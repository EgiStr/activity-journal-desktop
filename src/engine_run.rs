//! Runs `uv run journal …` in the engine's folder. With `--json` the engine prints one document on
//! stdout and its human lines on stderr: stderr is streamed to the Run log, stdout is collected
//! whole and parsed by `engine_json`. The app never interprets the engine's prose.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
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

impl RunKind {
    pub fn arguments(self) -> &'static [&'static str] {
        match self {
            RunKind::Final => &["run", "journal", "run", "--json"],
            RunKind::Checkpoint => &["run", "journal", "run", "--checkpoint", "--json"],
            RunKind::DryRun => &["run", "journal", "run", "--dry-run", "--json"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RunKind::Final => "Final run",
            RunKind::Checkpoint => "Checkpoint",
            RunKind::DryRun => "Dry run",
        }
    }
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

fn command(engine_root: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new("uv");
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
    let mut command = command(engine_root, arguments);
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
    kind: RunKind,
    messages: Sender<RunMessage>,
    wake: impl Fn() + Send + Sync + Clone + 'static,
) {
    let mut child = match command(engine_root, kind.arguments()).spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = messages.send(RunMessage::Line(format!("Could not start `uv`: {e}")));
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
        assert_eq!(RunKind::Final.arguments().join(" "), "run journal run --json");
        assert_eq!(
            RunKind::Checkpoint.arguments().join(" "),
            "run journal run --checkpoint --json"
        );
        assert_eq!(
            RunKind::DryRun.arguments().join(" "),
            "run journal run --dry-run --json"
        );
    }
}
