//! The review session log: an append-only JSONL conversation between the
//! agent that proposed a change and the human reviewing it.
//!
//! One file, both sides appending, both sides watching. Append-only is the
//! whole design: there is no shared mutable state to lock, a reader that
//! knows the last `seq` it saw can resume without re-reading, and a writer
//! never has to rewrite what someone else may have just added. A verdict
//! is simply the terminal event kind rather than a separate mechanism.
//!
//! Each line is one [`Event`]. A line that fails to parse is skipped
//! rather than failing the read: a half-written trailing line is a normal
//! thing to observe while the other side is mid-append, and discarding the
//! entire conversation because of it would be far worse than ignoring one
//! event that is about to be re-read a moment later.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

use crate::bundle::Chapter;
use crate::log::Verdict;

/// Why Claude marked a line, set when the review is pushed.
///
/// Three, deliberately: a fourth would be decoration, and a reviewer
/// cannot hold more than a few meanings at a glance. Each has to stay
/// clear of the colours already spoken for — green is approval and only
/// approval, and amber is the human's own voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Flag {
    /// The load-bearing line. Read this one properly.
    Attention,
    /// The risky one, with the reason.
    Caution,
    /// Looks odd, is not. Exists to stop a question being wasted, and is
    /// usually the most useful of the three.
    Context,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mark {
    /// Read it, looks right.
    Ok,
    /// Read it, something is wrong.
    Concern,
    /// Explicitly back to unmarked, so a mark can be taken back without
    /// rewriting an append-only log.
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    Agent,
    Human,
    /// The tool itself, reporting a fact about what it was given.
    ///
    /// Deliberately a third voice. A diagnostic is not the human speaking
    /// — attributing "3 hunks are not in any chapter" to them would be a
    /// lie, and one that changes how an agent responds to it.
    Threshold,
}

/// One entry in the conversation.
///
/// `kind` is the tag, so the JSON stays flat and readable by eye — this
/// file is something a person may well `cat` while debugging a handoff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Body {
    /// The agent's opening context: what this change is and what to watch
    /// for. Without it the human judges a wall of diff cold.
    Brief {
        title: String,
        summary: String,
        #[serde(default)]
        implications: Vec<String>,
        /// Absolute path to the repository this review is about.
        ///
        /// A review pushed into the inbox has no command line to carry
        /// this, so the log has to be self-describing: without it
        /// Threshold cannot render a review it did not itself launch.
        /// Optional so logs written before the inbox existed still load.
        #[serde(default)]
        repo: Option<String>,
    },
    /// A human comment that is deliberately *not* a decision. Attached to
    /// a line when `file` is set, general otherwise.
    Note {
        text: String,
        #[serde(default)]
        file: Option<String>,
        #[serde(default)]
        line: Option<u32>,
        /// The text of the annotated line, captured at the time.
        ///
        /// Line numbers shift the moment the agent revises, so this is
        /// what actually lets the agent find the code again, and lets the
        /// UI tell that an annotation has gone stale.
        #[serde(default)]
        line_content: Option<String>,
        /// Chapter title, when the note is about a whole chapter of the
        /// narrative rather than a line. Some observations belong to a
        /// section and to no single line in it.
        #[serde(default)]
        chapter: Option<String>,
        /// The note or reply this continues.
        ///
        /// Without it a follow-up lands as a second unrelated note on the
        /// same line rather than joining the exchange, so the reviewer can
        /// open a thread but never continue one.
        #[serde(default)]
        in_reply_to: Option<u64>,
        /// Set when Claude marked this line while pushing the review,
        /// rather than answering something.
        ///
        /// A flagged note is a pointer, not a question — but it is still a
        /// note, so the reviewer can reply and it becomes an ordinary
        /// thread without any of the machinery having to change.
        #[serde(default)]
        flag: Option<Flag>,
    },
    /// The agent answering a note.
    Reply {
        text: String,
        #[serde(default)]
        in_reply_to: Option<u64>,
    },
    /// The agent's narrative reading of the diff: chapters of hunks in
    /// the order that explains the change.
    ///
    /// Tied to the revision on the event. A revision invalidates the
    /// bundle, because a narrative describing code that has since changed
    /// is worse than no narrative at all — it reads as authoritative
    /// while pointing at the wrong lines.
    Bundle {
        chapters: Vec<Chapter>,
    },
    /// Threshold resolving the review's revision spec to the two commits
    /// git actually compares, so every later render diffs those instead
    /// of re-asking a question whose answer moves.
    ///
    /// Written by Threshold rather than by the agent, because Threshold
    /// is the party that must not be fooled: a pin the agent supplied
    /// would be one more claim to audit rather than a fact. Recorded in
    /// the log rather than held in memory so a review reopened tomorrow,
    /// in an app that has since restarted, is still the same review.
    Pinned {
        /// The commit the diff starts from.
        base: String,
        /// The commit the diff ends at.
        head: String,
        /// SHA-256 of the diff text at the moment of pinning, so a
        /// verdict can name content rather than a revision spec.
        digest: String,
    },
    /// The agent changed the code; the diff under review moves to `to`.
    ///
    /// Deliberately *not* named `revision`: [`Event`] is serialised with
    /// this body flattened into it, and [`Event::revision`] already owns
    /// that JSON key. Two fields flattening onto one key do not conflict
    /// loudly — the event simply fails to deserialise and, because a
    /// malformed line is skipped by design, silently vanishes from the
    /// conversation.
    Revised {
        to: String,
        #[serde(default)]
        note: Option<String>,
    },
    /// Threshold reporting a problem with what the agent pushed.
    ///
    /// The agent is already tailing this log for notes and a verdict, so
    /// findings arrive on a channel it is watching without inventing a
    /// second one.
    Diagnostic {
        /// Stable machine-readable identifier, e.g. `bundle_incomplete`.
        code: String,
        message: String,
        #[serde(default)]
        detail: Vec<String>,
    },
    /// The reviewer has been through a chapter.
    ///
    /// Progress through the review, deliberately *not* a verdict: marking
    /// every chapter as fine must not silently become approval, because a
    /// reviewer may still reject on something the chapters do not
    /// capture. The global verdict stays the terminal act.
    ChapterMark {
        /// Chapter title. Not an index: a revised bundle reorders
        /// chapters, and an index would silently re-point a mark at
        /// different content — the failure the coverage audit exists to
        /// prevent. A renamed chapter loses its marks, which is right,
        /// since a chapter whose title changed is not obviously the same
        /// chapter.
        chapter: String,
        mark: Mark,
    },
    /// The terminal event.
    Verdict {
        verdict: Verdict,
        #[serde(default)]
        reason: Option<String>,
        /// SHA-256 of the diff the human actually read.
        ///
        /// Without it an approval names a revision spec, and the agent's
        /// "does this verdict match what I handed over?" check compares
        /// one constant string to an identical one and can never fail.
        /// A digest is the only part of a verdict that an agent cannot
        /// satisfy by accident.
        ///
        /// `serde(default)` for verdicts written before pinning existed.
        #[serde(default)]
        digest: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// 1-based, monotonic within a session. Readers resume from the
    /// highest they have seen.
    pub seq: u64,
    pub at_ms: u64,
    pub from: Author,
    /// Which revision was under review when this was written.
    ///
    /// Recorded on every event, not just human ones: it is what makes a
    /// note about since-changed code visibly about the old version rather
    /// than silently re-anchored to whatever now sits at that line.
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(flatten)]
    pub body: Body,
}

