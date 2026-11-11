//! Windows notifications for a run the app started and for the screenshot loop stopping. The text
//! always names the cause; a block is never reported as a plain "failed".

use crate::engine_run::RunKind;
use crate::exit_code::{Outcome, Severity};

const APP_NAME: &str = "Activity Journal";

/// The title and body for a finished run. `cause` is the run's own failure (from `run --json`, or
/// `state.json` for an older engine); `degraded` names the sources a partial run lost.
pub fn run_finished(
    kind: RunKind,
    outcome: Option<Outcome>,
    cause: Option<&str>,
    degraded: &[String],
) -> (String, String) {
    let Some(o) = outcome else {
        return (
            format!("{}: could not start", kind.label()),
            "The engine did not report an exit code. Check the Run tab.".to_owned(),
        );
    };
    let title = match o.severity {
        Severity::Ok => format!("{}: done", kind.label()),
        Severity::Caveat => format!("{}: {} (exit {})", kind.label(), o.label, o.code),
        Severity::Blocked => format!("{} blocked: {} (exit {})", kind.label(), o.label, o.code),
    };
    let body = match cause {
        Some(cause) if o.severity == Severity::Blocked => format!("{} Cause: {cause}", o.advice),
        _ if o.severity == Severity::Caveat && !degraded.is_empty() => {
            format!("{} Degraded: {}.", o.advice, degraded.join(", "))
        }
        _ => o.advice.to_owned(),
    };
    (title, body)
}

/// The title and body when the screenshot loop stops itself.
pub fn screenshots_stopped(reason: &str) -> (String, String) {
    ("Screenshots stopped".to_owned(), reason.to_owned())
}

/// Show a notification without blocking the UI thread. A failure to show one is not an error the
/// owner can act on, so it is dropped.
pub fn show((title, body): (String, String)) {
    std::thread::spawn(move || {
        let _ = notify_rust::Notification::new()
            .appname(APP_NAME)
            .summary(&title)
            .body(&body)
            .show();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit_code::outcome_of;

    #[test]
    fn a_block_names_its_code_and_the_recorded_cause() {
        let (title, body) = run_finished(RunKind::Final, Some(outcome_of(20)), Some("remote is ahead"), &[]);
        assert_eq!(title, "Final run blocked: stuck (exit 20)");
        assert!(body.ends_with("Cause: remote is ahead"), "{body}");
    }

    #[test]
    fn privacy_and_lock_are_never_a_generic_failure() {
        assert_eq!(
            run_finished(RunKind::Checkpoint, Some(outcome_of(30)), None, &[]).0,
            "Checkpoint blocked: privacy blocked (exit 30)"
        );
        assert_eq!(
            run_finished(RunKind::Checkpoint, Some(outcome_of(75)), None, &[]).0,
            "Checkpoint blocked: lock held (exit 75)"
        );
    }

    #[test]
    fn partial_is_success_with_a_caveat_and_ignores_stale_causes() {
        let degraded = vec!["chrome".to_owned(), "git".to_owned()];
        let (title, body) = run_finished(RunKind::Final, Some(outcome_of(10)), Some("old cause"), &degraded);
        assert_eq!(title, "Final run: partial (exit 10)");
        assert!(!body.contains("old cause"));
        assert!(body.ends_with("Degraded: chrome, git."), "{body}");
        assert_eq!(
            run_finished(RunKind::DryRun, Some(outcome_of(0)), None, &[]).0,
            "Dry run: done"
        );
    }

    #[test]
    fn a_run_that_never_started_says_so() {
        assert_eq!(
            run_finished(RunKind::Final, None, None, &[]).0,
            "Final run: could not start"
        );
    }
}
