//! Full-text search over the published journals. Read-only: it reads the same Markdown files the
//! Journal tab shows and never edits one.
//!
//! A line matches when it contains every word of the query, ignoring case. A day counts once: its
//! final journal when it has one, otherwise its checkpoint, so a word is not reported twice for the
//! same day.

use std::collections::BTreeMap;

use crate::journal_files::DayJournals;

pub const MAX_HITS: usize = 200;
pub const MAX_PER_DAY: usize = 5;
const SNIPPET_CHARS: usize = 160;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub day: String,
    /// The day only has a checkpoint, so the text may be unfinished.
    pub checkpoint: bool,
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResult {
    pub hits: Vec<Hit>,
    /// More matches exist than are listed.
    pub truncated: bool,
    pub days_searched: usize,
    /// Files that could not be read, so a miss is not mistaken for "not there".
    pub unreadable: usize,
}

/// Search every day, newest first.
pub fn search(journals: &BTreeMap<String, DayJournals>, query: &str) -> SearchResult {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut result = SearchResult::default();
    if words.is_empty() {
        return result;
    }
    for (day, files) in journals.iter().rev() {
        let (path, checkpoint) = match (&files.final_journal, &files.checkpoint) {
            (Some(path), _) => (path, false),
            (None, Some(path)) => (path, true),
            (None, None) => continue,
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            result.unreadable += 1;
            continue;
        };
        result.days_searched += 1;
        let mut in_day = 0;
        for (index, line) in text.lines().enumerate() {
            let lowered = line.to_lowercase();
            if !words.iter().all(|word| lowered.contains(word.as_str())) {
                continue;
            }
            if result.hits.len() >= MAX_HITS {
                result.truncated = true;
                return result;
            }
            if in_day >= MAX_PER_DAY {
                result.truncated = true;
                break;
            }
            in_day += 1;
            result.hits.push(Hit {
                day: day.clone(),
                checkpoint,
                line: index + 1,
                text: snippet(line),
            });
        }
    }
    result
}

fn snippet(line: &str) -> String {
    let trimmed = line.trim().trim_start_matches(['#', '-', '*', ' ']);
    if trimmed.chars().count() <= SNIPPET_CHARS {
        return trimmed.to_owned();
    }
    let cut: String = trimmed.chars().take(SNIPPET_CHARS).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journals(files: &[(&str, &str, bool)]) -> (std::path::PathBuf, BTreeMap<String, DayJournals>) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("ajd-search-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut days: BTreeMap<String, DayJournals> = BTreeMap::new();
        for (index, (day, text, checkpoint)) in files.iter().enumerate() {
            let path = root.join(format!("{day}-{index}.md"));
            std::fs::write(&path, text).unwrap();
            let entry = days.entry((*day).to_owned()).or_default();
            if *checkpoint {
                entry.checkpoint = Some(path);
            } else {
                entry.final_journal = Some(path);
            }
        }
        (root, days)
    }

    #[test]
    fn a_line_must_hold_every_word_ignoring_case() {
        let (root, days) = journals(&[(
            "2026-11-10",
            "# Day\n- Fixed the Ledger rounding\n- Ledger only\n- rounding only\n",
            false,
        )]);

        let found = search(&days, "ledger ROUNDING");
        std::fs::remove_dir_all(root).unwrap();

        assert_eq!(found.hits.len(), 1);
        assert_eq!(
            (found.hits[0].line, found.hits[0].text.as_str()),
            (2, "Fixed the Ledger rounding")
        );
        assert_eq!(found.days_searched, 1);
    }

    #[test]
    fn newest_day_first_and_a_final_beats_its_checkpoint() {
        let (root, days) = journals(&[
            ("2026-11-09", "retry loop\n", false),
            ("2026-11-10", "retry in the final\n", false),
            ("2026-11-10", "retry in the checkpoint\n", true),
            ("2026-11-11", "retry only in a checkpoint\n", true),
        ]);

        let found = search(&days, "retry");
        std::fs::remove_dir_all(root).unwrap();

        let seen: Vec<_> = found.hits.iter().map(|h| (h.day.as_str(), h.checkpoint)).collect();
        assert_eq!(
            seen,
            [("2026-11-11", true), ("2026-11-10", false), ("2026-11-09", false)]
        );
        assert!(found.hits[1].text.contains("final"));
    }

    #[test]
    fn a_blank_query_finds_nothing_and_reads_nothing() {
        let (root, days) = journals(&[("2026-11-10", "text\n", false)]);

        let found = search(&days, "   ");
        std::fs::remove_dir_all(root).unwrap();

        assert_eq!(found, SearchResult::default());
    }

    #[test]
    fn one_day_cannot_fill_the_list() {
        let lines = "match\n".repeat(20);
        let (root, days) = journals(&[("2026-11-10", lines.as_str(), false)]);

        let found = search(&days, "match");
        std::fs::remove_dir_all(root).unwrap();

        assert_eq!(found.hits.len(), MAX_PER_DAY);
        assert!(found.truncated);
    }

    #[test]
    fn an_unreadable_file_is_counted_not_hidden() {
        let mut days = BTreeMap::new();
        days.insert(
            "2026-11-10".to_owned(),
            DayJournals {
                final_journal: Some("does-not-exist.md".into()),
                checkpoint: None,
            },
        );

        let found = search(&days, "x");

        assert_eq!((found.unreadable, found.days_searched), (1, 0));
    }

    #[test]
    fn a_long_line_is_cut_with_an_ellipsis() {
        let (root, days) = journals(&[("2026-11-10", &format!("## {}\n", "word ".repeat(80)), false)]);

        let found = search(&days, "word");
        std::fs::remove_dir_all(root).unwrap();

        assert!(found.hits[0].text.ends_with('…'));
        assert!(!found.hits[0].text.starts_with('#'));
    }
}
