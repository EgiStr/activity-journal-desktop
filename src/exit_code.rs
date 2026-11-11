//! What one `journal run` exit code means. The engine owns these numbers; this module only names
//! them, so the tray never collapses a privacy block or a held lock into a generic failure.

/// The outcome class that decides the tray colour and whether a retry is safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Caveat,
    Blocked,
}

/// One engine exit code with its meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub label: &'static str,
    pub advice: &'static str,
    pub severity: Severity,
}

/// Name an exit code. Codes the engine does not define are reported as such, never as success.
pub fn outcome_of(code: i32) -> Outcome {
    let (label, advice, severity) = match code {
        0 => ("ok", "The journal was published and verified.", Severity::Ok),
        10 => (
            "partial",
            "Published, but a source degraded or the day was rebuilt by force.",
            Severity::Caveat,
        ),
        20 => (
            "stuck",
            "Nothing was purged. Safe to rerun once the cause is fixed; do not retry blindly.",
            Severity::Blocked,
        ),
        30 => (
            "privacy blocked",
            "A credential survived redaction. Nothing was published.",
            Severity::Blocked,
        ),
        40 => ("day already closed", "That day is already published.", Severity::Caveat),
        75 => (
            "lock held",
            "Another run holds the lock. Wait for it to finish.",
            Severity::Blocked,
        ),
        _ => (
            "unknown exit code",
            "The engine ended in a way this app does not know.",
            Severity::Blocked,
        ),
    };
    Outcome {
        code,
        label,
        advice,
        severity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_code_has_its_own_meaning() {
        let labels: Vec<&str> = [0, 10, 20, 30, 40, 75].iter().map(|c| outcome_of(*c).label).collect();
        assert_eq!(
            labels,
            [
                "ok",
                "partial",
                "stuck",
                "privacy blocked",
                "day already closed",
                "lock held"
            ]
        );
    }

    #[test]
    fn only_zero_is_ok_and_blocking_codes_are_blocked() {
        assert_eq!(outcome_of(0).severity, Severity::Ok);
        assert_eq!(outcome_of(10).severity, Severity::Caveat);
        for code in [20, 30, 75, 1, -1] {
            assert_eq!(outcome_of(code).severity, Severity::Blocked, "code {code}");
        }
    }
}
