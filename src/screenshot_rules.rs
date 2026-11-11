//! The screenshot daemon's decision, ported from the engine's `cli/daemon_command.py`. The three
//! rules apply in the same order: free-space floor, then exclusions, then capture. An excluded
//! window means the screen is not grabbed at all, so this module decides before any pixel exists.

use std::path::{Component, Path, PathBuf};

use chrono::DateTime;
use chrono_tz::Tz;

pub const SCREENSHOTS_DIRECTORY: &str = "screenshots";

/// `[screenshot]` from the engine's settings, with the engine's defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenshotSettings {
    pub interval_seconds: u64,
    pub excluded_processes: Vec<String>,
    pub excluded_title_keywords: Vec<String>,
    pub minimum_free_megabytes: u64,
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self {
            interval_seconds: 300,
            excluded_processes: Vec::new(),
            excluded_title_keywords: Vec::new(),
            minimum_free_megabytes: 2048,
        }
    }
}

/// The foreground window: the owning process's executable name and the window title.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveWindow {
    pub process_name: String,
    pub title: String,
}

/// What one iteration decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Below the free-space floor: the daemon stops.
    Stopped(String),
    /// The foreground window is excluded: nothing is grabbed.
    Skipped(String),
    /// Grab the screen into this file.
    Capture(PathBuf),
}

/// Apply the three rules, in the engine's order.
pub fn decide(
    settings: &ScreenshotSettings,
    data_dir: &Path,
    free_megabytes: u64,
    window: &ActiveWindow,
    now: DateTime<Tz>,
) -> Result<Decision, String> {
    if free_megabytes < settings.minimum_free_megabytes {
        return Ok(Decision::Stopped(format!(
            "free space {free_megabytes} MB is below the {} MB floor; capture stopped",
            settings.minimum_free_megabytes
        )));
    }
    if let Some(reason) = excluded_reason(settings, window) {
        return Ok(Decision::Skipped(reason));
    }
    target_path(data_dir, now).map(Decision::Capture)
}

fn excluded_reason(settings: &ScreenshotSettings, window: &ActiveWindow) -> Option<String> {
    let process = window.process_name.to_lowercase();
    for name in &settings.excluded_processes {
        if !process.is_empty() && process == name.to_lowercase() {
            return Some(format!("process '{name}' is excluded; capture skipped"));
        }
    }
    let title = window.title.to_lowercase();
    for keyword in &settings.excluded_title_keywords {
        if !keyword.is_empty() && title.contains(&keyword.to_lowercase()) {
            return Some(format!(
                "window title matches excluded keyword '{keyword}'; capture skipped"
            ));
        }
    }
    None
}

/// `<data_dir>/screenshots/<day>/<HHMMSS>.png`, with the day in the configured timezone, refusing
/// any path that would land outside the data directory.
fn target_path(data_dir: &Path, now: DateTime<Tz>) -> Result<PathBuf, String> {
    let root = normalise(data_dir);
    let directory = normalise(
        &root
            .join(SCREENSHOTS_DIRECTORY)
            .join(now.format("%Y-%m-%d").to_string()),
    );
    if !directory.starts_with(&root) {
        return Err("screenshot directory resolves outside the data directory".to_owned());
    }
    Ok(directory.join(format!("{}.png", now.format("%H%M%S"))))
}

/// Collapse `.` and `..` without touching the disk, like Python's `resolve()` on a missing path.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn jakarta(hour: u32, minute: u32, second: u32) -> DateTime<Tz> {
        chrono_tz::Asia::Jakarta
            .with_ymd_and_hms(2026, 11, 11, hour, minute, second)
            .unwrap()
    }

    fn settings() -> ScreenshotSettings {
        ScreenshotSettings {
            excluded_processes: vec!["KeePass.exe".into()],
            excluded_title_keywords: vec!["Private".into(), String::new()],
            ..ScreenshotSettings::default()
        }
    }

    fn window(process: &str, title: &str) -> ActiveWindow {
        ActiveWindow {
            process_name: process.into(),
            title: title.into(),
        }
    }

    #[test]
    fn free_space_is_checked_before_exclusions() {
        let decision = decide(
            &settings(),
            Path::new("D:/data"),
            10,
            &window("keepass.exe", "x"),
            jakarta(9, 0, 0),
        );
        assert_eq!(
            decision.unwrap(),
            Decision::Stopped("free space 10 MB is below the 2048 MB floor; capture stopped".into())
        );
    }

    #[test]
    fn process_match_is_exact_and_case_insensitive() {
        let skipped = decide(
            &settings(),
            Path::new("D:/data"),
            9999,
            &window("keepass.EXE", ""),
            jakarta(9, 0, 0),
        );
        assert!(matches!(skipped.unwrap(), Decision::Skipped(r) if r.contains("KeePass.exe")));
        let captured = decide(
            &settings(),
            Path::new("D:/data"),
            9999,
            &window("keepass2.exe", ""),
            jakarta(9, 0, 0),
        );
        assert!(matches!(captured.unwrap(), Decision::Capture(_)));
    }

    #[test]
    fn title_keyword_is_a_substring_and_empty_keywords_never_match() {
        let skipped = decide(
            &settings(),
            Path::new("D:/data"),
            9999,
            &window("", "InPrivate - Edge"),
            jakarta(9, 0, 0),
        );
        assert!(matches!(skipped.unwrap(), Decision::Skipped(r) if r.contains("'Private'")));
        let captured = decide(
            &settings(),
            Path::new("D:/data"),
            9999,
            &window("", "Notes"),
            jakarta(9, 0, 0),
        );
        assert!(matches!(captured.unwrap(), Decision::Capture(_)));
    }

    #[test]
    fn an_empty_process_name_matches_no_process_exclusion() {
        let only_process = ScreenshotSettings {
            excluded_processes: vec![String::new()],
            ..ScreenshotSettings::default()
        };
        let decision = decide(
            &only_process,
            Path::new("D:/data"),
            9999,
            &window("", ""),
            jakarta(9, 0, 0),
        );
        assert!(matches!(decision.unwrap(), Decision::Capture(_)));
    }

    #[test]
    fn the_file_lands_in_the_local_day_directory() {
        let decision = decide(
            &settings(),
            Path::new("D:/data/./x/.."),
            9999,
            &window("", ""),
            jakarta(23, 59, 7),
        );
        assert_eq!(
            decision.unwrap(),
            Decision::Capture(PathBuf::from("D:/data/screenshots/2026-11-11/235907.png"))
        );
    }

    #[test]
    fn the_day_follows_the_configured_timezone_not_utc() {
        // 01:30 in Jakarta is still the previous day in UTC; the directory must use Jakarta's day.
        let decision = decide(
            &settings(),
            Path::new("D:/data"),
            9999,
            &window("", ""),
            jakarta(1, 30, 0),
        );
        assert_eq!(
            decision.unwrap(),
            Decision::Capture(PathBuf::from("D:/data/screenshots/2026-11-11/013000.png"))
        );
    }
}
