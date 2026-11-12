//! The engine's `--json` documents, read field by field. Each document carries `schema_version`;
//! a version this app does not know is refused, never guessed at. A day with no stats stays
//! `None` and is shown as missing, never as zeros.

use serde_json::Value;

/// The `schema_version` of every engine document this app understands.
pub const SCHEMA_VERSION: u64 = 1;

/// What `journal run --json` reports, reduced to what the app shows and notifies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunReport {
    pub exit_code: i64,
    pub exit_reason: String,
    /// The run's own failure, or else the first failing day's.
    pub cause: Option<String>,
    /// Every source that degraded on any day, sorted and without repeats.
    pub degraded: Vec<String>,
}

/// One row of the stats file's `hours`.
#[derive(Clone, Debug, PartialEq)]
pub struct HourRow {
    pub hour: u32,
    pub project: String,
    pub source: String,
    pub events: u64,
    pub active_seconds: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CostRow {
    pub provider: String,
    pub cost_usd: f64,
    pub requests: u64,
    pub total_tokens: u64,
}

/// One day's stats document (PRD §4.6), as the engine wrote it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DayStats {
    pub day: String,
    pub timezone: String,
    pub partial: bool,
    /// Source name and status, sorted by name.
    pub sources: Vec<(String, String)>,
    pub total_events: u64,
    pub total_active_seconds: u64,
    pub hours: Vec<HourRow>,
    pub top_apps: Vec<(String, u64)>,
    pub top_domains: Vec<(String, u64)>,
    pub cost: Vec<CostRow>,
}

impl DayStats {
    /// The sources whose status is neither `ok` nor `disabled`.
    pub fn degraded(&self) -> Vec<&str> {
        self.sources
            .iter()
            .filter(|(_, status)| status != "ok" && status != "disabled")
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

/// `journal timeline --json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub day: String,
    /// `stored`, `snapshot` or `published`; empty when the lock was held.
    pub origin: String,
    pub status: Option<String>,
    pub exit_code: i64,
    pub exit_reason: String,
    pub failure: Option<String>,
    pub stats: Option<DayStats>,
}

/// One day of `journal history --json`.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryDay {
    pub day: String,
    /// `ok`, `no_stats`, `no_journal` or `unreadable`.
    pub state: String,
    pub error: Option<String>,
    pub stats: Option<DayStats>,
}

pub fn parse_run(text: &str) -> Result<RunReport, String> {
    let doc = document(text)?;
    let days = array(&doc, "days");
    let first_day_failure = days.iter().find_map(|d| opt_str(d, "failure"));
    let mut degraded: Vec<String> = days
        .iter()
        .flat_map(|d| array(d, "degraded"))
        .chain(array(&doc, "degraded")) // a checkpoint names its sources at the top level
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    degraded.sort();
    degraded.dedup();
    Ok(RunReport {
        exit_code: int(&doc, "exit_code")?,
        exit_reason: string(&doc, "exit_reason")?,
        cause: opt_str(&doc, "failure").or(first_day_failure),
        degraded,
    })
}

pub fn parse_timeline(text: &str) -> Result<Timeline, String> {
    let doc = document(text)?;
    Ok(Timeline {
        day: string(&doc, "day")?,
        origin: opt_str(&doc, "origin").unwrap_or_default(),
        status: opt_str(&doc, "status"),
        exit_code: int(&doc, "exit_code")?,
        exit_reason: string(&doc, "exit_reason")?,
        failure: opt_str(&doc, "failure"),
        stats: optional_stats(&doc)?,
    })
}

pub fn parse_history(text: &str) -> Result<Vec<HistoryDay>, String> {
    let doc = document(text)?;
    array(&doc, "days")
        .iter()
        .map(|d| {
            Ok(HistoryDay {
                day: string(d, "day")?,
                state: string(d, "state")?,
                error: opt_str(d, "error"),
                stats: optional_stats(d)?,
            })
        })
        .collect()
}

fn optional_stats(parent: &Value) -> Result<Option<DayStats>, String> {
    match parent.get("stats") {
        None | Some(Value::Null) => Ok(None),
        Some(stats) => stats_of(stats).map(Some),
    }
}

