//! Pinning a review to something that cannot move under the reviewer.
//!
//! # The problem this exists to solve
//!
//! Threshold renders a review by running `git diff <spec>` at the moment
//! the reviewer looks at it. A spec like `main...HEAD` is not a thing —
//! it is a *question*, and both of its endpoints move. The agent commits
//! again, or someone advances `main`, and the same question returns a
//! different answer. Nothing in the review notices: the narrative still
//! reads as authoritative, line annotations re-anchor to whatever now
//! occupies that line number, and the verdict is recorded against the
//! spec string, so the permanent log says a human approved `main...HEAD`
//! without saying which `main...HEAD`.
//!
//! A pin resolves the spec once, to the two commit ids git would actually
//! compare, and reviews *those* from then on. Two object ids name exactly
//! one diff forever, so the reviewer and the agent are provably looking
//! at the same change.
//!
//! # What cannot be pinned
//!
//! Not every spec names two commits. `git diff HEAD~3` compares a commit
//! against the **working tree**, and so does a bare `git diff`. Those are
//! live by construction and no amount of resolving fixes them — the
//! answer is to say so loudly rather than to pretend. [`parse_range`]
//! exists to tell those two cases apart, and deliberately does no more
//! than that: git owns the revision grammar, and this only needs to know
//! whether a spec has two sides.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How git will read a revision spec: as two commits, or as one commit
/// against whatever is on disk right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    /// `A...B` — compare the merge base of `A` and `B` against `B`.
    ///
    /// The shape the review skill recommends, and the one worth getting
    /// right: it is what shows "the work on this branch" rather than
    /// every change that landed on the base meanwhile.
    MergeBase { left: String, right: String },
    /// `A..B`, which git treats exactly as `git diff A B`.
    Endpoints { left: String, right: String },
    /// One revision, which git compares against the working tree. Cannot
    /// be pinned: half of it is not a commit.
    WorkingTree { rev: String },
}

/// An omitted side of a range means `HEAD`, as it does everywhere in git
/// (`main..` is `main..HEAD`).
fn side_or_head(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        "HEAD".to_string()
    } else {
        s.to_string()
    }
}

/// Read the shape of a revision spec.
///
/// `...` is checked before `..` because the former contains the latter,
/// and mistaking a symmetric difference for a plain range silently
/// widens the diff to include everything that landed on the base branch
/// since — a reviewer would be shown other people's commits as though
/// they were the agent's.
pub fn parse_range(spec: &str) -> Range {
    let spec = spec.trim();
    if let Some((l, r)) = spec.split_once("...") {
        return Range::MergeBase { left: side_or_head(l), right: side_or_head(r) };
    }
    if let Some((l, r)) = spec.split_once("..") {
        return Range::Endpoints { left: side_or_head(l), right: side_or_head(r) };
    }
    Range::WorkingTree { rev: spec.to_string() }
}

/// The two commits a review is pinned to.
///
/// Full object ids, never abbreviations: an abbreviation is a prefix
/// search whose answer can change as the repository grows objects, which
/// is the exact class of problem a pin is here to end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub base: String,
    pub head: String,
}

/// Whether `s` is a full object id, checked before it is handed back to
/// `git` as a revision.
///
/// Everything pinned comes from `git rev-parse`, so this should always
/// hold; it is checked anyway because a pin is the one value in the
/// system that later gets trusted *instead of* re-deriving, and a pin
/// that is not an object id would quietly reintroduce a movable
/// reference under a name that promises it cannot move.
pub fn is_object_id(s: &str) -> bool {
    let n = s.len();
    (n == 40 || n == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Pin {
    pub fn is_valid(&self) -> bool {
        is_object_id(&self.base) && is_object_id(&self.head)
    }
}

/// A content fingerprint of a diff, hex-encoded SHA-256.
///
/// The pin stops the diff moving; this proves it did not. They do
/// different jobs and a review wants both: a pinned review carries a
/// digest so a verdict names content rather than a spec, and an
/// *unpinnable* one carries a digest because detecting drift after the
/// fact is the only thing left to do about it.
pub fn digest(diff_text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(diff_text.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symmetric_difference_is_read_as_a_merge_base_comparison() {
        assert_eq!(
            parse_range("main...HEAD"),
            Range::MergeBase { left: "main".into(), right: "HEAD".into() }
        );
    }

    #[test]
    fn two_dots_are_not_mistaken_for_three() {
        // The dangerous confusion: read as a merge base, `main..HEAD`
        // would show a narrower diff; read the other way, `main...HEAD`
        // would show every commit that landed on main meanwhile.
        assert_eq!(
            parse_range("main..HEAD"),
            Range::Endpoints { left: "main".into(), right: "HEAD".into() }
        );
    }

    #[test]
    fn an_omitted_side_means_head_as_it_does_in_git() {
        assert_eq!(
            parse_range("main..."),
            Range::MergeBase { left: "main".into(), right: "HEAD".into() }
        );
        assert_eq!(
            parse_range("..origin/main"),
            Range::Endpoints { left: "HEAD".into(), right: "origin/main".into() }
        );
    }

    #[test]
    fn a_single_revision_is_a_working_tree_comparison_and_so_unpinnable() {
        // The case that looks pinned and is not: `git diff HEAD~3`
        // compares that commit against whatever is on disk right now.
        for spec in ["HEAD", "HEAD~3", "v1.2^{commit}", "3d73c7b", "release/2.0"] {
            assert_eq!(
                parse_range(spec),
                Range::WorkingTree { rev: spec.to_string() },
                "{spec} names one commit, so the other side is the working tree"
            );
        }
    }

    #[test]
    fn surrounding_whitespace_does_not_change_the_shape() {
        assert_eq!(
            parse_range("  main...HEAD  "),
            Range::MergeBase { left: "main".into(), right: "HEAD".into() }
        );
    }

    #[test]
    fn only_a_full_object_id_counts_as_one() {
        assert!(is_object_id("46564a3f0e1c2b3a4d5e6f708192a3b4c5d6e7f8"));
        // sha256 repositories exist; their ids are 64 hex characters.
        assert!(is_object_id(&"a".repeat(64)));
        // An abbreviation is a prefix search, not an identity.
        assert!(!is_object_id("3d73c7b"));
        assert!(!is_object_id("main"));
        assert!(!is_object_id("HEAD"));
        // Right length, wrong alphabet.
        assert!(!is_object_id(&"z".repeat(40)));
    }

    #[test]
    fn a_pin_is_valid_only_when_both_sides_are_object_ids() {
        let oid = "46564a3f0e1c2b3a4d5e6f708192a3b4c5d6e7f8";
        assert!(Pin { base: oid.into(), head: oid.into() }.is_valid());
        assert!(!Pin { base: "main".into(), head: oid.into() }.is_valid());
        assert!(!Pin { base: oid.into(), head: "HEAD".into() }.is_valid());
    }

    #[test]
    fn a_digest_is_stable_and_distinguishes_diffs() {
        let a = "@@ -1,2 +1,3 @@\n one\n+two\n";
        let b = "@@ -1,2 +1,3 @@\n one\n+three\n";
        assert_eq!(digest(a), digest(a));
        assert_ne!(digest(a), digest(b));
        assert_eq!(digest(a).len(), 64, "hex-encoded SHA-256");
    }

    #[test]
    fn a_one_character_change_changes_the_digest() {
        // The whole point: an approval quotes a digest, so a diff that
        // shifted by a single character must not still match it.
        assert_ne!(digest("conn.execute(\"DROP TABLE x\")"), digest("conn.execute(\"DROP TABLE y\")"));
    }
}
