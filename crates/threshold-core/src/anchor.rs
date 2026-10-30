//! Keeping a line annotation on the code it was written about.
//!
//! A note records a file, a new-file line number and the text of that
//! line. The number is only meaningful for the commits the note was
//! written against. After the agent amends and the review re-pins, the
//! same number points at whatever now occupies it, and "this is the
//! irreversible one" goes on sitting confidently on a different line.
//!
//! So a note is carried forward rather than re-read:
//!
//! 1. The log says which pin was in force when it was written — the pin
//!    of the revision epoch it falls in ([`pin_when`]).
//! 2. When that differs from the pin in force now, git's own diff between
//!    the two heads says where every surviving line went ([`map_line`]),
//!    renames included.
//! 3. A line the new commits changed or deleted has no new position; the
//!    note is *outdated*, placed where that code used to be, and says so.
//!
//! A review that could not be pinned has no commits to diff between. There
//! the note's captured line text is looked for instead, nearest the
//! original line, which is weaker but far better than trusting the number.

use serde::Serialize;
use std::collections::HashMap;

use crate::diff::{line_at, DiffFile, Hunk, LineKind};
use crate::pin::Pin;
use crate::session::{Body, Event};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorStatus {
    /// On the line it was written on.
    Current,
    /// The code moved; the note moved with it.
    Moved,
    /// The code it was about was changed or removed. Placed where that
    /// code was, and never to be presented as a claim about what is there.
    Outdated,
}

/// Where a line note belongs in the diff on screen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Anchor {
    /// The root note's seq. Replies follow their thread.
    pub seq: u64,
    pub file: String,
    pub line: u32,
    pub status: AnchorStatus,
    /// Whether that line is part of the diff at all. A note on code the
    /// revision no longer changes has nowhere to be drawn inline, and the
    /// conversation list is the only place it can still be read.
    pub in_diff: bool,
}

/// `(start, count)` for each side of `@@ -a,b +c,d @@`, with git's
/// convention that an omitted count is 1.
fn ranges(header: &str) -> Option<((u32, u32), (u32, u32))> {
    let mut parts = header.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let side = |s: &str, sign: char| -> Option<(u32, u32)> {
        let s = s.strip_prefix(sign)?;
        let mut it = s.splitn(2, ',');
        let start = it.next()?.parse().ok()?;
        let count = it.next().map(str::parse).transpose().ok()?.unwrap_or(1);
        Some((start, count))
    };
    Some((side(parts.next()?, '-')?, side(parts.next()?, '+')?))
}

/// Map a line on the old side of a zero-context diff to the new side.
///
/// `Err(near)` when the line itself was changed or deleted, with `near`
/// the new-side line where that code was.
///
/// For an empty side git writes the line *before* the gap (`-3,0` inserts
/// after old line 3), so an empty range is taken to begin one line later.
pub fn map_line(hunks: &[Hunk], line: u32) -> Result<u32, u32> {
    let mut shift: i64 = 0;
    for hunk in hunks {
        let Some(((a, b), (c, d))) = ranges(&hunk.header) else { continue };
        let old_start = if b == 0 { a + 1 } else { a };
        let new_start = if d == 0 { c + 1 } else { c };
        if line < old_start {
            break;
        }
        if line < old_start + b {
            return Err(new_start);
        }
        shift = (new_start + d) as i64 - (old_start + b) as i64;
    }
    Ok((line as i64 + shift).max(1) as u32)
}

/// The pin that was in force when event `seq` was written.
///
/// A `revised` event starts a new epoch and the pin taken in it belongs to
/// every event in it — including an agent's flags, which arrive in the
/// same burst as the brief, a moment *before* Threshold pins.
pub fn pin_when(events: &[Event], seq: u64) -> Option<Pin> {
    let start = events
        .iter()
        .filter(|e| e.seq < seq && matches!(e.body, Body::Revised { .. }))
        .map(|e| e.seq)
        .max()
        .unwrap_or(0);
    let end = events
        .iter()
        .filter(|e| e.seq > seq && matches!(e.body, Body::Revised { .. }))
        .map(|e| e.seq)
        .min()
        .unwrap_or(u64::MAX);
    events.iter().rev().find_map(|e| match &e.body {
        Body::Pinned { base, head, .. } if e.seq > start && e.seq < end => Some(Pin { base: base.clone(), head: head.clone() }),
        _ => None,
    })
}

