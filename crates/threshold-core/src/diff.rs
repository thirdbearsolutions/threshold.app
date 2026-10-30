//! Parsing `git diff`'s unified-diff output into a structured form a UI
//! can render line by line. Handles the four shapes a file entry can take
//! (modified, added, deleted, renamed) and the `@@ ... @@` hunks within
//! each — everything `git diff`/`git diff --cached` actually produce,
//! nothing more (no support for arbitrary hand-written patch dialects).

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiffFile {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub hunks: Vec<Hunk>,
    pub additions: usize,
    pub deletions: usize,
    /// git reported the file as binary: it has no lines, only the one
    /// hunk standing for it (see [`parse_unified_diff`]).
    #[serde(default)]
    pub binary: bool,
}

impl DiffFile {
    /// The path worth showing in a file list: the new path for anything
    /// that still exists, the old path for a pure deletion.
    pub fn display_path(&self) -> &str {
        self.new_path.as_deref().or(self.old_path.as_deref()).unwrap_or("(unknown)")
    }
}

/// The new-file line number a hunk starts at, from its `@@ -a,b +c,d @@`
/// header. Defaults to 1 for a header this cannot read, matching what
/// the renderer does, so a malformed header misplaces an annotation
/// rather than losing the hunk.
fn new_start(header: &str) -> u32 {
    header
        .split_once('+')
        .map(|(_, rest)| rest)
        .map(|rest| rest.chars().take_while(char::is_ascii_digit).collect::<String>())
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
}

/// The text at new-file line `line` of `path`, if this diff shows it.
///
/// Exists so an annotation can be checked against the line it is sitting
/// on. A note anchors by file and line number, which is only meaningful
/// for the diff it was written against; once the code moves, the number
/// still resolves and still points at something, just not at the thing
/// the note is about. Comparing the text is the only way to tell.
///
/// Removed lines are skipped because they have no new-file number, which
/// is the same rule the renderer uses when deciding what can be
/// annotated at all.
pub fn line_at<'a>(files: &'a [DiffFile], path: &str, line: u32) -> Option<&'a str> {
    let file = files.iter().find(|f| f.display_path() == path)?;
    for hunk in &file.hunks {
        let mut n = new_start(&hunk.header);
        for l in &hunk.lines {
            if l.kind == LineKind::Removed {
                continue;
            }
            if n == line {
                return Some(&l.content);
            }
            n += 1;
        }
    }
    None
}

fn strip_prefix<'a>(path: &'a str, prefix: &str) -> &'a str {
    path.strip_prefix(prefix).unwrap_or(path)
}

fn parse_path_line(line: &str, marker: &str) -> Option<String> {
    let rest = line.strip_prefix(marker)?.trim();
    if rest == "/dev/null" {
        None
    } else {
        Some(strip_prefix(strip_prefix(rest, "a/"), "b/").to_string())
    }
}

/// The path a `diff --git a/P b/P` line names, when both sides are the
/// same path. That is every entry but a rename, and the only place a
/// binary file or a mode-only change names itself at all: neither has
/// `---`/`+++` lines. A space in the path makes splitting on spaces
/// ambiguous, so this uses the one split where both halves agree.
fn same_path(line: &str) -> Option<String> {
    let rest = line.strip_prefix("diff --git ")?;
    // "a/" + P + " b/" + P
    let n = rest.len().checked_sub(5)?;
    if n % 2 != 0 {
        return None;
    }
    let p = rest.get(2..2 + n / 2)?;
    (rest == format!("a/{p} b/{p}")).then(|| p.to_string())
}