fn stats_of(v: &Value) -> Result<DayStats, String> {
    check_version(v)?;
    let totals = v.get("totals").ok_or("stats has no totals")?;
    let named = |key: &str, count: &str| -> Result<Vec<(String, u64)>, String> {
        array(v, key)
            .iter()
            .map(|row| Ok((string(row, "name")?, uint(row, count)?)))
            .collect()
    };
    let mut sources: Vec<(String, String)> = v
        .get("sources")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, s)| (k.clone(), s.as_str().unwrap_or("unknown").to_owned()))
                .collect()
        })
        .unwrap_or_default();
    sources.sort();
    Ok(DayStats {
        day: string(v, "day")?,
        timezone: opt_str(v, "timezone").unwrap_or_default(),
        partial: v.get("partial").and_then(Value::as_bool).unwrap_or(false),
        sources,
        total_events: uint(totals, "events")?,
        total_active_seconds: uint(totals, "active_seconds")?,
        hours: array(v, "hours")
            .iter()
            .map(|row| {
                Ok(HourRow {
                    hour: u32::try_from(uint(row, "hour")?).map_err(|e| e.to_string())?,
                    project: string(row, "project")?,
                    source: string(row, "source")?,
                    events: uint(row, "events")?,
                    active_seconds: uint(row, "active_seconds")?,
                })
            })
            .collect::<Result<_, String>>()?,
        top_apps: named("top_apps", "active_seconds")?,
        top_domains: named("top_domains", "visits")?,
        cost: array(v, "cost")
            .iter()
            .map(|row| {
                Ok(CostRow {
                    provider: string(row, "provider")?,
                    cost_usd: row.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                    requests: uint(row, "requests")?,
                    total_tokens: uint(row, "total_tokens")?,
                })
            })
            .collect::<Result<_, String>>()?,
    })
}

fn document(text: &str) -> Result<Value, String> {
    let doc: Value = serde_json::from_str(text.trim()).map_err(|e| format!("engine output is not JSON: {e}"))?;
    check_version(&doc)?;
    Ok(doc)
}

fn check_version(v: &Value) -> Result<(), String> {
    match v.get("schema_version").and_then(Value::as_u64) {
        Some(SCHEMA_VERSION) => Ok(()),
        other => Err(format!(
            "engine schema_version {} is not the {SCHEMA_VERSION} this app understands; update the app",
            other.map_or_else(|| "missing".to_owned(), |n| n.to_string())
        )),
    }
}

fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn string(v: &Value, key: &str) -> Result<String, String> {
    opt_str(v, key).ok_or_else(|| format!("engine output has no text field `{key}`"))
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn int(v: &Value, key: &str) -> Result<i64, String> {
    v.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("engine output has no number `{key}`"))
}