/// Whether two lines are the same code. Trimmed, because a re-indent
/// moves code without changing it.
fn same_code(a: &str, b: &str) -> bool {
    a.trim() == b.trim()
}

/// Every new-side line of `path` in `diff`, numbered.
fn new_lines<'a>(diff: &'a [DiffFile], path: &str) -> Vec<(u32, &'a str)> {
    let Some(file) = diff.iter().find(|f| f.display_path() == path) else { return vec![] };
    let mut out = vec![];
    for hunk in &file.hunks {
        let Some((_, (c, _))) = ranges(&hunk.header) else { continue };
        let mut n = c;
        for l in &hunk.lines {
            if l.kind == LineKind::Removed {
                continue;
            }
            out.push((n, l.content.as_str()));
            n += 1;
        }
    }
    out
}

/// Without commits to diff between: find the note's code by its text.
///
/// Only text worth matching is looked for. A `}` or a blank line occurs
/// everywhere, and a confident "moved" onto the nearest brace is worse
/// than admitting the note is outdated.
fn by_content(diff: &[DiffFile], file: &str, line: u32, content: Option<&str>) -> (u32, AnchorStatus) {
    let Some(content) = content else {
        return (line, AnchorStatus::Current);
    };
    if line_at(diff, file, line).is_some_and(|cur| same_code(cur, content)) {
        return (line, AnchorStatus::Current);
    }
    if content.trim().len() < 3 {
        return (line, AnchorStatus::Outdated);
    }
    new_lines(diff, file)
        .into_iter()
        .filter(|(_, text)| same_code(text, content))
        .min_by_key(|(n, _)| n.abs_diff(line))
        .map(|(n, _)| (n, AnchorStatus::Moved))
        .unwrap_or((line, AnchorStatus::Outdated))
}