impl Event {
    pub fn is_verdict(&self) -> bool {
        matches!(self.body, Body::Verdict { .. })
    }
}

/// Read every well-formed event from the log, in file order.
///
/// A missing file is an empty session, not an error — the agent creates
/// the log by writing its brief, and the UI may open first.
pub fn read(path: &Path) -> Vec<Event> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Event>(l).ok())
        .collect()
}

/// Events after `seq`, for a caller that has already seen up to it.
pub fn read_since(path: &Path, seq: u64) -> Vec<Event> {
    read(path).into_iter().filter(|e| e.seq > seq).collect()
}

/// The highest `seq` in the log, or 0 for an empty one.
pub fn last_seq(path: &Path) -> u64 {
    read(path).iter().map(|e| e.seq).max().unwrap_or(0)
}

/// Every event belonging to the thread rooted at `root`, in order,
/// including the root itself.
///
/// Threads are a chain rather than a flat list: an agent replies to a
/// note, the reviewer replies to that reply, and each link names its
/// parent. Walking transitively keeps the whole exchange together however
/// deep it goes.
pub fn thread(events: &[Event], root: u64) -> Vec<Event> {
    let mut ids = vec![root];
    // Events are append-only and a reply always follows what it answers,
    // so one forward pass is enough to close the chain.
    for e in events {
        let parent = match &e.body {
            Body::Note { in_reply_to, .. } => *in_reply_to,
            Body::Reply { in_reply_to, .. } => *in_reply_to,
            _ => None,
        };
        if let Some(p) = parent {
            if ids.contains(&p) && !ids.contains(&e.seq) {
                ids.push(e.seq);
            }
        }
    }
    events.iter().filter(|e| ids.contains(&e.seq)).cloned().collect()
}

/// The state of one chapter, folded out of the log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChapterState {
    pub mark: Option<Mark>,
    /// Notes anchored to this chapter, whether or not answered.
    pub notes: usize,
    /// Notes with no reply yet — what is still outstanding.
    pub unanswered: usize,
    /// Lines Claude flagged in this chapter's files, by kind. A chapter
    /// with two cautions is worth reading before one with none.
    pub attention: usize,
    pub caution: usize,
    pub context: usize,
}

/// Fold the log into the current state of `chapter`.
///
/// Derived rather than stored: the log is append-only, so the latest mark
/// wins and taking one back is another event rather than a rewrite.
pub fn chapter_state(events: &[Event], chapter: &str) -> ChapterState {
    let mark = events.iter().rev().find_map(|e| match &e.body {
        Body::ChapterMark { chapter: c, mark } if c == chapter => Some(*mark),
        _ => None,
    });
    let notes: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(&e.body, Body::Note { chapter: Some(c), .. } if c == chapter))
        .collect();
    let unanswered = notes
        .iter()
        .filter(|n| thread(events, n.seq).len() < 2)
        .count();
    ChapterState {
        // Clear is the absence of a mark, not a third state to display.
        mark: match mark {
            Some(Mark::Clear) | None => None,
            other => other,
        },
        notes: notes.len(),
        unanswered,
        // Filled by the caller, which knows which files a chapter covers;
        // the log alone cannot say which chapter a line belongs to.
        attention: 0,
        caution: 0,
        context: 0,
    }
}