/// Parse the full output of `git diff` (or `git diff --cached`) into one
/// [`DiffFile`] per file entry, in order.
///
/// A binary file has no lines to show, and so no `@@` hunk. It gets one
/// hunk whose header is git's own `Binary files … differ` line and which
/// has no lines. A narrative can then place it in a chapter like any other
/// hunk, and the coverage audit notices one left out; a file with no hunks
/// at all could be neither.
pub fn parse_unified_diff(text: &str) -> Vec<DiffFile> {
    let lines: Vec<&str> = text.lines().collect();
    let mut files = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if !lines[i].starts_with("diff --git ") {
            i += 1;
            continue;
        }
        let named = same_path(lines[i]);
        i += 1;

        let mut status = FileStatus::Modified;
        let mut old_path = None;
        let mut new_path = None;
        let mut saw_paths = false;
        let mut binary: Option<String> = None;

        // Header lines between "diff --git" and the first hunk (or the
        // next file's "diff --git").
        while i < lines.len() && !lines[i].starts_with("@@") && !lines[i].starts_with("diff --git ") {
            let line = lines[i];
            if let Some(p) = line.strip_prefix("rename from ") {
                old_path = Some(p.trim().to_string());
                status = FileStatus::Renamed;
            } else if let Some(p) = line.strip_prefix("rename to ") {
                new_path = Some(p.trim().to_string());
                status = FileStatus::Renamed;
            } else if line.starts_with("new file mode") {
                status = FileStatus::Added;
            } else if line.starts_with("deleted file mode") {
                status = FileStatus::Deleted;
            } else if line.starts_with("Binary files ") && line.ends_with(" differ") {
                binary = Some(line.to_string());
            } else if let Some(p) = parse_path_line(line, "--- ") {
                old_path = Some(p);
                saw_paths = true;
            } else if line.starts_with("--- ") {
                old_path = None; // "--- /dev/null"
                saw_paths = true;
            } else if let Some(p) = parse_path_line(line, "+++ ") {
                new_path = Some(p);
                saw_paths = true;
            } else if line.starts_with("+++ ") {
                new_path = None; // "+++ /dev/null"
                saw_paths = true;
            }
            i += 1;
        }

        // No `---`/`+++`: a binary file or a mode change. A rename has
        // already named both sides; anything else is named by its
        // `diff --git` line, on whichever sides exist.
        if !saw_paths && status != FileStatus::Renamed {
            if status != FileStatus::Added {
                old_path = named.clone();
            }
            if status != FileStatus::Deleted {
                new_path = named;
            }
        }

        let mut hunks = Vec::new();
        let mut additions = 0;
        let mut deletions = 0;

        while i < lines.len() && lines[i].starts_with("@@") {
            let header = lines[i].to_string();
            i += 1;
            let mut hunk_lines = Vec::new();
            while i < lines.len() && !lines[i].starts_with("@@") && !lines[i].starts_with("diff --git ") {
                let line = lines[i];
                let (kind, content) = if let Some(c) = line.strip_prefix('+') {
                    additions += 1;
                    (LineKind::Added, c.to_string())
                } else if let Some(c) = line.strip_prefix('-') {
                    deletions += 1;
                    (LineKind::Removed, c.to_string())
                } else {
                    (LineKind::Context, line.strip_prefix(' ').unwrap_or(line).to_string())
                };
                hunk_lines.push(DiffLine { kind, content });
                i += 1;
            }
            hunks.push(Hunk { header, lines: hunk_lines });
        }

        if let Some(header) = &binary {
            hunks.push(Hunk { header: header.clone(), lines: Vec::new() });
        }
        files.push(DiffFile { old_path, new_path, status, hunks, additions, deletions, binary: binary.is_some() });
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_modification() {
        let diff = "diff --git a/src/foo.rs b/src/foo.rs\nindex abc123..def456 100644\n--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1,3 +1,4 @@\n fn main() {\n+    println!(\"hi\");\n     let x = 1;\n }\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.status, FileStatus::Modified);
        assert_eq!(f.new_path.as_deref(), Some("src/foo.rs"));
        assert_eq!(f.additions, 1);
        assert_eq!(f.deletions, 0);
        assert_eq!(f.hunks.len(), 1);
        assert_eq!(f.hunks[0].header, "@@ -1,3 +1,4 @@");
        assert!(f.hunks[0].lines.iter().any(|l| l.kind == LineKind::Added && l.content.contains("println")));
    }

    #[test]
    fn parses_a_new_file() {
        let diff = "diff --git a/new.rs b/new.rs\nnew file mode 100644\nindex 0000000..abc123\n--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1,2 @@\n+line1\n+line2\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files[0].status, FileStatus::Added);
        assert_eq!(files[0].old_path, None);
        assert_eq!(files[0].new_path.as_deref(), Some("new.rs"));
        assert_eq!(files[0].additions, 2);
    }

    #[test]
    fn parses_a_deleted_file() {
        let diff = "diff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\nindex abc123..0000000\n--- a/gone.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-line1\n-line2\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files[0].status, FileStatus::Deleted);
        assert_eq!(files[0].old_path.as_deref(), Some("gone.rs"));
        assert_eq!(files[0].new_path, None);
        assert_eq!(files[0].deletions, 2);
        assert_eq!(files[0].display_path(), "gone.rs");
    }

    #[test]
    fn parses_a_pure_rename_with_no_content_change() {
        let diff = "diff --git a/old.rs b/new.rs\nsimilarity index 100%\nrename from old.rs\nrename to new.rs\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files[0].status, FileStatus::Renamed);
        assert_eq!(files[0].old_path.as_deref(), Some("old.rs"));
        assert_eq!(files[0].new_path.as_deref(), Some("new.rs"));
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn parses_multiple_files_in_one_diff() {
        let diff = "diff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/b.rs b/b.rs\nindex 3..4 100644\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].display_path(), "a.rs");
        assert_eq!(files[1].display_path(), "b.rs");
    }

    #[test]
    fn context_lines_keep_their_content_without_the_leading_space() {
        let diff = "diff --git a/f.rs b/f.rs\nindex 1..2 100644\n--- a/f.rs\n+++ b/f.rs\n@@ -1,2 +1,2 @@\n unchanged line\n-old\n+new\n";
        let files = parse_unified_diff(diff);
        let context = &files[0].hunks[0].lines[0];
        assert_eq!(context.kind, LineKind::Context);
        assert_eq!(context.content, "unchanged line");
    }

    #[test]
    fn a_new_binary_file_is_named_and_stands_as_one_hunk() {
        let diff = "diff --git a/docs/world overview.png b/docs/world overview.png\nnew file mode 100644\nindex 0000000..c36b068\nBinary files /dev/null and b/docs/world overview.png differ\ndiff --git a/a.rs b/a.rs\nindex 1..2 100644\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 2);
        let png = &files[0];
        assert!(png.binary);
        assert_eq!(png.status, FileStatus::Added);
        assert_eq!((png.old_path.as_deref(), png.new_path.as_deref()), (None, Some("docs/world overview.png")));
        assert_eq!(png.hunks.len(), 1, "one hunk, so a chapter can place it");
        assert_eq!(png.hunks[0].header, "Binary files /dev/null and b/docs/world overview.png differ");
        assert!(png.hunks[0].lines.is_empty());
        assert!(!files[1].binary);
    }

    #[test]
    fn a_modified_and_a_deleted_binary_file_are_named_on_the_sides_they_exist() {
        let diff = "diff --git a/i.png b/i.png\nindex 1..2 100644\nBinary files a/i.png and b/i.png differ\ndiff --git a/gone.png b/gone.png\ndeleted file mode 100644\nindex 3..0000000\nBinary files a/gone.png and /dev/null differ\n";
        let files = parse_unified_diff(diff);
        assert_eq!((files[0].old_path.as_deref(), files[0].new_path.as_deref()), (Some("i.png"), Some("i.png")));
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!((files[1].old_path.as_deref(), files[1].new_path.as_deref()), (Some("gone.png"), None));
    }

    #[test]
    fn a_mode_change_is_named_too() {
        let diff = "diff --git a/run.sh b/run.sh\nold mode 100644\nnew mode 100755\n";
        let f = &parse_unified_diff(diff)[0];
        assert_eq!(f.display_path(), "run.sh");
        assert!(!f.binary);
        assert!(f.hunks.is_empty());
    }

    #[test]
    fn a_binary_hunk_counts_toward_coverage() {
        let diff = "diff --git a/i.png b/i.png\nnew file mode 100644\nBinary files /dev/null and b/i.png differ\n";
        let files = parse_unified_diff(diff);
        let cov = crate::coverage(&files, &[]);
        assert_eq!(cov.unbundled.len(), 1, "a narrative that leaves an image out is told so");
    }

    #[test]
    fn an_empty_diff_yields_no_files() {
        assert!(parse_unified_diff("").is_empty());
    }
}
