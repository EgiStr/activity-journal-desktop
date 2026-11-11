//! Breakdowns of one day's stats for the dashboard. Every number is a sum of the engine's own
//! `hours` rows, so the panels add up to the engine's `totals` and to each other.

use std::collections::BTreeMap;

use crate::engine_json::DayStats;

/// What the hourly chart and the breakdowns measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// ActivityWatch foreground time, in minutes on the chart.
    Active,
    /// Every event of every kind.
    Events,
}

impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Metric::Active => "Active minutes",
            Metric::Events => "Events",
        }
    }
}

/// One row of a breakdown: a name with its event count and active seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    pub events: u64,
    pub active_seconds: u64,
}

pub fn by_project(stats: &DayStats) -> Vec<Share> {
    breakdown(stats, |row| row.project.as_str())
}

pub fn by_source(stats: &DayStats) -> Vec<Share> {
    breakdown(stats, |row| row.source.as_str())
}

/// Largest first by active time, then by events, then by name, so the order is stable.
fn breakdown(stats: &DayStats, key: impl Fn(&crate::engine_json::HourRow) -> &str) -> Vec<Share> {
    let mut sums: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for row in &stats.hours {
        let entry = sums.entry(key(row)).or_default();
        entry.0 += row.events;
        entry.1 += row.active_seconds;
    }
    let mut shares: Vec<Share> = sums
        .into_iter()
        .map(|(name, (events, active_seconds))| Share {
            name: name.to_owned(),
            events,
            active_seconds,
        })
        .collect();
    shares.sort_by(|a, b| {
        b.active_seconds
            .cmp(&a.active_seconds)
            .then(b.events.cmp(&a.events))
            .then(a.name.cmp(&b.name))
    });
    shares
}

/// Per project (in `by_project` order), the value of each local hour 0–23.
pub fn hourly(stats: &DayStats, metric: Metric) -> Vec<(String, [f64; 24])> {
    by_project(stats)
        .into_iter()
        .map(|share| {
            let mut hours = [0.0; 24];
            for row in stats.hours.iter().filter(|r| r.project == share.name) {
                if let Some(slot) = hours.get_mut(row.hour as usize) {
                    *slot += match metric {
                        Metric::Active => row.active_seconds as f64 / 60.0,
                        Metric::Events => row.events as f64,
                    };
                }
            }
            (share.name, hours)
        })
        .collect()
}

/// Whether the rows add up to the engine's totals. A mismatch is shown, never hidden.
pub fn reconciles(stats: &DayStats) -> bool {
    let events: u64 = stats.hours.iter().map(|r| r.events).sum();
    let active: u64 = stats.hours.iter().map(|r| r.active_seconds).sum();
    events == stats.total_events && active == stats.total_active_seconds
}

pub fn cost_total(stats: &DayStats) -> f64 {
    stats.cost.iter().map(|c| c.cost_usd).sum()
}

/// `3725` seconds as `1h 02m`; under an hour as `12m`.
pub fn duration(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes >= 60 {
        format!("{}h {:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_json::{CostRow, HourRow};

    fn row(hour: u32, project: &str, source: &str, events: u64, active: u64) -> HourRow {
        HourRow {
            hour,
            project: project.into(),
            source: source.into(),
            events,
            active_seconds: active,
        }
    }

    fn stats() -> DayStats {
        DayStats {
            day: "2026-11-10".into(),
            total_events: 7,
            total_active_seconds: 3000,
            hours: vec![
                row(9, "alpha", "activitywatch", 2, 1800),
                row(9, "unassigned", "git", 1, 0),
                row(14, "alpha", "git", 3, 0),
                row(14, "beta", "activitywatch", 1, 1200),
            ],
            cost: vec![
                CostRow {
                    provider: "a".into(),
                    cost_usd: 0.1,
                    requests: 1,
                    total_tokens: 5,
                },
                CostRow {
                    provider: "b".into(),
                    cost_usd: 0.2,
                    requests: 1,
                    total_tokens: 5,
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn breakdowns_add_up_to_the_engine_totals() {
        let s = stats();
        assert!(reconciles(&s));
        for shares in [by_project(&s), by_source(&s)] {
            assert_eq!(shares.iter().map(|x| x.events).sum::<u64>(), s.total_events);
            assert_eq!(
                shares.iter().map(|x| x.active_seconds).sum::<u64>(),
                s.total_active_seconds
            );
        }
    }

    #[test]
    fn projects_are_ordered_by_active_time() {
        let names: Vec<_> = by_project(&stats()).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["alpha", "beta", "unassigned"]);
    }

    #[test]
    fn the_hourly_matrix_places_each_row_in_its_local_hour() {
        let matrix = hourly(&stats(), Metric::Active);
        assert_eq!(matrix[0].0, "alpha");
        assert_eq!(matrix[0].1[9], 30.0);
        assert_eq!(matrix[1].1[14], 20.0);
        let events = hourly(&stats(), Metric::Events);
        let total: f64 = events.iter().flat_map(|(_, h)| h.iter()).sum();
        assert_eq!(total, 7.0);
    }

    #[test]
    fn a_mismatch_is_reported() {
        let mut s = stats();
        s.total_events = 8;
        assert!(!reconciles(&s));
    }

    #[test]
    fn cost_and_durations_read_plainly() {
        assert!((cost_total(&stats()) - 0.3).abs() < 1e-9);
        assert_eq!(duration(3725), "1h 02m");
        assert_eq!(duration(720), "12m");
    }
}
