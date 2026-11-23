//! Runs `uv run journal …` in the engine's folder, or, when the owner turned the Rust runner on,
//! the engine's own `journal-rs` for a final run or a checkpoint. With `--json` either prints one
//! document on stdout (the same shape) and its human lines on stderr: stderr is streamed to the
//! Run log, stdout is collected whole and parsed by `engine_json`. The app never interprets the
//! engine's prose.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;

/// Which run the owner asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunKind {
    /// `journal run`: the final journal for yesterday and any backlog.
    Final,
    /// `journal run --checkpoint`: today's snapshot.
    Checkpoint,
    /// `journal run --dry-run`: what a run would do, touching nothing.
    DryRun,
}

/// Whether the engine at `engine_root` has the `--json` commands (Phase 1). An older engine
/// rejects `--json` as an unknown option, so it is run the old way instead.
pub fn speaks_json(engine_root: &Path) -> bool {
    engine_root.join("src/activity_journal/cli/run_document.py").is_file()
}

impl RunKind {
    pub fn arguments(self, json: bool) -> Vec<&'static str> {
        let mut arguments = match self {
            RunKind::Final => vec!["run", "journal", "run"],
            RunKind::Checkpoint => vec!["run", "journal", "run", "--checkpoint"],
            RunKind::DryRun => vec!["run", "journal", "run", "--dry-run"],
        };
        if json {
            arguments.push("--json");
        }
        arguments
    }

    pub fn label(self) -> &'static str {
        match self {
            RunKind::Final => "Final run",
            RunKind::Checkpoint => "Checkpoint",
            RunKind::DryRun => "Dry run",
        }
    }
}

/// Where the engine's release build of `journal-rs` lives.
pub fn journal_rs(engine_root: &Path) -> PathBuf {
    engine_root.join("rust/core/target/release/journal-rs.exe")
}

/// The program a run starts and its arguments.
#[derive(Debug)]
pub struct Invocation {
    pub program: PathBuf,
    pub arguments: Vec<String>,
}

impl Invocation {
    /// How the Run log names it: `uv run journal run --json`, or `journal-rs run --json`.
    pub fn describe(&self) -> String {
        let name = self.program.file_stem().map_or_else(
            || self.program.display().to_string(),
            |stem| stem.to_string_lossy().into_owned(),
        );
        let mut words = vec![name];
        words.extend(self.arguments.iter().take_while(|a| *a != "--engine").cloned());
        words.join(" ")
    }
}

/// What starts `kind`, and a note for the Run log when the Rust runner was asked for but the run
/// goes through `uv` instead because `journal-rs` is not built (`cargo build --release` in
/// `rust/core`).
pub fn invocation(engine_root: &Path, kind: RunKind, rust_runner: bool) -> (Invocation, Option<String>) {
    let python = || Invocation {
        program: PathBuf::from("uv"),
        arguments: kind
            .arguments(speaks_json(engine_root))
            .into_iter()
            .map(str::to_owned)
            .collect(),
    };
    if !rust_runner {
        return (python(), None);
    }
    let binary = journal_rs(engine_root);
    if !binary.is_file() {
        let note = format!(
            "journal-rs is not built at {}; the Python engine ran it",
            binary.display()
        );
        return (python(), Some(note));
    }
    let mut arguments = vec!["run".to_owned()];
    match kind {
        RunKind::Checkpoint => arguments.push("--checkpoint".to_owned()),
        // `journal run` catches up by default and `journal-rs` only when asked, so ask: the two
        // runners then do the same days.
        RunKind::Final => arguments.push("--catch-up".to_owned()),
        RunKind::DryRun => arguments.extend(["--dry-run".to_owned(), "--catch-up".to_owned()]),
    }
    arguments.extend([
        "--json".to_owned(),
        "--engine".to_owned(),
        engine_root.display().to_string(),
    ]);
    (
        Invocation {
            program: binary,
            arguments,
        },
        None,
    )
}

/// What a running engine reports back.
#[derive(Debug)]
pub enum RunMessage {
    Line(String),
    /// The exit code (`None` when the process could not be started or was killed) and stdout.
    Finished {
        code: Option<i32>,
        stdout: String,
    },
}

/// The result of a one-shot query such as `journal timeline --json`.
#[derive(Debug)]
pub struct QueryResult {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

fn command<S: AsRef<std::ffi::OsStr>>(program: &Path, engine_root: &Path, arguments: &[S]) -> Command {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(engine_root)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);
    command
}

