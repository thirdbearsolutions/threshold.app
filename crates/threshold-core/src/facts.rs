//! The diff's facts, shown at the foot of a review and kept with each
//! verdict: files and lines changed, which files are deleted, and which
//! look like tests.

use crate::diff::{DiffFile, FileStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffFacts {
    pub files_changed: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub has_deletions: bool,
    pub deleted_files: Vec<String>,
    pub touches_tests: bool,
    pub test_files: Vec<String>,
}

/// A path "looks like" a test file if any component of it is exactly
/// `test`/`tests`, or the file stem starts/ends with `test`/`_test` — the
/// common conventions across the languages this repo's other tools
/// already care about (Rust `tests/`, JS `__tests__`/`*.test.ts`, Python
/// `test_*.py`), not a claim of running the test suite to check coverage.
fn looks_like_test_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let components: Vec<&str> = lower.split('/').collect();
    if components.iter().any(|c| *c == "test" || *c == "tests" || *c == "__tests__") {
        return true;
    }
    let Some(filename) = components.last() else { return false };
    let parts: Vec<&str> = filename.split('.').collect();
    if parts.iter().any(|p| *p == "test" || *p == "tests") {
        return true;
    }
    let stem = parts.first().copied().unwrap_or(filename);
    stem.starts_with("test_") || stem.ends_with("_test")
}

pub fn summarize(files: &[DiffFile]) -> DiffFacts {
    let mut lines_added = 0;
    let mut lines_removed = 0;
    let mut deleted_files = Vec::new();
    let mut test_files = Vec::new();

    for file in files {
        lines_added += file.additions;
        lines_removed += file.deletions;
        if file.status == FileStatus::Deleted {
            deleted_files.push(file.display_path().to_string());
        }
        if looks_like_test_path(file.display_path()) {
            test_files.push(file.display_path().to_string());
        }
    }

    DiffFacts {
        files_changed: files.len(),
        lines_added,
        lines_removed,
        has_deletions: !deleted_files.is_empty(),
        touches_tests: !test_files.is_empty(),
        deleted_files,
        test_files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified_diff;

    fn files(diff: &str) -> Vec<DiffFile> {
        parse_unified_diff(diff)
    }

    #[test]
    fn counts_files_and_lines_across_a_multi_file_diff() {
        let diff = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n-old1\n-old2\n+new1\ndiff --git a/b.rs b/b.rs\nindex 3..4 100644\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1,2 @@\n+added\n unchanged\n";
        let summary = summarize(&files(diff));
        assert_eq!(summary.files_changed, 2);
        assert_eq!(summary.lines_added, 2);
        assert_eq!(summary.lines_removed, 2);
    }

    #[test]
    fn flags_a_deleted_file() {
        let diff = "diff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\nindex 1..0\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n";
        let summary = summarize(&files(diff));
        assert!(summary.has_deletions);
        assert_eq!(summary.deleted_files, vec!["gone.rs"]);
    }

    #[test]
    fn a_pure_modification_has_no_deletions_flag() {
        let diff = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let summary = summarize(&files(diff));
        assert!(!summary.has_deletions);
        assert!(summary.deleted_files.is_empty());
    }

    #[test]
    fn recognises_common_test_path_conventions() {
        assert!(looks_like_test_path("tests/it_works.rs"));
        assert!(looks_like_test_path("src/foo/test_bar.py"));
        assert!(looks_like_test_path("src/foo.test.ts"));
        assert!(looks_like_test_path("__tests__/foo.js"));
        assert!(!looks_like_test_path("src/latest_report.rs"));
        assert!(!looks_like_test_path("src/contest_winner.rs"));
    }

    #[test]
    fn flags_touches_tests_when_a_changed_file_matches() {
        let diff = "diff --git a/tests/it_works.rs b/tests/it_works.rs\nindex 1..2 100644\n--- a/tests/it_works.rs\n+++ b/tests/it_works.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let summary = summarize(&files(diff));
        assert!(summary.touches_tests);
        assert_eq!(summary.test_files, vec!["tests/it_works.rs"]);
    }

    #[test]
    fn an_empty_change_set_summarizes_to_all_zero_and_false() {
        let summary = summarize(&[]);
        assert_eq!(summary.files_changed, 0);
        assert!(!summary.has_deletions);
        assert!(!summary.touches_tests);
    }
}
