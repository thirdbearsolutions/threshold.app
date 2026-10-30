//! # threshold-core
//!
//! Parse `git diff` output into a structured, renderable form (`diff`),
//! count what it changes (`facts`), validate the revision
//! a diff is taken against (`revision`), resolve that revision to the
//! commits it names so the diff cannot move under the reviewer (`pin`),
//! carry the review conversation between agent and human (`session`),
//! and persist a log of approve/reject decisions (`log`).

mod anchor;
mod bundle;
mod diff;
mod inbox;
mod log;
mod pin;
mod revision;
mod facts;
mod session;

pub use anchor::{map_line, pin_when, resolve as resolve_anchors, Anchor, AnchorStatus};
pub use bundle::{coverage, hunk_refs, Chapter, Coverage, HunkRef};
pub use diff::{line_at, parse_unified_diff, DiffFile, DiffLine, FileStatus, Hunk, LineKind};
pub use inbox::{inbox_dir, list as list_reviews, resolve_name as resolve_review_name, set_archived, waiting, ReviewNameError, ReviewSummary, INBOX_SUBPATH};
pub use log::{append as append_decision, load as load_decisions, now_ms, Decision, Verdict};
pub use pin::{digest, is_object_id, parse_range, Pin, Range};
pub use revision::{validate_revision, RevisionError, MAX_REVISION_LEN};
pub use facts::{summarize, DiffFacts};
pub use session::{
    append as append_event, chapter_state, flag_counts, flagged, thread, current_bundle, current_pin, current_revision, diagnose, last_seq, read as read_session, read_since, verdict as session_verdict,
    Author, Body, ChapterState, Event, Flag, Mark,
};