/// Run `uv <arguments>` on a background thread and send its whole output once it ends.
pub fn query(engine_root: &Path, arguments: &[&str], done: Sender<QueryResult>, wake: impl Fn() + Send + 'static) {
    let mut command = command(Path::new("uv"), engine_root, arguments);
    std::thread::spawn(move || {
        let result = match command.output() {
            Ok(output) => QueryResult {
                code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
            Err(e) => QueryResult {
                code: None,
                stdout: String::new(),
                stderr: format!("Could not start `uv`: {e}"),
            },
        };
        let _ = done.send(result);
        wake();
    });
}

/// Start the run on background threads. `wake` is called after every message so the UI repaints.
pub fn start(
    engine_root: &Path,
    invocation: &Invocation,
    messages: Sender<RunMessage>,
    wake: impl Fn() + Send + Sync + Clone + 'static,
) {
    let mut child = match command(&invocation.program, engine_root, &invocation.arguments).spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = messages.send(RunMessage::Line(format!(
                "Could not start `{}`: {e}",
                invocation.program.display()
            )));
            let _ = messages.send(RunMessage::Finished {
                code: None,
                stdout: String::new(),
            });
            wake();
            return;
        }
    };
    let stdout = child.stdout.take().map(|mut stream| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stream.read_to_string(&mut text);
            text
        })
    });
    let stderr = child.stderr.take().map(|stream| {
        let messages = messages.clone();
        let wake = wake.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = messages.send(RunMessage::Line(line));
                wake();
            }
        })
    });
    std::thread::spawn(move || {
        let stdout = stdout.and_then(|t| t.join().ok()).unwrap_or_default();
        if let Some(pump) = stderr {
            let _ = pump.join();
        }
        let code = child.wait().ok().and_then(|status| status.code());
        let _ = messages.send(RunMessage::Finished { code, stdout });
        wake();
    });
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_match_the_engine_cli() {
        assert_eq!(RunKind::Final.arguments(true).join(" "), "run journal run --json");
        assert_eq!(
            RunKind::Checkpoint.arguments(true).join(" "),
            "run journal run --checkpoint --json"
        );
        assert_eq!(
            RunKind::DryRun.arguments(true).join(" "),
            "run journal run --dry-run --json"
        );
    }

    fn scratch_engine(test: &str, with_binary: bool) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("ajd-engine-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/activity_journal/cli")).unwrap();
        std::fs::write(root.join("src/activity_journal/cli/run_document.py"), "").unwrap();
        if with_binary {
            let binary = journal_rs(&root);
            std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
            std::fs::write(binary, "").unwrap();
        }
        root
    }

    #[test]
    fn without_the_rust_runner_every_run_goes_through_uv() {
        let root = scratch_engine("python", true);
        let (invocation, note) = invocation(&root, RunKind::Checkpoint, false);
        assert_eq!(invocation.program, std::path::PathBuf::from("uv"));
        assert_eq!(invocation.arguments.join(" "), "run journal run --checkpoint --json");
        assert_eq!(note, None);
    }

    #[test]
    fn the_rust_runner_runs_final_and_checkpoint_through_journal_rs() {
        let root = scratch_engine("rust", true);
        let engine = root.display().to_string();
        let (final_run, note) = invocation(&root, RunKind::Final, true);
        assert_eq!(final_run.program, journal_rs(&root));
        assert_eq!(
            final_run.arguments,
            ["run", "--catch-up", "--json", "--engine", engine.as_str()]
        );
        assert_eq!(note, None);
        let (checkpoint, _) = invocation(&root, RunKind::Checkpoint, true);
        assert_eq!(
            checkpoint.arguments,
            ["run", "--checkpoint", "--json", "--engine", engine.as_str()]
        );
    }

    #[test]
    fn the_rust_runner_plans_a_dry_run_over_the_same_days_as_a_final_run() {
        let root = scratch_engine("dry", true);
        let engine = root.display().to_string();
        let (dry, note) = invocation(&root, RunKind::DryRun, true);
        assert_eq!(dry.program, journal_rs(&root));
        assert_eq!(
            dry.arguments,
            ["run", "--dry-run", "--catch-up", "--json", "--engine", engine.as_str()]
        );
        assert_eq!(note, None);
    }

    #[test]
    fn a_missing_binary_falls_back_to_uv_and_says_so() {
        let root = scratch_engine("unbuilt", false);
        let (unbuilt, note) = invocation(&root, RunKind::Final, true);
        assert_eq!(unbuilt.program, std::path::PathBuf::from("uv"));
        assert!(note.unwrap().contains("not built"));
    }

    #[test]
    fn an_older_engine_is_run_without_json() {
        assert_eq!(RunKind::Final.arguments(false).join(" "), "run journal run");
        assert!(!speaks_json(Path::new("no such engine folder")));
    }
}
