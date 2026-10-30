//! The sample review behind the tour.
//!
//! Real rather than staged: a small git repo with two commits, and a
//! review of them in the inbox, so the tour walks through the app as it
//! actually behaves, pinning and coverage checks included. The repo is
//! made-up: a library catalogue moving its book covers into storage.

use std::path::{Path, PathBuf};

use threshold_core::{Author, Body, Chapter, Flag, HunkRef};

/// File-name suffix that marks the sample in the inbox.
pub(crate) const SUFFIX: &str = "-sample-tour.jsonl";

/// Whether a review log is the tour's sample. Its verdict goes in the
/// review's own log only, never in History.
pub(crate) fn is_sample(path: &Path) -> bool {
    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(SUFFIX))
}

const REVISION: &str = "main~1...main";

const BEFORE: &[(&str, &str)] = &[
    ("README.md", "# stacks\n\nA made-up library catalogue, for Threshold's tour.\n"),
    ("stacks/settings.py", "COVERS_DIR = \"/var/stacks/covers\"\n"),
    (
        "stacks/covers.py",
        "import os\n\nfrom stacks import settings\n\n\ndef cover_path(book):\n    return os.path.join(settings.COVERS_DIR, f\"{book.isbn}.jpg\")\n\n\ndef cover_url(book):\n    return f\"/covers/{book.isbn}.jpg\"\n",
    ),
    (
        "stacks/views.py",
        "from stacks import covers\n\n\ndef book_card(book):\n    return {\"title\": book.title, \"cover\": covers.cover_url(book)}\n",
    ),
    (
        "tests/test_covers.py",
        "from stacks import covers\n\n\ndef test_cover_url_uses_the_isbn(book):\n    assert covers.cover_url(book).endswith(f\"{book.isbn}.jpg\")\n",
    ),
];

const AFTER: &[(&str, &str)] = &[
    ("stacks/settings.py", "COVERS_DIR = \"/var/stacks/covers\"\nCOVERS_BUCKET = \"stacks-covers\"\n"),
    (
        "stacks/covers.py",
        "from stacks import storage\n\n\ndef cover_key(book):\n    return f\"covers/{book.isbn}.jpg\"\n\n\ndef cover_url(book):\n    return storage.url(cover_key(book))\n",
    ),
    (
        "stacks/views.py",
        "from stacks import covers, storage\n\n\ndef book_card(book):\n    key = covers.cover_key(book)\n    cover = covers.cover_url(book) if storage.exists(key) else \"/static/no-cover.svg\"\n    return {\"title\": book.title, \"cover\": cover}\n",
    ),
    (
        "stacks/commands/move_covers.py",
        "\"\"\"Copy every cover into storage, then delete the local file.\"\"\"\n\nimport os\n\nfrom stacks import settings, storage\n\n\ndef run():\n    for name in sorted(os.listdir(settings.COVERS_DIR)):\n        path = os.path.join(settings.COVERS_DIR, name)\n        storage.put(f\"covers/{name}\", path)\n        os.remove(path)\n",
    ),
    (
        "tests/test_covers.py",
        "from stacks import covers, views\n\n\ndef test_cover_url_uses_the_isbn(book):\n    assert covers.cover_url(book).endswith(f\"{book.isbn}.jpg\")\n\n\ndef test_a_missing_cover_shows_the_placeholder(book_without_cover):\n    assert views.book_card(book_without_cover)[\"cover\"] == \"/static/no-cover.svg\"\n",
    ),
];

/// Chapters in the order that explains the change, each claiming the
/// hunks of the files listed. Every file in the diff is in one.
const CHAPTERS: &[(&str, &str, &[&str])] = &[
    ("Covers come from storage", "The actual idea: a cover is a key in storage, not a file on this server.", &["stacks/covers.py", "stacks/settings.py"]),
    ("Pages cope with a missing cover", "A placeholder instead of a broken image, and a test for it.", &["stacks/views.py", "tests/test_covers.py"]),
    ("Moving the files", "A one-off command, and the irreversible part.", &["stacks/commands/move_covers.py"]),
];

