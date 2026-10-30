//! The review inbox: a directory of session logs that agents push into.
//!
//! The handoff is a file appearing in a directory. No process to launch,
//! no port, no flag, no filename to agree on in advance — an agent writes
//! `~/.threshold/reviews/<anything>.jsonl` and Threshold picks it up.
//!
//! That also removes a whole class of bug rather than mitigating it: when
//! every review is its own file, two concurrent reviews cannot read each
//! other's verdict, which was only ever prevented by convention before.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::session::{self, Body};

/// Where agents push reviews. Under `$HOME` rather than an app-support
/// directory so it is somewhere a person can `ls`, and somewhere an agent
/// can write without knowing anything about macOS bundle layout.
pub const INBOX_SUBPATH: &str = ".threshold/reviews";

pub fn inbox_dir(home: &Path) -> PathBuf {
    home.join(INBOX_SUBPATH)
}

/// One review, as summarised for the list.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ReviewSummary {
    pub path: PathBuf,
    /// From the brief. Falls back to the filename when a log has no brief
    /// yet — an agent may create the file before it has written anything.
    pub title: String,
    pub repo: Option<String>,
    pub revision: Option<String>,
    /// True once a verdict has been written on the current revision. One
    /// from before a `revised` event does not count: see
    /// [`session::current_verdict`].
    pub decided: bool,
    /// Timestamp of the newest event, or 0 for an empty log.
    pub last_at_ms: u64,
    pub events: usize,
    /// Events written by the agent or the human, leaving out Threshold's
    /// own.
    ///
    /// What "this review was updated" should be measured by. Threshold
    /// pins and diagnoses a review in the background, seconds after it
    /// arrives, and those lines are addressed to the agent; counting them
    /// would announce every new review to the reviewer twice.
    pub spoken: usize,
    /// Put away by the reviewer without a verdict. See [`set_archived`].
    pub archived: bool,
}

/// The file beside the reviews that records which are archived. Not a
/// `.jsonl`, so it is never mistaken for a review.
const ARCHIVE_FILE: &str = "archive.json";

/// Archived reviews, by file name, with how many spoken events each had
/// when it was put away.
type Archive = BTreeMap<String, usize>;

fn load_archive(dir: &Path) -> Archive {
    std::fs::read_to_string(dir.join(ARCHIVE_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Archive a review, or bring it back, without deciding it.
///
/// Recorded beside the reviews rather than in the review's own log. The
/// log is the conversation, and putting a review away is not something
/// said to the agent: an event there would wake a session tailing the file
/// to tell it nothing it can act on, and it is not a verdict, so an agent
/// still waiting on one keeps waiting.
///
/// Archiving remembers how much had been said. When the agent or the
/// reviewer says more — a reply, a revision — the review is back in the
/// inbox without anyone unarchiving it, the way a mail thread returns when
/// someone answers it. Something new in a review is never filed away
/// unseen.
pub fn set_archived(dir: &Path, review: &Path, archived: bool) -> std::io::Result<()> {
    let Some(name) = review.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(());
    };
    let mut archive = load_archive(dir);
    if archived {
        let spoken = session::read(review).iter().filter(|e| e.from != session::Author::Threshold).count();
        archive.insert(name, spoken);
    } else {
        archive.remove(&name);
    }
    std::fs::create_dir_all(dir)?;
    // Written whole to a temporary file and renamed into place, so a crash
    // mid-write cannot leave a half-written archive that unarchives
    // everything.
    let tmp = dir.join(format!("{ARCHIVE_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(&archive).unwrap_or_default())?;
    std::fs::rename(tmp, dir.join(ARCHIVE_FILE))
}

fn summarise(path: &Path, archive: &Archive) -> ReviewSummary {
    let events = session::read(path);
    let brief = events.iter().find_map(|e| match &e.body {
        Body::Brief { title, repo, .. } => Some((title.clone(), repo.clone())),
        _ => None,
    });
    // So a review that has been revised lists its current state rather
    // than the one it opened on.
    let revision = session::current_revision(&events);
    let (title, repo) = brief.unwrap_or_else(|| {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        (name, None)
    });
    let spoken = events.iter().filter(|e| e.from != session::Author::Threshold).count();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    ReviewSummary {
        path: path.to_path_buf(),
        title,
        repo,
        revision,
        decided: session::current_verdict(&events).is_some(),
        last_at_ms: events.iter().map(|e| e.at_ms).max().unwrap_or(0),
        events: events.len(),
        spoken,
        // Still archived only while nothing more has been said.
        archived: archive.get(&name).is_some_and(|&at| spoken <= at),
    }
}

/// Every review in `dir`, newest first, undecided before decided.
///
/// The ordering is the point of the list: what is waiting on you comes
/// first, and history stays reachable underneath rather than being
/// deleted. A missing directory is an empty inbox, not an error.
pub fn list(dir: &Path) -> Vec<ReviewSummary> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let archive = load_archive(dir);
    let mut out: Vec<ReviewSummary> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .map(|p| summarise(&p, &archive))
        .collect();
    out.sort_by(|a, b| {
        a.decided
            .cmp(&b.decided)
            .then(b.last_at_ms.cmp(&a.last_at_ms))
            .then(a.path.cmp(&b.path))
    });
    out
}

/// Why a name from outside the app does not name a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewNameError {
    /// Empty, not a bare file name, hidden, or not a `.jsonl`.
    NotAReviewName,
    /// Well-formed, but no such review, or it resolves outside the inbox
    /// (a symlink out of it). Deliberately one answer for both: a caller
    /// probing the filesystem should learn nothing about what exists.
    NotFound,
}

impl std::fmt::Display for ReviewNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAReviewName => f.write_str("not a review name"),
            Self::NotFound => f.write_str("no such review"),
        }
    }
}

