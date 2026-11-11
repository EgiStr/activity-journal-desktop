//! Read-only view of the engine's `state.json`. The app never writes it: only the engine moves a day.

use std::path::Path;

/// One day as the engine last recorded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayRecord {
    pub day_id: String,
    pub status: String,
    pub since: String,
    pub stuck_cause: Option<String>,
}

/// Parse the state file text; days are returned newest first.
pub fn parse_state(text: &str) -> Result<Vec<DayRecord>, String> {
    let raw: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("state.json is not JSON: {e}"))?;
    let object = raw.as_object().ok_or("state.json must hold a JSON object")?;
    let mut days = Vec::with_capacity(object.len());
    for (day_id, entry) in object {
        let field = |name: &str| entry.get(name).and_then(|v| v.as_str()).map(str::to_owned);
        let status = field("status").ok_or_else(|| format!("day {day_id} has no status"))?;
        days.push(DayRecord {
            day_id: day_id.clone(),
            status,
            since: field("since").unwrap_or_default(),
            stuck_cause: field("stuck_cause"),
        });
    }
    days.sort_by(|a, b| b.day_id.cmp(&a.day_id));
    Ok(days)
}

/// Read `state.json`; a missing file means no day has started yet.
pub fn read_state(path: &Path) -> Result<Vec<DayRecord>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_state(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// The pid the engine wrote beside its run lock. Informational only: the OS lock decides.
pub fn lock_owner(data_dir: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(data_dir.join("run.lock.pid")).ok()?;
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_newest_first_with_stuck_cause() {
        let text = r#"{
          "2026-11-09": {"status": "purged", "since": "2026-11-10T00:16:00+07:00", "stuck_cause": null, "stuck_since": null},
          "2026-11-10": {"status": "committed", "since": "2026-11-11T00:15:00+07:00", "stuck_cause": "push rejected", "stuck_since": "x"}
        }"#;
        let days = parse_state(text).unwrap();
        assert_eq!(days[0].day_id, "2026-11-10");
        assert_eq!(days[0].stuck_cause.as_deref(), Some("push rejected"));
        assert_eq!(days[1].status, "purged");
        assert_eq!(days[1].stuck_cause, None);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_list() {
        assert!(parse_state("[]").is_err());
        assert!(parse_state("{\"d\": {}}").is_err());
        assert!(parse_state("nope").is_err());
    }

    #[test]
    fn a_missing_file_means_no_days() {
        assert_eq!(
            read_state(Path::new("definitely/missing/state.json")).unwrap(),
            Vec::new()
        );
    }
}