/// Every line Claude flagged, in log order.
pub fn flagged(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|e| matches!(&e.body, Body::Note { flag: Some(_), .. }))
        .collect()
}

/// Count flags whose threads sit in `files`, for a chapter's tally.
///
/// Counted where [`crate::anchor::resolve`] placed them, not where they
/// were written: a flag carried to a renamed file counts for that file,
/// and an outdated one is not drawn in the gutter, so a table of contents
/// promising it would send the reviewer hunting for a mark that is not
/// there.
pub fn flag_counts(events: &[Event], anchors: &[crate::anchor::Anchor], files: &[String]) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for e in flagged(events) {
        let Body::Note { flag: Some(f), .. } = &e.body else { continue };
        let Some(a) = anchors.iter().find(|a| a.seq == e.seq) else { continue };
        if a.status == crate::anchor::AnchorStatus::Outdated || !files.iter().any(|c| *c == a.file) {
            continue;
        }
        match f {
            Flag::Attention => counts.0 += 1,
            Flag::Caution => counts.1 += 1,
            Flag::Context => counts.2 += 1,
        }
    }
    counts
}

/// Append a diagnostic only if the same code has not already been
/// reported about what the agent has sent most recently.
///
/// The checks run on every load and every poll, so re-appending would
/// fill both the log and the reviewer's screen with the same finding.
/// Keyed on revision as well as code because the same problem against a
/// new revision is genuinely new information. And keyed on what the
/// finding was about: a `revised` event replaces the code, and a
/// `bundle` replaces the narrative a `bundle_*` finding describes, often
/// under the same spec string. The same problem in the replacement is
/// news the agent needs, so a finding from before it does not count.
/// The UI retires findings by the same rule (`diagnostics.ts`).
pub fn diagnose(
    path: &Path,
    revision: Option<String>,
    code: &str,
    message: String,
    detail: Vec<String>,
) -> std::io::Result<Option<Event>> {
    let events = read(path);
    let bundle_finding = code.starts_with("bundle_");
    let since = events
        .iter()
        .filter(|e| {
            e.from == Author::Agent
                && (matches!(e.body, Body::Revised { .. })
                    || (bundle_finding && matches!(e.body, Body::Bundle { .. })))
        })
        .map(|e| e.seq)
        .max()
        .unwrap_or(0);
    let already = events.iter().any(|e| {
        e.seq > since
            && e.revision == revision
            && matches!(&e.body, Body::Diagnostic { code: c, .. } if c == code)
    });
    if already {
        return Ok(None);
    }
    append(
        path,
        Author::Threshold,
        revision,
        Body::Diagnostic { code: code.to_string(), message, detail },
    )
    .map(Some)
}

/// The most recent bundle written against `revision`, if any.
///
/// Scoped to the revision on purpose: an older bundle describing an
/// earlier state of the code is not a fallback, it is a hazard, so it is
/// simply not returned.
pub fn current_bundle(path: &Path, revision: Option<&str>) -> Option<Vec<Chapter>> {
    read(path).into_iter().rev().find_map(|e| match e.body {
        Body::Bundle { chapters } if e.revision.as_deref() == revision => Some(chapters),
        _ => None,
    })
}

/// The pin still in force for `revision`, if Threshold has resolved one.
///
/// Scoped to the revision for the same reason [`current_bundle`] is: a
/// pin taken against an earlier spec names commits that are no longer
/// what the review is about, and using it as a fallback would show the
/// reviewer the wrong change while looking entirely settled.
///
/// A `revised` event retires the pin even when the spec is unchanged,
/// which is the normal case rather than an edge: the review skill tells
/// the agent to amend and re-announce `main...HEAD`, so the spec is
/// identical on both sides of a revision and recency is the only thing
/// that distinguishes the old commits from the new ones. Without this a
/// revised review would keep rendering the diff it was pinned to and the
/// agent's fix would never appear.
pub fn current_pin(path: &Path, revision: Option<&str>) -> Option<(crate::pin::Pin, String)> {
    let events = read(path);
    let revised_at = events
        .iter()
        .rev()
        .find(|e| matches!(e.body, Body::Revised { .. }))
        .map(|e| e.seq)
        .unwrap_or(0);
    events.into_iter().rev().find_map(|e| match e.body {
        Body::Pinned { base, head, digest } if e.revision.as_deref() == revision && e.seq > revised_at => {
            Some((crate::pin::Pin { base, head }, digest))
        }
        _ => None,
    })
}

/// The revision a review is about now.
///
/// The latest `revised` event's `to` when there is one, else the latest
/// revision any event names — which, before a revision, is the brief's.
///
/// Not simply the latest revision named by any event: the `revised` event
/// the skill tells an agent to write carries no `revision` of its own, so
/// the newest explicit one still names the spec it replaced. And it has
/// to be one answer everywhere, because diagnostics and pins are keyed on
/// it: two callers disagreeing would report every finding twice and pin
/// the review twice under two labels.
pub fn current_revision(events: &[Event]) -> Option<String> {
    events
        .iter()
        .rev()
        .find_map(|e| match &e.body {
            Body::Revised { to, .. } => Some(to.clone()),
            _ => None,
        })
        .or_else(|| events.iter().rev().find_map(|e| e.revision.clone()))
}

