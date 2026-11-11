//! Runs `uv run journal run …` in the engine's folder and streams its text output back. The app
//! reads only the exit code and the lines; it never interprets the engine's prose.

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
            RunKind::Final => &["run", "journal", "run"],
            RunKind::Checkpoint => &["run", "journal", "run", "--checkpoint"],
            RunKind::DryRun => &["run", "journal", "run", "--dry-run"],
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
    /// The exit code, or `None` when the process could not be started or was killed.
    Finished(Option<i32>),
}

/// Start the run on background threads. `wake` is called after every message so the UI repaints.
pub fn start(
    engine_root: &Path,
    kind: RunKind,
    messages: Sender<RunMessage>,
    wake: impl Fn() + Send + Sync + Clone + 'static,
) {
    let mut command = Command::new("uv");
    command
        .args(kind.arguments())
        .current_dir(engine_root)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = messages.send(RunMessage::Line(format!("Could not start `uv`: {e}")));
            let _ = messages.send(RunMessage::Finished(None));
            wake();
            return;
        }
    };
    let readers = [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ];
    let pumps: Vec<_> = readers
        .into_iter()
        .flatten()
        .map(|stream| {
            let messages = messages.clone();
            let wake = wake.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let _ = messages.send(RunMessage::Line(line));
                    wake();
                }
            })
        })
        .collect();
    std::thread::spawn(move || {
        for pump in pumps {
            let _ = pump.join();
        }
        let code = child.wait().ok().and_then(|status| status.code());
        let _ = messages.send(RunMessage::Finished(code));
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
        assert_eq!(RunKind::Final.arguments().join(" "), "run journal run");
        assert_eq!(
            RunKind::Checkpoint.arguments().join(" "),
            "run journal run --checkpoint"
        );
        assert_eq!(RunKind::DryRun.arguments().join(" "), "run journal run --dry-run");
    }
}
