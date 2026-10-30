//! Rebundling a diff into a narrative: named chapters of hunks, in the
//! order that makes the change comprehensible rather than the order the
//! filesystem imposes.
//!
//! # Why coverage is the whole point
//!
//! A narrative is the *agent's claim* about a change; the file view is the
//! *ground truth*. Those are not the same kind of thing, and conflating
//! them is dangerous in one specific way: if nine hunks are grouped into
//! three tidy chapters and the tenth is quietly left out, the reviewer
//! reads a coherent story and approves a change they never saw. Nothing
//! about that looks like a failure from their side.
//!
//! So this module's real job is not grouping — it is *auditing* the
//! grouping. [`coverage`] reports what a bundle claims, what it misses,
//! and what it claims that does not exist, so the UI can show all three
//! rather than only the tidy story.

use serde::{Deserialize, Serialize};

use crate::diff::DiffFile;

/// A reference to one hunk of one file.
///
/// A hunk header (`@@ -1,2 +3,4 @@ ...`) is unique within a file for a
/// given revision and is already parsed, so it serves as the identifier
/// without inventing one and without depending on positional indexes that
/// shift the moment anything is reordered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HunkRef {
    pub file: String,
    pub header: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub hunks: Vec<HunkRef>,
}

/// What a bundle claims, measured against the diff actually under review.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Coverage {
    /// Hunks in the diff that some chapter claims.
    pub claimed: usize,
    /// Every hunk in the diff.
    pub total: usize,
    /// In the diff, claimed by nobody. These must be shown, not hidden:
    /// an omitted hunk is the failure this module exists to prevent.
    pub unbundled: Vec<HunkRef>,
    /// Claimed by a chapter but absent from the diff — the narrative is
    /// describing a different revision, so nothing in it can be trusted
    /// to line up.
    pub phantom: Vec<HunkRef>,
}

impl Coverage {
    /// Whether the narrative accounts for the diff exactly.
    pub fn is_complete(&self) -> bool {
        self.unbundled.is_empty() && self.phantom.is_empty()
    }
}

/// Every hunk in `files`, in diff order.
pub fn hunk_refs(files: &[DiffFile]) -> Vec<HunkRef> {
    files
        .iter()
        .flat_map(|f| {
            // A deleted file has no new_path; fall back so its hunks are
            // still addressable rather than silently unreferenceable.
            let path = f.new_path.clone().or_else(|| f.old_path.clone()).unwrap_or_default();
            f.hunks.iter().map(move |h| HunkRef { file: path.clone(), header: h.header.clone() })
        })
        .collect()
}