fn uint(v: &Value, key: &str) -> Result<u64, String> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("engine output has no count `{key}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATS: &str = r#"{"schema_version":1,"day":"2026-11-10","timezone":"Asia/Jakarta",
        "partial":true,"sources":{"git":"ok","chrome":"timeout","codex":"disabled"},
        "totals":{"events":3,"active_seconds":1500},
        "hours":[{"hour":9,"project":"alpha","source":"activitywatch","events":2,"active_seconds":1500},
                 {"hour":11,"project":"unassigned","source":"git","events":1,"active_seconds":0}],
        "top_apps":[{"name":"code.exe","active_seconds":1500}],
        "top_domains":[{"name":"github.com","visits":4}],
        "cost":[{"provider":"openai","cost_usd":0.3,"requests":4,"total_tokens":1050}]}"#;

    #[test]
    fn a_partial_run_names_its_degraded_sources_once() {
        let text = r#"{"schema_version":1,"kind":"run","exit_code":10,"exit_reason":"partial",
            "failure":null,"days":[{"failure":null,"degraded":["chrome"]},
            {"failure":null,"degraded":["chrome","git"]}]}"#;
        let report = parse_run(text).unwrap();
        assert_eq!(report.exit_code, 10);
        assert_eq!(report.degraded, ["chrome", "git"]);
        assert_eq!(report.cause, None);
    }

    /// Documents `journal-rs run --json` printed on a scratch engine: the app reads them as it
    /// reads `journal run --json`.
    #[test]
    fn journal_rs_documents_read_like_the_python_ones() {
        let checkpoint = r#"{"committed_sha":"654cc08c02c2ab4082b2fda97051d6b4264e3262","day":"2026-11-10","degraded":[],"exit_code":0,"exit_reason":"ok","failure":null,"kind":"checkpoint","published":true,"schema_version":1,"skipped_plugins":{},"sources":{"git-commit-collector":"ok"}}"#;
        let report = parse_run(checkpoint).unwrap();
        assert_eq!((report.exit_code, report.exit_reason.as_str()), (0, "ok"));
        assert!(report.degraded.is_empty());
        assert_eq!(report.cause, None);

        let closed = r#"{"blocked_by_backlog":false,"days":[{"actions":[],"day":"2026-11-10","degraded":[],"exit_code":40,"exit_reason":"day_closed","failure":null,"forced":false,"published":false,"sources":{},"status":"purged"}],"exit_code":40,"exit_reason":"day_closed","failure":null,"kind":"run","lock_held":false,"planned":["2026-11-10"],"plans":[],"schema_version":1,"skipped":["2026-11-10"],"skipped_plugins":{},"untouched":[]}"#;
        let report = parse_run(closed).unwrap();
        assert_eq!((report.exit_code, report.exit_reason.as_str()), (40, "day_closed"));
    }

    #[test]
    fn a_blocked_run_takes_its_cause_from_the_run_then_the_day() {
        let day_only = r#"{"schema_version":1,"exit_code":20,"exit_reason":"stuck","failure":null,
            "days":[{"failure":"remote is ahead","degraded":[]}]}"#;
        assert_eq!(parse_run(day_only).unwrap().cause.as_deref(), Some("remote is ahead"));
        let checkpoint = r#"{"schema_version":1,"kind":"checkpoint","exit_code":75,
            "exit_reason":"locked","failure":"run lock is held by process 9","degraded":[]}"#;
        assert_eq!(
            parse_run(checkpoint).unwrap().cause.as_deref(),
            Some("run lock is held by process 9")
        );
    }

    #[test]
    fn an_unknown_schema_version_is_refused() {
        let error = parse_run(r#"{"schema_version":2,"exit_code":0,"exit_reason":"ok"}"#).unwrap_err();
        assert!(error.contains("schema_version 2"), "{error}");
        assert!(parse_run("dry-run: no changes made").is_err());
    }

    #[test]
    fn a_timeline_carries_the_whole_stats_document() {
        let text = format!(
            r#"{{"schema_version":1,"day":"2026-11-10","origin":"snapshot","status":null,
            "exit_code":10,"exit_reason":"partial","failure":null,"stats":{STATS}}}"#
        );
        let timeline = parse_timeline(&text).unwrap();
        let stats = timeline.stats.unwrap();
        assert_eq!(timeline.origin, "snapshot");
        assert_eq!(stats.total_events, 3);
        assert_eq!(stats.hours[0].project, "alpha");
        assert_eq!(stats.degraded(), ["chrome"]);
        assert_eq!(stats.top_domains, [("github.com".to_owned(), 4)]);
        assert!((stats.cost[0].cost_usd - 0.3).abs() < 1e-9);
    }

    #[test]
    fn a_locked_timeline_has_no_stats_rather_than_zeros() {
        let text = r#"{"schema_version":1,"day":"2026-11-12","origin":null,"status":null,
            "exit_code":75,"exit_reason":"locked","failure":"run lock is held","stats":null}"#;
        let timeline = parse_timeline(text).unwrap();
        assert!(timeline.stats.is_none());
        assert_eq!(timeline.exit_reason, "locked");
    }

    #[test]
    fn history_keeps_missing_days_missing() {
        let text = format!(
            r#"{{"schema_version":1,"from":"2026-11-10","to":"2026-11-11","days":[
            {{"day":"2026-11-10","state":"ok","stats":{STATS}}},
            {{"day":"2026-11-11","state":"no_journal","stats":null}}]}}"#
        );
        let days = parse_history(&text).unwrap();
        assert_eq!(days[0].stats.as_ref().unwrap().total_active_seconds, 1500);
        assert_eq!(days[1].state, "no_journal");
        assert!(days[1].stats.is_none());
    }
}