/// The answered thread: where it sits, the question, and the answer.
const THREAD: (&str, &str, &str, &str) = (
    "stacks/views.py",
    "    cover = covers.cover_url(book) if storage.exists(key) else \"/static/no-cover.svg\"",
    "Doesn't this ask storage about every book on every page?",
    "Once per book card, yes. A page shows twenty at most and storage.exists is a cached lookup, so it costs a few milliseconds. If it shows up as slow, the placeholder can move into the template instead.",
);

/// Lines the sample agent flags: file, the line's exact text, the flag,
/// and what it says.
const FLAGS: &[(&str, &str, Flag, &str)] = &[
    ("stacks/covers.py", "    return storage.url(cover_key(book))", Flag::Attention, "Every cover URL now comes from here."),
    ("stacks/commands/move_covers.py", "        os.remove(path)", Flag::Caution, "This is the irreversible one: each local cover is deleted once it is uploaded."),
    ("stacks/settings.py", "COVERS_BUCKET = \"stacks-covers\"", Flag::Context, "Read once at startup, like COVERS_DIR above it."),
];

fn write_files(repo: &Path, files: &[(&str, &str)]) -> Result<(), String> {
    for (path, text) in files {
        let p = repo.join(path);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        }
        std::fs::write(&p, text).map_err(|e| format!("writing {}: {e}", p.display()))?;
    }
    Ok(())
}