/// The session's verdict, if one has been reached.
pub fn verdict(path: &Path) -> Option<Event> {
    current_verdict(&read(path)).cloned()
}

/// The verdict on the review as it stands now.
///
/// A `revised` event hands over different commits, and a verdict quotes
/// the digest of the diff it was given, so one written before the latest
/// revision is about a change nobody is looking at any more. Counting it
/// would file the revised review under Decided, where nothing waits on it
/// and nothing re-pins it.
pub fn current_verdict(events: &[Event]) -> Option<&Event> {
    let revised_at = events
        .iter()
        .rev()
        .find(|e| matches!(e.body, Body::Revised { .. }))
        .map(|e| e.seq)
        .unwrap_or(0);
    events.iter().rev().find(|e| e.is_verdict() && e.seq > revised_at)
}

/// Append one event, assigning it the next `seq`.
///
/// Opened with `append`, and written as a single line in a single `write`
/// call, so two processes appending concurrently interleave whole lines
/// rather than corrupting each other's — which is what makes a
/// lock-free-two-writer design safe here at all.
pub fn append(path: &Path, from: Author, revision: Option<String>, body: Body) -> std::io::Result<Event> {
    let event = Event {
        seq: last_seq(path) + 1,
        at_ms: crate::log::now_ms(),
        from,
        revision,
        body,
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut line = serde_json::to_string(&event)?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief() -> Body {
        Body::Brief {
            title: "Move book covers into storage".into(),
            summary: "Uploads every cover and deletes the local copy.".into(),
            implications: vec!["Irreversible: the local files are deleted".into()],
            repo: Some("/tmp/repo".into()),
        }
    }

    #[test]
    fn events_round_trip_and_seq_increments() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("session.jsonl");
        assert_eq!(last_seq(&p), 0, "a missing log is an empty session");

        let a = append(&p, Author::Agent, Some("main...HEAD".into()), brief()).unwrap();
        let b = append(
            &p,
            Author::Human,
            Some("main...HEAD".into()),
            Body::Note { text: "why?".into(), file: Some("src/x.rs".into()), line: Some(42), line_content: Some("let id: String".into()), chapter: None, in_reply_to: None, flag: None },
        )
        .unwrap();
        assert_eq!((a.seq, b.seq), (1, 2));

        let all = read(&p);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], a);
        assert_eq!(all[1], b);
        assert_eq!(last_seq(&p), 2);
    }

    #[test]
    fn read_since_returns_only_what_the_caller_has_not_seen() {
        // The resume path: an agent that replied at seq 2 wants seq 3+.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Agent, None, brief()).unwrap();
        append(&p, Author::Human, None, Body::Note { text: "one".into(), file: None, line: None, line_content: None, chapter: None, in_reply_to: None, flag: None }).unwrap();
        append(&p, Author::Human, None, Body::Note { text: "two".into(), file: None, line: None, line_content: None, chapter: None, in_reply_to: None, flag: None }).unwrap();

        let fresh = read_since(&p, 2);
        assert_eq!(fresh.len(), 1);
        assert!(matches!(&fresh[0].body, Body::Note { text, .. } if text == "two"));
        assert!(read_since(&p, 3).is_empty());
    }

    #[test]
    fn a_half_written_trailing_line_is_skipped_not_fatal() {
        // Observed routinely: the other side is mid-append. Losing the
        // whole conversation over it would be far worse than ignoring one
        // event that is about to be re-read a moment later.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Agent, None, brief()).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"seq\":2,\"at_ms\":1,\"from\":\"hum").unwrap();

        let events = read(&p);
        assert_eq!(events.len(), 1, "the complete event must survive");
        assert_eq!(last_seq(&p), 1, "and seq must not count the broken line");
    }

    #[test]
    fn a_note_records_the_revision_it_was_written_against() {
        // So a comment about since-changed code reads as being about the
        // old version rather than re-anchoring to whatever is there now.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Human, Some("abc123".into()), Body::Note { text: "n".into(), file: Some("a.rs".into()), line: Some(1), line_content: Some("x".into()), chapter: None, in_reply_to: None, flag: None }).unwrap();
        append(&p, Author::Agent, Some("abc123".into()), Body::Revised { to: "def456".into(), note: Some("fixed".into()) }).unwrap();

        let events = read(&p);
        assert_eq!(events[0].revision.as_deref(), Some("abc123"));
        assert!(matches!(&events[1].body, Body::Revised { to, .. } if to == "def456"));
    }

    #[test]
    fn the_verdict_is_found_and_is_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Agent, None, brief()).unwrap();
        assert!(verdict(&p).is_none(), "no verdict until one is written");

        append(&p, Author::Human, None, Body::Verdict { verdict: Verdict::Rejected, reason: Some("no down path".into()), digest: None }).unwrap();
        let v = verdict(&p).expect("verdict must be found");
        assert!(v.is_verdict());
        assert!(matches!(v.body, Body::Verdict { verdict: Verdict::Rejected, reason: Some(r), .. } if r == "no down path"));
    }

    #[test]
    fn a_revision_retires_the_verdict_before_it() {
        // Approved, then the agent commits again: the digest that was
        // approved is no longer what is handed over, so the review is
        // waiting again until the new commits get a verdict of their own.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Agent, None, brief()).unwrap();
        append(&p, Author::Human, None, Body::Verdict { verdict: Verdict::Approved, reason: None, digest: Some("d1".into()) }).unwrap();
        append(&p, Author::Agent, None, Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        assert!(verdict(&p).is_none(), "the approval was of the old commits");

        append(&p, Author::Human, None, Body::Verdict { verdict: Verdict::Approved, reason: None, digest: Some("d2".into()) }).unwrap();
        assert!(matches!(verdict(&p).unwrap().body, Body::Verdict { digest: Some(d), .. } if d == "d2"));
    }

    #[test]
    fn two_writers_appending_concurrently_produce_whole_readable_lines() {
        // The property the lock-free design rests on: O_APPEND plus one
        // write per line means interleaving happens between lines, never
        // inside one. If this breaks, the log silently corrupts.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        std::thread::scope(|s| {
            for who in [Author::Agent, Author::Human] {
                let p = p.clone();
                s.spawn(move || {
                    for i in 0..25 {
                        let _ = append(&p, who, None, Body::Note { text: format!("{who:?}-{i}"), file: None, line: None, line_content: None, chapter: None, in_reply_to: None, flag: None });
                    }
                });
            }
        });
        let raw = std::fs::read_to_string(&p).unwrap();
        let lines = raw.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(lines, 50, "every append must land as its own line");
        assert_eq!(read(&p).len(), 50, "and every line must parse");
    }
    #[test]
    fn every_body_variant_survives_a_round_trip_through_the_flattened_event() {
        // Event flattens Body into itself, so any body field sharing a
        // name with an Event field collides on one JSON key. That does
        // not fail loudly: the line just stops deserialising, and read()
        // skips malformed lines by design, so the event disappears from
        // the conversation without a word. Revised { revision } hit
        // exactly this. Exercising every variant keeps the next added
        // field from reintroducing it.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let bodies = vec![
            brief(),
            Body::Note { text: "n".into(), file: Some("a.rs".into()), line: Some(3), line_content: Some("x".into()), chapter: None, in_reply_to: None, flag: None },
            Body::Note { text: "general".into(), file: None, line: None, line_content: None, chapter: Some("A chapter".into()), in_reply_to: None, flag: None },
            Body::Reply { text: "because".into(), in_reply_to: Some(2) },
            Body::Revised { to: "HEAD".into(), note: None },
            Body::Bundle { chapters: vec![crate::bundle::Chapter { title: "t".into(), summary: "s".into(), hunks: vec![] }] },
            Body::Verdict { verdict: Verdict::Approved, reason: None, digest: Some("abc".into()) },
            Body::Pinned { base: "a".repeat(40), head: "b".repeat(40), digest: "c".repeat(64) },
            Body::Diagnostic { code: "bundle_incomplete".into(), message: "m".into(), detail: vec!["d".into()] },
            Body::ChapterMark { chapter: "A chapter".into(), mark: Mark::Ok },
        ];
        for b in &bodies {
            append(&p, Author::Agent, Some("rev".into()), b.clone()).unwrap();
        }
        let back = read(&p);
        assert_eq!(back.len(), bodies.len(), "a variant was lost in serialisation");
        for (got, want) in back.iter().zip(&bodies) {
            assert_eq!(&got.body, want);
            assert_eq!(got.revision.as_deref(), Some("rev"), "Event::revision must survive alongside the body");
        }
    }

    #[test]
    fn a_pin_belongs_to_the_revision_it_was_taken_against() {
        // A pin from an earlier spec names commits the review is no
        // longer about. Offered as a fallback it would render the wrong
        // change while looking entirely settled, which is worse than
        // rendering nothing.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let oid = |c: char| std::iter::repeat(c).take(40).collect::<String>();

        append(
            &p,
            Author::Threshold,
            Some("main...HEAD".into()),
            Body::Pinned { base: oid('a'), head: oid('b'), digest: "d1".into() },
        )
        .unwrap();

        let (pin, digest) = current_pin(&p, Some("main...HEAD")).expect("the pin for this revision");
        assert_eq!(pin.head, oid('b'));
        assert_eq!(digest, "d1");
        assert!(current_pin(&p, Some("other...HEAD")).is_none(), "a pin must not travel to another revision");
        assert!(current_pin(&p, None).is_none(), "nor to the working tree");
    }

    #[test]
    fn announcing_a_revision_retires_the_pin_even_when_the_spec_is_unchanged() {
        // The normal case, not an edge: the skill tells the agent to
        // amend and re-announce main...HEAD, so the spec is identical on
        // both sides and only recency separates the old commits from the
        // new. Keeping the pin here would render the pre-amend diff
        // forever and the agent's fix would never appear.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let oid = |c: char| std::iter::repeat(c).take(40).collect::<String>();
        let rev = Some("main...HEAD".to_string());

        append(&p, Author::Threshold, rev.clone(), Body::Pinned { base: oid('a'), head: oid('b'), digest: "d".into() }).unwrap();
        assert!(current_pin(&p, Some("main...HEAD")).is_some());

        append(&p, Author::Agent, rev.clone(), Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        assert!(current_pin(&p, Some("main...HEAD")).is_none(), "the pin must not survive the revision it predates");

        // Re-pinning puts the review back on a fixed referent.
        append(&p, Author::Threshold, rev.clone(), Body::Pinned { base: oid('a'), head: oid('e'), digest: "d2".into() }).unwrap();
        let (pin, _) = current_pin(&p, Some("main...HEAD")).expect("re-pinned");
        assert_eq!(pin.head, oid('e'));
    }

    #[test]
    fn the_current_revision_follows_a_revised_event_that_names_no_revision_of_its_own() {
        // The skill's `revised` line has `to` and no `revision`, so the
        // newest explicit revision still names the spec it replaced.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        assert_eq!(current_revision(&read(&p)), None);

        append(&p, Author::Agent, None, brief()).unwrap();
        append(&p, Author::Agent, Some("v1".into()), Body::Bundle { chapters: vec![] }).unwrap();
        assert_eq!(current_revision(&read(&p)).as_deref(), Some("v1"), "before a revision, the latest one named");

        append(&p, Author::Agent, None, Body::Revised { to: "v2".into(), note: None }).unwrap();
        assert_eq!(current_revision(&read(&p)).as_deref(), Some("v2"));

        // A reply still labelled with the old spec must not drag it back.
        append(&p, Author::Agent, Some("v1".into()), Body::Reply { text: "r".into(), in_reply_to: None }).unwrap();
        assert_eq!(current_revision(&read(&p)).as_deref(), Some("v2"));
    }

    #[test]
    fn the_latest_pin_for_a_revision_wins() {
        // Re-pinning is how an amended commit is picked up: the spec is
        // unchanged, so only recency distinguishes the two.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let oid = |c: char| std::iter::repeat(c).take(40).collect::<String>();
        let rev = Some("main...HEAD".to_string());

        append(&p, Author::Threshold, rev.clone(), Body::Pinned { base: oid('a'), head: oid('b'), digest: "old".into() }).unwrap();
        append(&p, Author::Threshold, rev.clone(), Body::Pinned { base: oid('a'), head: oid('c'), digest: "new".into() }).unwrap();

        let (pin, digest) = current_pin(&p, Some("main...HEAD")).unwrap();
        assert_eq!(pin.head, oid('c'));
        assert_eq!(digest, "new");
    }

    #[test]
    fn a_bundle_written_against_another_revision_is_not_offered() {
        // A narrative describing code that has since changed reads as
        // authoritative while pointing at the wrong lines, so a stale
        // bundle must not be used as a fallback.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let ch = |t: &str| crate::bundle::Chapter { title: t.into(), summary: String::new(), hunks: vec![] };

        append(&p, Author::Agent, Some("old".into()), Body::Bundle { chapters: vec![ch("stale")] }).unwrap();
        assert!(current_bundle(&p, Some("new")).is_none(), "a bundle for another revision must not be returned");
        assert_eq!(current_bundle(&p, Some("old")).unwrap()[0].title, "stale");

        append(&p, Author::Agent, Some("new".into()), Body::Bundle { chapters: vec![ch("fresh")] }).unwrap();
        assert_eq!(current_bundle(&p, Some("new")).unwrap()[0].title, "fresh");
    }

    #[test]
    fn the_latest_bundle_for_a_revision_wins() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let ch = |t: &str| crate::bundle::Chapter { title: t.into(), summary: String::new(), hunks: vec![] };
        append(&p, Author::Agent, Some("r".into()), Body::Bundle { chapters: vec![ch("first")] }).unwrap();
        append(&p, Author::Agent, Some("r".into()), Body::Bundle { chapters: vec![ch("second")] }).unwrap();
        assert_eq!(current_bundle(&p, Some("r")).unwrap()[0].title, "second");
    }

    #[test]
    fn a_diagnostic_is_reported_once_per_revision() {
        // The checks run on every load and poll. Without this the log
        // fills with the same finding and the reviewer's screen with it.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let rev = Some("v1".to_string());

        let first = diagnose(&p, rev.clone(), "bundle_incomplete", "3 missing".into(), vec![]).unwrap();
        assert!(first.is_some(), "the first report is written");
        for _ in 0..5 {
            assert!(diagnose(&p, rev.clone(), "bundle_incomplete", "3 missing".into(), vec![]).unwrap().is_none());
        }
        assert_eq!(read(&p).len(), 1);

        // A different problem is different information.
        assert!(diagnose(&p, rev.clone(), "revision_unreadable", "bad rev".into(), vec![]).unwrap().is_some());
        // And the same problem against a new revision is too: the agent
        // revised and still got it wrong, which it needs to hear.
        assert!(diagnose(&p, Some("v2".into()), "bundle_incomplete", "still 3".into(), vec![]).unwrap().is_some());
        assert_eq!(read(&p).len(), 3);
    }

    #[test]
    fn a_problem_that_survives_a_new_bundle_or_revision_is_reported_again() {
        // The skill re-announces `main...HEAD` after an amend, and a
        // corrected bundle comes at the same revision, so the spec string
        // alone cannot say the finding is about something new.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let rev = Some("main...HEAD".to_string());
        let bundle = || Body::Bundle { chapters: vec![] };
        append(&p, Author::Agent, rev.clone(), bundle()).unwrap();
        assert!(diagnose(&p, rev.clone(), "bundle_incomplete", "17 of 66".into(), vec![]).unwrap().is_some());
        assert!(diagnose(&p, rev.clone(), "review_unpinned", "m".into(), vec![]).unwrap().is_some());

        // A second bundle, still incomplete: the agent needs to hear it.
        append(&p, Author::Agent, rev.clone(), bundle()).unwrap();
        assert!(diagnose(&p, rev.clone(), "bundle_incomplete", "3 of 66".into(), vec![]).unwrap().is_some());
        assert!(diagnose(&p, rev.clone(), "bundle_incomplete", "3 of 66".into(), vec![]).unwrap().is_none(), "once per bundle");
        // A bundle says nothing about the revision.
        assert!(diagnose(&p, rev.clone(), "review_unpinned", "m".into(), vec![]).unwrap().is_none());

        // A revision under the same spec replaces everything.
        append(&p, Author::Agent, rev.clone(), Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        assert!(diagnose(&p, rev.clone(), "review_unpinned", "m".into(), vec![]).unwrap().is_some());

        // Nothing but the agent's own events moves the line.
        append(&p, Author::Human, rev.clone(), Body::Note { text: "hm".into(), file: None, line: None, line_content: None, in_reply_to: None, chapter: None, flag: None }).unwrap();
        assert!(diagnose(&p, rev.clone(), "review_unpinned", "m".into(), vec![]).unwrap().is_none());
    }

    #[test]
    fn a_diagnostic_is_attributed_to_threshold_not_a_participant() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let e = diagnose(&p, None, "c", "m".into(), vec!["one".into()]).unwrap().unwrap();
        assert_eq!(e.from, Author::Threshold);
        assert!(matches!(e.body, Body::Diagnostic { detail, .. } if detail == vec!["one".to_string()]));
    }

    #[test]
    fn the_latest_mark_wins_and_can_be_taken_back() {
        // The log is append-only, so changing your mind is another event
        // rather than a rewrite, and Clear reads as no mark at all.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let mark = |m| Body::ChapterMark { chapter: "Migration".into(), mark: m };

        assert_eq!(chapter_state(&read(&p), "Migration").mark, None);
        append(&p, Author::Human, None, mark(Mark::Ok)).unwrap();
        assert_eq!(chapter_state(&read(&p), "Migration").mark, Some(Mark::Ok));
        append(&p, Author::Human, None, mark(Mark::Concern)).unwrap();
        assert_eq!(chapter_state(&read(&p), "Migration").mark, Some(Mark::Concern));
        append(&p, Author::Human, None, mark(Mark::Clear)).unwrap();
        assert_eq!(chapter_state(&read(&p), "Migration").mark, None, "Clear is the absence of a mark");
    }

    #[test]
    fn chapter_notes_are_counted_and_answered_ones_stop_being_outstanding() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let note = |t: &str, ch: &str| Body::Note {
            text: t.into(), file: None, line: None, line_content: None, chapter: Some(ch.into()), in_reply_to: None, flag: None,
        };
        let a = append(&p, Author::Human, None, note("first", "Migration")).unwrap();
        append(&p, Author::Human, None, note("second", "Migration")).unwrap();
        append(&p, Author::Human, None, note("elsewhere", "Rename")).unwrap();

        let st = chapter_state(&read(&p), "Migration");
        assert_eq!((st.notes, st.unanswered), (2, 2));

        append(&p, Author::Agent, None, Body::Reply { text: "because".into(), in_reply_to: Some(a.seq) }).unwrap();
        let st = chapter_state(&read(&p), "Migration");
        assert_eq!((st.notes, st.unanswered), (2, 1), "an answered note is no longer outstanding");
        assert_eq!(chapter_state(&read(&p), "Rename").notes, 1, "notes do not leak between chapters");
    }

    #[test]
    fn a_mark_belongs_to_the_chapter_title_it_names() {
        // Titles are the anchor. A mark must not follow a position.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Human, None, Body::ChapterMark { chapter: "First".into(), mark: Mark::Ok }).unwrap();
        let events = read(&p);
        assert_eq!(chapter_state(&events, "First").mark, Some(Mark::Ok));
        assert_eq!(chapter_state(&events, "Second").mark, None);
        assert_eq!(chapter_state(&events, "first").mark, None, "titles are matched exactly");
    }

    #[test]
    fn a_thread_gathers_the_whole_chain_however_deep() {
        // The reviewer asks, the agent answers, the reviewer asks again.
        // Each link names its parent, so a flat "replies to root" scan
        // would drop the third message entirely.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let note = |t: &str, parent: Option<u64>| Body::Note {
            text: t.into(), file: Some("a.rs".into()), line: Some(1),
            line_content: Some("x".into()), chapter: None, in_reply_to: parent, flag: None,
        };
        let root = append(&p, Author::Human, None, note("why?", None)).unwrap();
        let ans = append(&p, Author::Agent, None, Body::Reply { text: "because".into(), in_reply_to: Some(root.seq) }).unwrap();
        append(&p, Author::Human, None, note("but what about X?", Some(ans.seq))).unwrap();
        // An unrelated thread must not be swept in.
        append(&p, Author::Human, None, note("separate question", None)).unwrap();

        let events = read(&p);
        let t = thread(&events, root.seq);
        assert_eq!(t.len(), 3, "root, answer, and the follow-up to the answer");
        assert_eq!(t[0].seq, root.seq);
        assert!(matches!(&t[2].body, Body::Note { text, .. } if text == "but what about X?"));
    }

    #[test]
    fn an_unanswered_chapter_note_stops_counting_once_anything_replies() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let n = append(&p, Author::Human, None, Body::Note {
            text: "q".into(), file: None, line: None, line_content: None,
            chapter: Some("Ch".into()), in_reply_to: None, flag: None,
        }).unwrap();
        assert_eq!(chapter_state(&read(&p), "Ch").unanswered, 1);
        append(&p, Author::Agent, None, Body::Reply { text: "a".into(), in_reply_to: Some(n.seq) }).unwrap();
        assert_eq!(chapter_state(&read(&p), "Ch").unanswered, 0);
    }

    #[test]
    fn flagged_lines_are_found_and_counted_per_file_set() {
        // A chapter's tally needs the files it covers: the log records
        // which file a flag is on, but nothing about which chapter that
        // file belongs to.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let flag = |f: &str, k: Flag| Body::Note {
            text: "look".into(), file: Some(f.into()), line: Some(1),
            line_content: Some("x".into()), chapter: None, in_reply_to: None, flag: Some(k),
        };
        append(&p, Author::Agent, None, flag("a.rs", Flag::Caution)).unwrap();
        append(&p, Author::Agent, None, flag("a.rs", Flag::Context)).unwrap();
        append(&p, Author::Agent, None, flag("b.rs", Flag::Attention)).unwrap();
        // An ordinary question is not a flag.
        append(&p, Author::Human, None, Body::Note {
            text: "why?".into(), file: Some("a.rs".into()), line: Some(2),
            line_content: None, chapter: None, in_reply_to: None, flag: None,
        }).unwrap();

        let events = read(&p);
        assert_eq!(flagged(&events).len(), 3, "only flagged notes count");
        // Counted where drawn, so the diff has to show the flagged lines.
        let diff = crate::diff::parse_unified_diff(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1 @@\n+x\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -0,0 +1 @@\n+x\n",
        );
        let anchors = crate::anchor::resolve(&events, None, &diff, &mut |_, _| None);
        assert_eq!(flag_counts(&events, &anchors, &["a.rs".to_string()]), (0, 1, 1));
        assert_eq!(flag_counts(&events, &anchors, &["b.rs".to_string()]), (1, 0, 0));
        assert_eq!(flag_counts(&events, &anchors, &["a.rs".into(), "b.rs".into()]), (1, 1, 1));
        assert_eq!(flag_counts(&events, &anchors, &[]), (0, 0, 0));
    }

    #[test]
    fn a_flag_is_counted_where_it_is_drawn_and_not_once_its_code_is_gone() {
        // The table of contents must agree with the gutter. A chapter
        // promising two cautions while the diff shows one sends the
        // reviewer hunting for a mark that is not there.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Agent, None, Body::Note {
            text: "the irreversible one".into(), file: Some("a.rs".into()), line: Some(2),
            line_content: Some("DROP TABLE products".into()), chapter: None, in_reply_to: None, flag: Some(Flag::Caution),
        }).unwrap();
        let events = read(&p);
        let count = |diff: &str| {
            let diff = crate::diff::parse_unified_diff(diff);
            let anchors = crate::anchor::resolve(&events, None, &diff, &mut |_, _| None);
            flag_counts(&events, &anchors, &["a.rs".to_string()])
        };

        assert_eq!(count("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,2 @@\n keep\n+DROP TABLE products\n"), (0, 1, 0));
        // The line number now holds something else, and the code is nowhere.
        assert_eq!(count("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,2 @@\n keep\n+let x = 1;\n"), (0, 0, 0));
        // Whitespace is not movement.
        assert_eq!(count("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,1 +1,2 @@\n keep\n+    DROP TABLE products\n"), (0, 1, 0));
    }

    #[test]
    fn a_flagged_note_can_still_be_replied_to_like_any_other() {
        // A pointer that cannot be questioned would be a worse pointer;
        // reusing Note means threading already works on it.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let root = append(&p, Author::Agent, None, Body::Note {
            text: "this is the risky one".into(), file: Some("a.rs".into()), line: Some(9),
            line_content: Some("drop table".into()), chapter: None, in_reply_to: None,
            flag: Some(Flag::Caution),
        }).unwrap();
        append(&p, Author::Human, None, Body::Note {
            text: "risky how?".into(), file: Some("a.rs".into()), line: Some(9),
            line_content: Some("drop table".into()), chapter: None,
            in_reply_to: Some(root.seq), flag: None,
        }).unwrap();
        assert_eq!(thread(&read(&p), root.seq).len(), 2);
    }

}