/// The review in `dir` called `name`, where `name` arrived over the
/// network.
///
/// This is the one place a network bug becomes command execution: a
/// review's brief names a repo, and loading its diff runs `git` there. So
/// a caller names a review by its file name in the inbox and nothing else
/// — never a path, never a repo — and the answer must still be inside the
/// inbox once symlinks are followed.
pub fn resolve_name(dir: &Path, name: &str) -> Result<PathBuf, ReviewNameError> {
    let bare = !name.is_empty()
        && !name.starts_with('.')
        && name.ends_with(".jsonl")
        && !name.contains(['/', '\\', '\0'])
        && Path::new(name).file_name() == Some(std::ffi::OsStr::new(name));
    if !bare {
        return Err(ReviewNameError::NotAReviewName);
    }
    let inbox = std::fs::canonicalize(dir).map_err(|_| ReviewNameError::NotFound)?;
    let full = std::fs::canonicalize(inbox.join(name)).map_err(|_| ReviewNameError::NotFound)?;
    if full.parent() != Some(inbox.as_path()) || !full.is_file() {
        return Err(ReviewNameError::NotFound);
    }
    // The inbox's own spelling of the path, not the canonical one, so it
    // matches what `list` reports and what the archive is keyed by.
    Ok(dir.join(name))
}