/// Place every line-anchored thread in the diff on screen.
///
/// `current` is the pin being rendered, `diff` what it renders, and
/// `between(old_head, new_head)` a zero-context, rename-detecting diff
/// between two commits, or `None` when git cannot produce one — the old
/// commits pruned, say — in which case the note falls back to its text.
pub fn resolve(
    events: &[Event],
    current: Option<&Pin>,
    diff: &[DiffFile],
    between: &mut dyn FnMut(&str, &str) -> Option<Vec<DiffFile>>,
) -> Vec<Anchor> {
    let mut cache: HashMap<String, Option<Vec<DiffFile>>> = HashMap::new();
    let mut out = vec![];
    for e in events {
        let Body::Note { file: Some(file), line: Some(line), line_content, in_reply_to: None, .. } = &e.body else {
            continue;
        };
        let content = line_content.as_deref();
        let written = pin_when(events, e.seq);

        let (path, n, status) = match (written, current) {
            (Some(w), Some(c)) if w.head == c.head => (file.clone(), *line, AnchorStatus::Current),
            (Some(w), Some(c)) => {
                let moved = cache.entry(w.head.clone()).or_insert_with(|| between(&w.head, &c.head));
                match moved {
                    None => {
                        let (n, s) = by_content(diff, file, *line, content);
                        (file.clone(), n, s)
                    }
                    Some(changes) => match changes.iter().find(|f| f.old_path.as_deref() == Some(file.as_str())) {
                        // Untouched between the two heads: every line is where it was.
                        None => (file.clone(), *line, AnchorStatus::Current),
                        Some(f) => match &f.new_path {
                            None => (file.clone(), *line, AnchorStatus::Outdated),
                            Some(to) => match map_line(&f.hunks, *line) {
                                Ok(m) if m == *line && to == file => (to.clone(), m, AnchorStatus::Current),
                                Ok(m) => (to.clone(), m, AnchorStatus::Moved),
                                Err(near) => (to.clone(), near, AnchorStatus::Outdated),
                            },
                        },
                    },
                }
            }
            _ => {
                let (n, s) = by_content(diff, file, *line, content);
                (file.clone(), n, s)
            }
        };

        // Whatever placed it, a line that now reads differently is not the
        // code the note was about.
        let status = match (content, line_at(diff, &path, n)) {
            (Some(want), Some(cur)) if !same_code(want, cur) => AnchorStatus::Outdated,
            _ => status,
        };
        out.push(Anchor { seq: e.seq, in_diff: line_at(diff, &path, n).is_some(), file: path, line: n, status });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::parse_unified_diff;
    use crate::session::{append, read, Author};

    fn hunks(diff: &str) -> Vec<Hunk> {
        parse_unified_diff(diff).remove(0).hunks
    }

    const HEAD: &str = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n";

    #[test]
    fn lines_after_an_insertion_move_down_and_lines_before_it_stay() {
        // Two lines inserted after old line 3.
        let h = hunks(&format!("{HEAD}@@ -3,0 +4,2 @@\n+x\n+y\n"));
        assert_eq!(map_line(&h, 3), Ok(3));
        assert_eq!(map_line(&h, 4), Ok(6));
        assert_eq!(map_line(&h, 10), Ok(12));
    }

    #[test]
    fn lines_after_a_deletion_move_up_and_the_deleted_ones_are_outdated() {
        let h = hunks(&format!("{HEAD}@@ -4,2 +3,0 @@\n-a\n-b\n"));
        assert_eq!(map_line(&h, 3), Ok(3));
        assert_eq!(map_line(&h, 4), Err(4));
        assert_eq!(map_line(&h, 5), Err(4));
        assert_eq!(map_line(&h, 6), Ok(4));
    }

    #[test]
    fn a_changed_line_is_outdated_where_it_stood() {
        let h = hunks(&format!("{HEAD}@@ -4 +4 @@\n-old\n+new\n"));
        assert_eq!(map_line(&h, 4), Err(4));
        assert_eq!(map_line(&h, 5), Ok(5));
    }

    #[test]
    fn shifts_from_several_hunks_accumulate() {
        let h = hunks(&format!("{HEAD}@@ -2,0 +3 @@\n+x\n@@ -10,3 +11 @@\n-a\n-b\n-c\n+d\n"));
        assert_eq!(map_line(&h, 5), Ok(6), "one line in above it");
        assert_eq!(map_line(&h, 20), Ok(19), "one in, then three out and one in");
    }

    fn oid(c: char) -> String {
        std::iter::repeat(c).take(40).collect()
    }

    fn note(file: &str, line: u32, content: &str) -> Body {
        Body::Note { text: "the irreversible one".into(), file: Some(file.into()), line: Some(line), line_content: Some(content.into()), chapter: None, in_reply_to: None, flag: None }
    }

    fn pinned(head: char) -> Body {
        Body::Pinned { base: oid('0'), head: oid(head), digest: "d".into() }
    }

    #[test]
    fn a_flag_pushed_before_the_pin_belongs_to_that_pin() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        let flag = append(&p, Author::Agent, None, note("a.rs", 5, "x")).unwrap();
        append(&p, Author::Threshold, None, pinned('a')).unwrap();
        append(&p, Author::Agent, None, Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        append(&p, Author::Threshold, None, pinned('b')).unwrap();
        let later = append(&p, Author::Human, None, note("a.rs", 9, "y")).unwrap();
        let events = read(&p);
        assert_eq!(pin_when(&events, flag.seq).unwrap().head, oid('a'));
        assert_eq!(pin_when(&events, later.seq).unwrap().head, oid('b'));
    }

    /// The diff on screen after the amend: `DROP TABLE` now on line 7.
    fn current_diff() -> Vec<DiffFile> {
        parse_unified_diff(&format!("{HEAD}@@ -1,3 +1,8 @@\n keep\n+one\n+two\n+three\n+four\n+five\n+DROP TABLE products\n+tail\n"))
    }

    #[test]
    fn a_note_follows_its_code_across_an_amend() {
        // The bug: written on line 2 against the first commits, the note
        // stayed on line 2 after the amend pushed the code to line 7.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Threshold, None, pinned('a')).unwrap();
        let n = append(&p, Author::Human, None, note("a.rs", 2, "DROP TABLE products")).unwrap();
        append(&p, Author::Agent, None, Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        append(&p, Author::Threshold, None, pinned('b')).unwrap();
        let events = read(&p);

        let mut asked = vec![];
        let mut between = |from: &str, to: &str| {
            asked.push((from.to_string(), to.to_string()));
            // Five lines inserted above it between the two heads.
            Some(parse_unified_diff(&format!("{HEAD}@@ -1,0 +2,5 @@\n+one\n+two\n+three\n+four\n+five\n")))
        };
        let current = Pin { base: oid('0'), head: oid('b') };
        let anchors = resolve(&events, Some(&current), &current_diff(), &mut between);

        assert_eq!(asked, vec![(oid('a'), oid('b'))], "diffed from the commits it was written against to the ones on screen");
        assert_eq!(anchors, vec![Anchor { seq: n.seq, file: "a.rs".into(), line: 7, status: AnchorStatus::Moved, in_diff: true }]);
    }

    #[test]
    fn a_note_on_code_the_amend_changed_is_outdated_not_moved_onto_something_else() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Threshold, None, pinned('a')).unwrap();
        append(&p, Author::Human, None, note("a.rs", 7, "DROP TABLE legacy")).unwrap();
        append(&p, Author::Agent, None, Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        append(&p, Author::Threshold, None, pinned('b')).unwrap();
        let events = read(&p);
        let mut between = |_: &str, _: &str| Some(parse_unified_diff(&format!("{HEAD}@@ -7 +7 @@\n-DROP TABLE legacy\n+DROP TABLE products\n")));
        let a = resolve(&events, Some(&Pin { base: oid('0'), head: oid('b') }), &current_diff(), &mut between);
        assert_eq!((a[0].line, a[0].status), (7, AnchorStatus::Outdated));
    }

    #[test]
    fn a_note_follows_a_renamed_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Threshold, None, pinned('a')).unwrap();
        append(&p, Author::Human, None, note("old.rs", 2, "DROP TABLE products")).unwrap();
        append(&p, Author::Agent, None, Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        append(&p, Author::Threshold, None, pinned('b')).unwrap();
        let events = read(&p);
        let mut between = |_: &str, _: &str| {
            Some(parse_unified_diff("diff --git a/old.rs b/a.rs\nsimilarity index 90%\nrename from old.rs\nrename to a.rs\n--- a/old.rs\n+++ b/a.rs\n@@ -1,0 +2,5 @@\n+one\n+two\n+three\n+four\n+five\n"))
        };
        let a = resolve(&events, Some(&Pin { base: oid('0'), head: oid('b') }), &current_diff(), &mut between);
        assert_eq!((a[0].file.as_str(), a[0].line, a[0].status), ("a.rs", 7, AnchorStatus::Moved));
    }

    #[test]
    fn without_pins_a_note_finds_its_code_by_text_nearest_where_it_was() {
        // An unpinned review: the working tree moved, and there are no
        // commits to diff between.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Human, None, note("a.rs", 2, "  DROP TABLE products")).unwrap();
        append(&p, Author::Human, None, note("a.rs", 4, "}")).unwrap();
        let events = read(&p);
        let mut between = |_: &str, _: &str| -> Option<Vec<DiffFile>> { panic!("nothing to diff between") };
        let a = resolve(&events, None, &current_diff(), &mut between);
        assert_eq!((a[0].line, a[0].status), (7, AnchorStatus::Moved), "found by its text, re-indent and all");
        assert_eq!(a[1].status, AnchorStatus::Outdated, "a brace is not evidence of anything");
    }

    #[test]
    fn under_the_same_pin_a_note_stays_put() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.jsonl");
        append(&p, Author::Threshold, None, pinned('b')).unwrap();
        append(&p, Author::Human, None, note("a.rs", 7, "DROP TABLE products")).unwrap();
        // Replies ride along with their thread and are not placed.
        append(&p, Author::Human, None, Body::Note { text: "and?".into(), file: Some("a.rs".into()), line: Some(7), line_content: None, chapter: None, in_reply_to: Some(2), flag: None }).unwrap();
        let events = read(&p);
        let mut between = |_: &str, _: &str| -> Option<Vec<DiffFile>> { panic!("same commits") };
        let a = resolve(&events, Some(&Pin { base: oid('0'), head: oid('b') }), &current_diff(), &mut between);
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].line, a[0].status, a[0].in_diff), (7, AnchorStatus::Current, true));
    }
}