/// git with the user's own configuration kept out: no hooks, no signing,
/// a fixed author and dates, so the sample is the same on every machine.
fn git(repo: &Path, args: &[&str]) -> Result<(), String> {
    let out = crate::git_command()
        .arg("-C")
        .arg(repo)
        .args(["-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Threshold sample")
        .env("GIT_AUTHOR_EMAIL", "sample@threshold.invalid")
        .env("GIT_COMMITTER_NAME", "Threshold sample")
        .env("GIT_COMMITTER_EMAIL", "sample@threshold.invalid")
        .env("GIT_AUTHOR_DATE", "2026-01-01T12:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T12:00:00Z")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .map_err(crate::git_spawn_error)?;
    if !out.status.success() {
        return Err(crate::git_error(&out.stderr));
    }
    Ok(())
}

/// Build the sample repo under `root` (replacing any earlier one), and a
/// review of it in `inbox` named `stamp` + [`SUFFIX`]. Earlier sample
/// reviews are removed, so the inbox holds one at most. Returns the
/// review's path.
pub(crate) fn create(root: &Path, inbox: &Path, stamp: &str) -> Result<PathBuf, String> {
    let repo = root.join("stacks");
    if repo.exists() {
        std::fs::remove_dir_all(&repo).map_err(|e| format!("clearing the old sample: {e}"))?;
    }
    std::fs::create_dir_all(&repo).map_err(|e| format!("creating {}: {e}", repo.display()))?;
    git(&repo, &["init", "-q"])?;
    write_files(&repo, BEFORE)?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "The catalogue as it was"])?;
    write_files(&repo, AFTER)?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "Move book covers into storage"])?;

    let repo_str = repo.to_string_lossy().to_string();
    let diff = crate::diff_at(&repo_str, false, Some(REVISION), None)?;
    let mut chapters: Vec<Chapter> = CHAPTERS
        .iter()
        .map(|(title, summary, _)| Chapter { title: (*title).into(), summary: (*summary).into(), hunks: vec![] })
        .collect();
    for f in &diff.files {
        let path = f.new_path.clone().or_else(|| f.old_path.clone()).unwrap_or_default();
        let at = CHAPTERS.iter().position(|(_, _, files)| files.contains(&path.as_str())).ok_or(format!("{path} is in no chapter"))?;
        for h in &f.hunks {
            chapters[at].hunks.push(HunkRef { file: path.clone(), header: h.header.clone() });
        }
    }

    std::fs::create_dir_all(inbox).map_err(|e| format!("creating {}: {e}", inbox.display()))?;
    if let Ok(entries) = std::fs::read_dir(inbox) {
        for e in entries.flatten() {
            if is_sample(&e.path()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let log = inbox.join(format!("{stamp}{SUFFIX}"));
    let rev = Some(REVISION.to_string());
    let append = |body: Body| threshold_core::append_event(&log, Author::Agent, rev.clone(), body).map_err(|e| format!("writing the sample review: {e}"));
    append(Body::Brief {
        title: "Sample: move book covers into storage".into(),
        summary: "A made-up change, for the tour. Book covers move from a folder on the server into a storage service, pages show a placeholder when a cover is missing, and a one-off command moves the existing files.".into(),
        implications: vec![
            "Irreversible: the command deletes each local cover once it is uploaded".into(),
            "Pages now depend on storage being reachable".into(),
            "I have not run the command against a full set of covers, which is worth a second look".into(),
        ],
        repo: Some(repo_str),
    })?;
    append(Body::Bundle { chapters })?;
    for (file, content, flag, text) in FLAGS {
        let text_after = AFTER.iter().find(|(p, _)| p == file).map(|(_, t)| *t).unwrap_or("");
        let line = text_after.lines().position(|l| l == *content).ok_or(format!("flagged line not in {file}"))? as u32 + 1;
        append(Body::Note {
            text: (*text).into(),
            file: Some((*file).into()),
            line: Some(line),
            line_content: Some((*content).into()),
            chapter: None,
            in_reply_to: None,
            flag: Some(*flag),
        })?;
    }
    // One thread already answered, so the tour can show a conversation.
    let (file, content, question, answer) = THREAD;
    let text_after = AFTER.iter().find(|(p, _)| *p == file).map(|(_, t)| *t).unwrap_or("");
    let line = text_after.lines().position(|l| l == content).ok_or(format!("thread line not in {file}"))? as u32 + 1;
    let asked = threshold_core::append_event(
        &log,
        Author::Human,
        rev.clone(),
        Body::Note { text: question.into(), file: Some(file.into()), line: Some(line), line_content: Some(content.into()), chapter: None, in_reply_to: None, flag: None },
    )
    .map_err(|e| format!("writing the sample review: {e}"))?;
    append(Body::Reply { text: answer.into(), in_reply_to: Some(asked.seq) })?;
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_is_a_complete_review_of_a_real_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("reviews");
        let log = create(&tmp.path().join("sample"), &inbox, "2026-01-01-120000").unwrap();
        assert!(is_sample(&log));

        let events = threshold_core::read_session(&log);
        let repo = crate::repo_of(&log).unwrap();
        let diff = crate::diff_at(&repo, false, Some(REVISION), None).unwrap();
        assert_eq!(diff.files.len(), 5);

        // Every hunk in a chapter, and nothing claimed that isn't there.
        let bundle = events.iter().rev().find_map(|e| match &e.body { Body::Bundle { chapters } => Some(chapters.clone()), _ => None }).unwrap();
        let cov = threshold_core::coverage(&diff.files, &bundle);
        assert!(cov.unbundled.is_empty() && cov.phantom.is_empty(), "{cov:?}");

        // One question from the reader, answered.
        let asked = events.iter().find(|e| e.from == Author::Human).unwrap();
        assert!(events.iter().any(|e| matches!(&e.body, Body::Reply { in_reply_to: Some(n), .. } if *n == asked.seq)));

        // Each flag and note points at the line it quotes.
        for e in &events {
            if let Body::Note { file: Some(f), line: Some(n), line_content: Some(c), .. } = &e.body {
                let text = std::fs::read_to_string(Path::new(&repo).join(f)).unwrap();
                assert_eq!(text.lines().nth(*n as usize - 1), Some(c.as_str()), "{f}:{n}");
            }
        }
    }

    #[test]
    fn a_second_tour_replaces_the_first_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let inbox = tmp.path().join("reviews");
        let first = create(&tmp.path().join("sample"), &inbox, "2026-01-01-120000").unwrap();
        let second = create(&tmp.path().join("sample"), &inbox, "2026-01-02-120000").unwrap();
        assert!(!first.exists());
        assert!(second.exists());
    }
}