/// How many reviews are waiting on a decision. Archived ones are not: the
/// reviewer has put them away, and a badge that cannot be cleared without
/// a verdict nobody wants to give is a badge that gets ignored.
pub fn waiting(dir: &Path) -> usize {
    list(dir).iter().filter(|r| !r.decided && !r.archived).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Verdict;
    use crate::session::{append, Author};

    fn brief(title: &str, repo: &str) -> Body {
        Body::Brief {
            title: title.into(),
            summary: "s".into(),
            implications: vec![],
            repo: Some(repo.into()),
        }
    }

    #[test]
    fn a_missing_inbox_is_empty_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(&dir.path().join("nope")).is_empty());
        assert_eq!(waiting(&dir.path().join("nope")), 0);
    }

    #[test]
    fn a_review_reports_its_title_repo_and_revision_from_the_log() {
        // The whole point of a self-describing log: rendered from the file
        // alone, with no command line to supply context.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("one.jsonl");
        append(&p, Author::Agent, Some("main...HEAD".into()), brief("Drop the column", "/repo/a")).unwrap();

        let r = &list(dir.path())[0];
        assert_eq!(r.title, "Drop the column");
        assert_eq!(r.repo.as_deref(), Some("/repo/a"));
        assert_eq!(r.revision.as_deref(), Some("main...HEAD"));
        assert!(!r.decided);
    }

    #[test]
    fn waiting_reviews_sort_above_decided_ones() {
        let dir = tempfile::tempdir().unwrap();
        let done = dir.path().join("done.jsonl");
        append(&done, Author::Agent, None, brief("finished", "/r")).unwrap();
        append(&done, Author::Human, None, Body::Verdict { verdict: Verdict::Approved, reason: None, digest: None }).unwrap();

        let open = dir.path().join("open.jsonl");
        append(&open, Author::Agent, None, brief("still waiting", "/r")).unwrap();

        let all = list(dir.path());
        assert_eq!(all[0].title, "still waiting", "what needs you comes first");
        assert!(all[1].decided, "history stays reachable underneath");
        assert_eq!(waiting(dir.path()), 1);
    }

    #[test]
    fn a_review_revised_after_its_verdict_is_waiting_again() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("revised.jsonl");
        append(&p, Author::Agent, Some("main...HEAD".into()), brief("Drop the column", "/r")).unwrap();
        append(&p, Author::Human, None, Body::Verdict { verdict: Verdict::Approved, reason: None, digest: Some("d1".into()) }).unwrap();
        assert_eq!(waiting(dir.path()), 0);

        append(&p, Author::Agent, None, Body::Revised { to: "main...HEAD".into(), note: Some("fixed the down path".into()) }).unwrap();
        assert!(!list(dir.path())[0].decided, "the approval was of commits no longer handed over");
        assert_eq!(waiting(dir.path()), 1);
    }

    #[test]
    fn the_current_revision_wins_after_a_revision() {
        // A revised review must list where it is now, not where it opened.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        append(&p, Author::Agent, Some("v1".into()), brief("t", "/r")).unwrap();
        append(&p, Author::Agent, Some("v2".into()), Body::Revised { to: "v2".into(), note: None }).unwrap();
        assert_eq!(list(dir.path())[0].revision.as_deref(), Some("v2"));
    }

    #[test]
    fn threshold_writing_into_a_review_is_not_the_review_being_spoken_in() {
        // Background checks append to reviews nobody has opened; the
        // reviewer should hear about the agent, not about Threshold.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        append(&p, Author::Agent, None, brief("t", "/r")).unwrap();
        crate::session::diagnose(&p, None, "review_unpinned", "m".into(), vec![]).unwrap();
        let r = &list(dir.path())[0];
        assert_eq!((r.events, r.spoken), (2, 1));
    }

    #[test]
    fn an_archived_review_is_listed_as_archived_and_stops_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("stale.jsonl");
        append(&p, Author::Agent, None, brief("stale", "/r")).unwrap();
        assert_eq!(waiting(dir.path()), 1);

        set_archived(dir.path(), &p, true).unwrap();
        let r = &list(dir.path())[0];
        assert!(r.archived);
        assert!(!r.decided, "archiving is not a verdict");
        assert_eq!(waiting(dir.path()), 0);
        assert!(session::verdict(&p).is_none(), "and nothing was written into the conversation");
        assert_eq!(session::read(&p).len(), 1);

        set_archived(dir.path(), &p, false).unwrap();
        assert!(!list(dir.path())[0].archived);
        assert_eq!(waiting(dir.path()), 1);
    }

    #[test]
    fn something_new_said_in_an_archived_review_brings_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        append(&p, Author::Agent, None, brief("t", "/r")).unwrap();
        set_archived(dir.path(), &p, true).unwrap();

        // Threshold's own bookkeeping is not news.
        session::diagnose(&p, None, "review_unpinned", "m".into(), vec![]).unwrap();
        assert!(list(dir.path())[0].archived);

        // Claude answering is.
        append(&p, Author::Agent, None, Body::Reply { text: "fixed".into(), in_reply_to: None }).unwrap();
        assert!(!list(dir.path())[0].archived);
    }

    #[test]
    fn the_archive_file_is_not_a_review_and_a_corrupt_one_archives_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        append(&p, Author::Agent, None, brief("t", "/r")).unwrap();
        set_archived(dir.path(), &p, true).unwrap();
        assert_eq!(list(dir.path()).len(), 1, "archive.json is not listed");
        std::fs::write(dir.path().join("archive.json"), "{ not json").unwrap();
        assert!(!list(dir.path())[0].archived);
    }

    #[test]
    fn a_log_with_no_brief_yet_falls_back_to_its_filename() {
        // An agent may create the file before writing anything; the review
        // must still be listed rather than silently withheld.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("half-written-review.jsonl"), "").unwrap();
        let r = &list(dir.path())[0];
        assert_eq!(r.title, "half-written-review");
        assert_eq!(r.events, 0);
        assert!(!r.decided);
    }

    #[test]
    fn a_review_is_named_by_its_file_name_in_the_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("r.jsonl");
        append(&p, Author::Agent, None, brief("t", "/r")).unwrap();
        assert_eq!(resolve_name(dir.path(), "r.jsonl"), Ok(p));
    }

    #[test]
    fn a_name_from_the_network_cannot_reach_outside_the_inbox() {
        // Reaching a file outside the inbox would let a request choose the
        // repo `git` runs in, so every escape is refused.
        let root = tempfile::tempdir().unwrap();
        let inbox = root.path().join("reviews");
        std::fs::create_dir(&inbox).unwrap();
        let outside = root.path().join("outside.jsonl");
        append(&outside, Author::Agent, None, brief("not yours", "/etc")).unwrap();
        std::fs::create_dir(inbox.join("sub")).unwrap();
        append(&inbox.join("sub/nested.jsonl"), Author::Agent, None, brief("n", "/r")).unwrap();
        std::fs::write(inbox.join("notes.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&outside, inbox.join("link.jsonl")).unwrap();
        assert!(inbox.join("link.jsonl").exists(), "the symlink must point at something, or this proves nothing");

        for name in ["../outside.jsonl", "sub/nested.jsonl", "..", "", ".hidden.jsonl", "notes.txt", "a\\b.jsonl", "nul\0.jsonl", "/etc/passwd", outside.to_str().unwrap()] {
            assert_eq!(resolve_name(&inbox, name), Err(ReviewNameError::NotAReviewName), "{name:?}");
        }
        assert_eq!(resolve_name(&inbox, "link.jsonl"), Err(ReviewNameError::NotFound), "a symlink out of the inbox");
        assert_eq!(resolve_name(&inbox, "missing.jsonl"), Err(ReviewNameError::NotFound));
    }

    #[test]
    fn non_jsonl_files_in_the_directory_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();
        std::fs::write(dir.path().join(".DS_Store"), "junk").unwrap();
        append(&dir.path().join("real.jsonl"), Author::Agent, None, brief("real", "/r")).unwrap();
        let all = list(dir.path());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "real");
    }
}
