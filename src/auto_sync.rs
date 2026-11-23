//! The "sync to GitHub every N minutes" timer.
//!
//! A sync is a checkpoint run: it publishes today's snapshot to the journal repository's remote.
//! The decision of when one is due is kept here, away from the window, so it can be tested.

use std::time::{Duration, Instant};

/// How often a sync runs unless the owner picks another interval.
pub const DEFAULT_MINUTES: u64 = 10;
pub const MIN_MINUTES: u64 = 1;
/// One day: longer than that is the nightly run's job.
pub const MAX_MINUTES: u64 = 24 * 60;

/// An interval from the settings file: a missing or absurd value becomes something usable.
pub fn clamp_minutes(minutes: Option<i64>) -> u64 {
    minutes.map_or(DEFAULT_MINUTES, |m| {
        m.clamp(MIN_MINUTES as i64, MAX_MINUTES as i64) as u64
    })
}

/// The moment the next sync is due, or `None` while syncing is off.
pub fn next_due(enabled: bool, last_sync: Instant, minutes: u64) -> Option<Instant> {
    enabled.then(|| last_sync + Duration::from_secs(minutes * 60))
}

/// Whether to start a sync now: it is on, the interval has passed, and no run is going.
pub fn is_due(enabled: bool, last_sync: Instant, minutes: u64, now: Instant, running: bool) -> bool {
    !running && next_due(enabled, last_sync, minutes).is_some_and(|due| now >= due)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_ten_minutes_and_a_bad_value_is_made_usable() {
        assert_eq!(clamp_minutes(None), 10);
        assert_eq!(clamp_minutes(Some(0)), 1);
        assert_eq!(clamp_minutes(Some(-5)), 1);
        assert_eq!(clamp_minutes(Some(30)), 30);
        assert_eq!(clamp_minutes(Some(1_000_000)), MAX_MINUTES);
    }

    #[test]
    fn a_sync_is_due_only_when_on_idle_and_the_interval_has_passed() {
        let last = Instant::now();
        let after = last + Duration::from_secs(10 * 60);

        assert!(is_due(true, last, 10, after, false));
        assert!(
            !is_due(true, last, 10, after - Duration::from_secs(1), false),
            "too early"
        );
        assert!(!is_due(false, last, 10, after, false), "off");
        assert!(!is_due(true, last, 10, after, true), "a run is already going");
    }

    #[test]
    fn nothing_is_due_while_syncing_is_off() {
        assert_eq!(next_due(false, Instant::now(), 10), None);
    }
}