/// Audit `chapters` against the diff.
pub fn coverage(files: &[DiffFile], chapters: &[Chapter]) -> Coverage {
    let actual = hunk_refs(files);
    let claimed: Vec<&HunkRef> = chapters.iter().flat_map(|c| c.hunks.iter()).collect();

    let unbundled: Vec<HunkRef> =
        actual.iter().filter(|h| !claimed.contains(h)).cloned().collect();
    let phantom: Vec<HunkRef> = claimed
        .iter()
        .filter(|h| !actual.contains(h))
        .map(|h| (*h).clone())
        // A chapter listing the same missing hunk twice is one problem,
        // not two.
        .fold(Vec::new(), |mut acc, h| {
            if !acc.contains(&h) {
                acc.push(h);
            }
            acc
        });

    Coverage {
        claimed: actual.len() - unbundled.len(),
        total: actual.len(),
        unbundled,
        phantom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified_diff;

    const DIFF: &str = "\
diff --git a/a.rs b/a.rs
index 1..2 100644
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,3 @@
 one
+two
 three
@@ -10,2 +11,3 @@
 ten
+eleven
 twelve
diff --git a/b.rs b/b.rs
index 3..4 100644
--- a/b.rs
+++ b/b.rs
@@ -5,2 +5,3 @@
 five
+six
 seven
";

    fn files() -> Vec<DiffFile> {
        parse_unified_diff(DIFF)
    }

    fn href(file: &str, header: &str) -> HunkRef {
        HunkRef { file: file.into(), header: header.into() }
    }

    #[test]
    fn every_hunk_in_the_diff_is_addressable() {
        let refs = hunk_refs(&files());
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].file, "a.rs");
        assert_eq!(refs[2].file, "b.rs");
    }

    #[test]
    fn a_bundle_covering_everything_reports_complete() {
        let f = files();
        let all = hunk_refs(&f);
        let chapters = vec![
            Chapter { title: "first".into(), summary: String::new(), hunks: vec![all[0].clone(), all[1].clone()] },
            Chapter { title: "second".into(), summary: String::new(), hunks: vec![all[2].clone()] },
        ];
        let c = coverage(&f, &chapters);
        assert_eq!((c.claimed, c.total), (3, 3));
        assert!(c.is_complete());
    }

    #[test]
    fn an_omitted_hunk_is_reported_rather_than_disappearing() {
        // The failure this module exists to prevent: a tidy narrative that
        // quietly leaves something out, so the reviewer approves a change
        // they were never shown.
        let f = files();
        let all = hunk_refs(&f);
        let chapters = vec![Chapter { title: "only some".into(), summary: String::new(), hunks: vec![all[0].clone()] }];
        let c = coverage(&f, &chapters);
        assert_eq!((c.claimed, c.total), (1, 3));
        assert!(!c.is_complete());
        assert_eq!(c.unbundled, vec![all[1].clone(), all[2].clone()]);
    }

    #[test]
    fn an_empty_bundle_leaves_the_entire_diff_unbundled() {
        // Not "complete because nothing was claimed".
        let f = files();
        let c = coverage(&f, &[]);
        assert_eq!((c.claimed, c.total), (0, 3));
        assert_eq!(c.unbundled.len(), 3);
        assert!(!c.is_complete());
    }

    #[test]
    fn a_chapter_naming_a_hunk_that_is_not_there_is_flagged_as_phantom() {
        // Means the narrative describes a different revision, so none of
        // its groupings can be trusted to line up with what is on screen.
        let f = files();
        let all = hunk_refs(&f);
        let mut hunks = all.clone();
        hunks.push(href("a.rs", "@@ -99,1 +99,1 @@"));
        let chapters = vec![Chapter { title: "stale".into(), summary: String::new(), hunks }];
        let c = coverage(&f, &chapters);
        assert_eq!(c.claimed, 3, "the real hunks still count as claimed");
        assert_eq!(c.phantom, vec![href("a.rs", "@@ -99,1 +99,1 @@")]);
        assert!(!c.is_complete());
    }

    #[test]
    fn the_same_missing_hunk_named_twice_is_one_problem() {
        let f = files();
        let ghost = href("b.rs", "@@ -50,1 +50,1 @@");
        let chapters = vec![
            Chapter { title: "one".into(), summary: String::new(), hunks: vec![ghost.clone()] },
            Chapter { title: "two".into(), summary: String::new(), hunks: vec![ghost.clone()] },
        ];
        assert_eq!(coverage(&f, &chapters).phantom, vec![ghost]);
    }

    #[test]
    fn a_hunk_claimed_by_two_chapters_is_covered_not_double_counted() {
        // Overlapping chapters are a reasonable way to explain a change
        // that genuinely serves two purposes; the count must stay honest.
        let f = files();
        let all = hunk_refs(&f);
        let chapters = vec![
            Chapter { title: "one".into(), summary: String::new(), hunks: all.clone() },
            Chapter { title: "two".into(), summary: String::new(), hunks: vec![all[0].clone()] },
        ];
        let c = coverage(&f, &chapters);
        assert_eq!((c.claimed, c.total), (3, 3));
        assert!(c.is_complete());
    }
}
