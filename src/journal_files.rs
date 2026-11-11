//! The published journals on disk: `<journal_dir>/journal/YYYY/MM/YYYY-MM-DD[-checkpoint].md`.
//! Read-only. The app shows them; it never edits a journal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The journal files that exist for one day.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DayJournals {
    pub final_journal: Option<PathBuf>,
    pub checkpoint: Option<PathBuf>,
}

/// Map each day id to its journal files, newest day first when iterated in reverse.
pub fn find_journals(journal_dir: &Path) -> BTreeMap<String, DayJournals> {
    let mut days: BTreeMap<String, DayJournals> = BTreeMap::new();
    let root = journal_dir.join("journal");
    for year in subdirectories(&root) {
        for month in subdirectories(&year) {
            let Ok(files) = std::fs::read_dir(&month) else { continue };
            for path in files.filter_map(Result::ok).map(|f| f.path()) {
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned) else {
                    continue;
                };
                let stem = stem.as_str();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                match stem.strip_suffix("-checkpoint") {
                    Some(day) if is_day_id(day) => days.entry(day.to_owned()).or_default().checkpoint = Some(path),
                    None if is_day_id(stem) => days.entry(stem.to_owned()).or_default().final_journal = Some(path),
                    _ => {}
                }
            }
        }
    }
    days
}

fn subdirectories(path: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

fn is_day_id(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_final_and_checkpoint_and_ignores_others() {
        let root = std::env::temp_dir().join(format!("ajd-journals-{}", std::process::id()));
        let month = root.join("journal").join("2026").join("11");
        std::fs::create_dir_all(&month).unwrap();
        for name in [
            "2026-11-10.md",
            "2026-11-11-checkpoint.md",
            "README.md",
            "2026-11-10.txt",
        ] {
            std::fs::write(month.join(name), "x").unwrap();
        }
        let days = find_journals(&root);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(days.keys().collect::<Vec<_>>(), ["2026-11-10", "2026-11-11"]);
        assert!(days["2026-11-10"].final_journal.is_some() && days["2026-11-10"].checkpoint.is_none());
        assert!(days["2026-11-11"].checkpoint.is_some() && days["2026-11-11"].final_journal.is_none());
    }

    #[test]
    fn day_ids_are_strict() {
        assert!(is_day_id("2026-11-10"));
        assert!(!is_day_id("2026-1-10"));
        assert!(!is_day_id("README"));
    }
}
