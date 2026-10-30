//! Tauri shell around `threshold-core`: run `git diff` in a chosen repo —
//! against the working tree, the index, or a revision such as
//! `main...HEAD` — parse and summarize it, and record an approve/reject
//! decision to a persisted log.
//!
//! A review resolves its revision to the two commits git would compare
//! and diffs those from then on, so what the human is reading cannot
//! change while they read it; a spec that names only one commit is
//! compared against the working tree and cannot be pinned, which is
//! reported rather than hidden. The verdict is appended to the review's
//! session log, which the agent is already watching.

mod icon;
mod mobile;
mod notify;
mod push;
mod sample;
mod skill;
mod server;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use threshold_core::{
    digest, parse_range, parse_unified_diff, summarize, validate_revision, Author, Body, Chapter,
    ChapterState, Coverage, Decision, DiffFile, Event, Mark, Pin, Range, ReviewSummary, DiffFacts,
    Verdict,
};

#[derive(Default)]
struct LaunchArgs {
    repo: Option<String>,
    /// A revision spec to diff against, e.g. `main...HEAD`. Lets an agent
    /// hand over work it has already committed, which is the normal shape
    /// of agentic work — otherwise the only reviewable thing is the
    /// working tree, and the agent would have to un-commit to be reviewed.
    revision: Option<String>,
    /// Path to the review session log — the append-only conversation
    /// between the agent and the human.
    session: Option<PathBuf>,
}

fn parse_launch_args() -> LaunchArgs {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = LaunchArgs::default();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--revision" {
            out.revision = args.get(i + 1).cloned();
            i += 2;
        } else if args[i] == "--session" {
            out.session = args.get(i + 1).map(PathBuf::from);
            i += 2;
        } else if out.repo.is_none() && !args[i].starts_with('-') {
            out.repo = Some(args[i].clone());
            i += 1;
        } else {
            i += 1;
        }
    }
    out
}

pub(crate) fn decisions_path(app: &AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("decisions.json")
}

#[derive(Serialize)]
pub(crate) struct DiffPayload {
    files: Vec<DiffFile>,
    summary: DiffFacts,
    /// What this diff was taken from, for display and for the decision
    /// log: `null` for the working tree, `"--cached"` for the index, or
    /// the revision spec.
    source: Option<String>,
    /// The two commits this diff was actually taken between, when the
    /// review is pinned. `None` means the diff includes the working tree
    /// and can therefore change while the reviewer is reading it — which
    /// the UI has to say out loud rather than imply by omission.
    pin: Option<Pin>,
    /// SHA-256 of the diff text on screen. Quoted by the verdict, so an
    /// approval names the change rather than the question that produced
    /// it.
    digest: String,
}

#[tauri::command]
fn initial_repo() -> Option<String> {
    parse_launch_args().repo
}

#[tauri::command]
fn initial_revision() -> Option<String> {
    parse_launch_args().revision
}

#[tauri::command]
async fn pick_repo_folder(app: AppHandle) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    rx.recv().ok().flatten().and_then(|p| p.into_path().ok()).map(|p| p.display().to_string())
}

/// The git Threshold runs: its own copy, bundled beside the executable, so
/// a Mac without Apple's Command Line Tools works. Builds that
/// don't carry one, such as tests, use git from PATH.
pub(crate) fn git_command() -> Command {
    static BUNDLED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    let bundled = BUNDLED.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let git = exe.parent()?.join("git");
        git.is_file().then_some(git)
    });
    match bundled {
        Some(git) => Command::new(git),
        None => Command::new("git"),
    }
}

/// What Threshold says when git can't run at all. On a Mac, `/usr/bin/git`
/// is a stub until Apple's Command Line Tools are installed.
const GIT_MISSING: &str = "Threshold needs git, which on a Mac comes with Apple's Command Line Tools. Install them by running xcode-select --install in Terminal, then try again.";

/// git's own error text, unless it's the macOS stub saying the Command
/// Line Tools aren't installed.
pub(crate) fn git_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.contains("xcrun: error") || text.contains("No developer tools were found") {
        GIT_MISSING.to_string()
    } else {
        text
    }
}

/// Why git couldn't be started.
pub(crate) fn git_spawn_error(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        GIT_MISSING.to_string()
    } else {
        format!("running git: {e}")
    }
}

/// Run `git` in `repo` and return its stdout, or its own error text.
///
/// git reports failures better than a reimplementation would, so its
/// stderr is passed straight back rather than being reinterpreted.
fn git(repo: &str, args: &[&str]) -> Result<String, String> {
    let output = git_command()
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(git_spawn_error)?;
    if !output.status.success() {
        return Err(git_error(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Resolve one side of a range to the commit it names.
///
/// `^{commit}` so an annotated tag resolves to the commit it points at
/// rather than to the tag object, which is not a thing `git diff` can
/// compare.
fn rev_parse_commit(repo: &str, rev: &str) -> Result<String, String> {
    let checked = validate_revision(rev).map_err(|e| e.to_string())?;
    let out = git(repo, &["rev-parse", "--verify", &format!("{checked}^{{commit}}")])?;
    let oid = out.trim().to_string();
    if !threshold_core::is_object_id(&oid) {
        return Err(format!("git resolved {checked} to something that is not a commit id"));
    }
    Ok(oid)
}

/// Resolve a revision spec to the two commits git would compare.
///
/// `Ok(None)` is the honest answer for a spec that names only one
/// commit: git compares it against the working tree, so half the diff is
/// whatever is on disk at the moment of rendering and no amount of
/// resolving makes it stand still. That case is reported to the reviewer
/// rather than papered over.
fn resolve_pin(repo: &str, spec: &str) -> Result<Option<Pin>, String> {
    match parse_range(spec) {
        Range::MergeBase { left, right } => {
            let l = validate_revision(&left).map_err(|e| e.to_string())?.to_string();
            let r = validate_revision(&right).map_err(|e| e.to_string())?.to_string();
            let out = git(repo, &["merge-base", &l, &r])?;
            let base = out.trim().to_string();
            if !threshold_core::is_object_id(&base) {
                return Err(format!("no common ancestor of {l} and {r}"));
            }
            Ok(Some(Pin { base, head: rev_parse_commit(repo, &r)? }))
        }
        Range::Endpoints { left, right } => Ok(Some(Pin {
            base: rev_parse_commit(repo, &left)?,
            head: rev_parse_commit(repo, &right)?,
        })),
        Range::WorkingTree { .. } => Ok(None),
    }
}

/// Diff the working tree, the index, a revision, or — when the review has
/// been pinned — the two commits that revision resolved to.
///
/// A pin wins over the spec that produced it. That is the whole point:
/// `main...HEAD` is a question whose answer moves, and once it has been
/// answered the review is about *that* answer. Re-asking would show the
/// reviewer a different change under the same heading, invalidate every
/// line annotation, and leave the verdict attached to neither.
///
/// `revision` wins over `staged` when both are given: asking for
/// `main...HEAD --cached` is not a thing, and silently honouring the
/// weaker of the two would show a diff the caller did not ask for.
///
/// The spec is validated before it goes near `git` (rejecting a leading
/// `-`, which git would read as an option — `--output=` writes a file),
/// and a `--` separator follows it so git cannot reinterpret it as a
/// path. Whether the revision actually *exists* is left to git, whose own
/// error is passed straight back: it handles ranges like `main...HEAD`
/// that `rev-parse --verify` rejects, and reports failures better than a
/// reimplementation would.
pub(crate) fn diff_at(path: &str, staged: bool, revision: Option<&str>, pin: Option<&Pin>) -> Result<DiffPayload, String> {
    let mut args: Vec<String> = vec!["diff".into()];
    let spec = revision.map(str::trim).filter(|r| !r.is_empty());

    let source = match (pin, spec) {
        // Pinned: two object ids, so this returns the same text however
        // the branch refs move.
        (Some(p), spec) if p.is_valid() => {
            args.push(p.base.clone());
            args.push(p.head.clone());
            args.push("--".into());
            spec.map(str::to_string)
        }
        (_, Some(spec)) => {
            let checked = validate_revision(spec).map_err(|e| e.to_string())?;
            args.push(checked.to_string());
            args.push("--".into());
            Some(checked.to_string())
        }
        (_, None) if staged => {
            args.push("--cached".into());
            Some("--cached".to_string())
        }
        (_, None) => None,
    };

    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    // Two object ids name one diff forever, so a pinned diff is asked of
    // git once. Opening a review on the phone takes it four or five times.
    let pinned = pin.filter(|p| p.is_valid()).map(|p| (path.to_string(), p.base.clone(), p.head.clone()));
    let cached = pinned.as_ref().and_then(|k| PINNED_TEXT.lock().unwrap().get(k).cloned());
    let text = match cached {
        Some(t) => t,
        None => {
            let t = git(path, &borrowed)?;
            if let Some(k) = pinned {
                PINNED_TEXT.lock().unwrap().insert(k, t.clone());
            }
            t
        }
    };
    let files = parse_unified_diff(&text);
    let summary = summarize(&files);
    Ok(DiffPayload {
        files,
        summary,
        source,
        pin: pin.filter(|p| p.is_valid()).cloned(),
        digest: digest(&text),
    })
}

/// Diff text by repo and pinned commits. Immutable, so never invalidated;
/// bounded by the reviews one person opens.
static PINNED_TEXT: std::sync::LazyLock<Mutex<HashMap<(String, String, String), String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Diffs between two pinned heads, for carrying notes across a revision.
/// Immutable for the same reason.
static BETWEEN: std::sync::LazyLock<Mutex<HashMap<(String, String, String), Vec<DiffFile>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// The pin in force for whatever the caller is asking to see.
///
/// Only ever the review's own pin: a revision the human typed into the
/// field to look around is not the review, and pretending it is pinned
/// would be a false reassurance.
fn pin_for(app: &AppHandle, revision: Option<&str>) -> Option<Pin> {
    let session = session_path_for(app)?;
    threshold_core::current_pin(&session, revision).map(|(p, _)| p)
}

#[tauri::command]
fn load_diff(app: AppHandle, path: String, staged: bool, revision: Option<String>) -> Result<DiffPayload, String> {
    let pin = pin_for(&app, revision.as_deref().map(str::trim).filter(|r| !r.is_empty()));
    diff_at(&path, staged, revision.as_deref(), pin.as_ref())
}

/// Largest file offered for expanding context. Past this it is almost
/// certainly generated or vendored, and shipping it to the UI to show
/// twenty lines is the wrong trade.
const MAX_CONTEXT_BYTES: usize = 4 * 1024 * 1024;

/// The new side of `file` as the diff on screen sees it, one entry per
/// line, so the UI can show the unchanged code around a hunk.
///
/// Taken from wherever that diff's new side lives: the pinned head, the
/// right-hand commit of an unpinned range, the index for a staged diff,
/// or the working tree. Never the working tree for a pinned review — the
/// context has to belong to the same commits as the hunks it surrounds.
pub(crate) fn file_text_at(repo: &str, staged: bool, revision: Option<&str>, pin: Option<&Pin>, file: &str) -> Result<Vec<String>, String> {
    if file.is_empty() || file.contains('\0') || file.contains('\n') {
        return Err("not a file path".into());
    }
    let from_git = |object: String| git(repo, &["show", "--no-color", &object]);
    let spec = revision.map(str::trim).filter(|r| !r.is_empty());
    let text = match (pin.filter(|p| p.is_valid()), spec) {
        // `<oid>:<path>` begins with the object id, so git cannot read it
        // as an option however the path is spelled.
        (Some(p), _) => from_git(format!("{}:{file}", p.head))?,
        (None, Some(spec)) => match parse_range(spec) {
            Range::MergeBase { right, .. } | Range::Endpoints { right, .. } => {
                let r = validate_revision(&right).map_err(|e| e.to_string())?.to_string();
                from_git(format!("{r}:{file}"))?
            }
            Range::WorkingTree { .. } => read_in_repo(repo, file)?,
        },
        (None, None) if staged => from_git(format!(":{file}"))?,
        (None, None) => read_in_repo(repo, file)?,
    };
    if text.len() > MAX_CONTEXT_BYTES {
        return Err("this file is too large to expand".into());
    }
    Ok(text.lines().map(str::to_string).collect())
}

/// Read `file` from the working tree, refusing any path that resolves
/// outside the repository.
fn read_in_repo(repo: &str, file: &str) -> Result<String, String> {
    let root = std::fs::canonicalize(repo).map_err(|e| e.to_string())?;
    let full = std::fs::canonicalize(root.join(file)).map_err(|e| e.to_string())?;
    if !full.starts_with(&root) {
        return Err("that path is outside the repository".into());
    }
    std::fs::read_to_string(full).map_err(|e| e.to_string())
}

/// Largest image shown. Past this it is not a figure somebody meant to be
/// read in a review.
const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// The media type of an image a webview can show, from its extension.
fn image_mime(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        // Shown through <img>, where an SVG cannot run script.
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

/// Like [`git`], for output that is not text.
fn git_bytes(repo: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = git_command().arg("-C").arg(repo).args(args).output().map_err(git_spawn_error)?;
    if !output.status.success() {
        return Err(git_error(&output.stderr));
    }
    Ok(output.stdout)
}

/// The bytes of `file` on one side of the diff on screen.
///
/// The new side comes from the same place [`file_text_at`] reads context
/// from. The old side is its counterpart: the pinned base, the left of an
/// unpinned range, HEAD for a staged diff, the index for a working-tree
/// diff, or the named commit when the working tree is compared with one.
fn file_bytes_at(repo: &str, staged: bool, revision: Option<&str>, pin: Option<&Pin>, file: &str, old: bool) -> Result<Vec<u8>, String> {
    if file.is_empty() || file.contains('\0') || file.contains('\n') {
        return Err("not a file path".into());
    }
    let show = |object: String| git_bytes(repo, &["show", "--no-color", &object]);
    let spec = revision.map(str::trim).filter(|r| !r.is_empty());
    let bytes = match (pin.filter(|p| p.is_valid()), spec, old) {
        (Some(p), _, false) => show(format!("{}:{file}", p.head))?,
        (Some(p), _, true) => show(format!("{}:{file}", p.base))?,
        (None, Some(spec), _) => match parse_range(spec) {
            Range::MergeBase { left, right } => {
                let l = validate_revision(&left).map_err(|e| e.to_string())?.to_string();
                let r = validate_revision(&right).map_err(|e| e.to_string())?.to_string();
                if old {
                    let base = git(repo, &["merge-base", &l, &r])?.trim().to_string();
                    show(format!("{base}:{file}"))?
                } else {
                    show(format!("{r}:{file}"))?
                }
            }
            Range::Endpoints { left, right } => {
                let side = if old { left } else { right };
                let s = validate_revision(&side).map_err(|e| e.to_string())?.to_string();
                show(format!("{s}:{file}"))?
            }
            Range::WorkingTree { .. } if old => {
                let s = validate_revision(spec).map_err(|e| e.to_string())?.to_string();
                show(format!("{s}:{file}"))?
            }
            Range::WorkingTree { .. } => read_bytes_in_repo(repo, file)?,
        },
        (None, None, true) if staged => show(format!("HEAD:{file}"))?,
        (None, None, true) => show(format!(":{file}"))?,
        (None, None, false) if staged => show(format!(":{file}"))?,
        (None, None, false) => read_bytes_in_repo(repo, file)?,
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("this file is too large to show".into());
    }
    Ok(bytes)
}

fn read_bytes_in_repo(repo: &str, file: &str) -> Result<Vec<u8>, String> {
    let root = std::fs::canonicalize(repo).map_err(|e| e.to_string())?;
    let full = std::fs::canonicalize(root.join(file)).map_err(|e| e.to_string())?;
    if !full.starts_with(&root) {
        return Err("that path is outside the repository".into());
    }
    std::fs::read(full).map_err(|e| e.to_string())
}

/// An image from one side of the diff on screen, as a `data:` URL, or an
/// error saying why not. Only for files an image viewer can show; any
/// other binary is described, never shipped to the UI.
pub(crate) fn image_at(repo: &str, staged: bool, revision: Option<&str>, pin: Option<&Pin>, file: &str, old: bool) -> Result<String, String> {
    use base64::Engine;
    let mime = image_mime(file).ok_or("not an image")?;
    let bytes = file_bytes_at(repo, staged, revision, pin, file, old)?;
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
fn file_image(app: AppHandle, path: String, staged: bool, revision: Option<String>, file: String, old: bool) -> Result<String, String> {
    let spec = revision.as_deref().map(str::trim).filter(|r| !r.is_empty());
    let pin = pin_for(&app, spec);
    image_at(&path, staged, spec, pin.as_ref(), &file, old)
}

#[tauri::command]
fn file_text(app: AppHandle, path: String, staged: bool, revision: Option<String>, file: String) -> Result<Vec<String>, String> {
    let spec = revision.as_deref().map(str::trim).filter(|r| !r.is_empty());
    let pin = pin_for(&app, spec);
    file_text_at(&path, staged, spec, pin.as_ref(), &file)
}

#[tauri::command]
fn list_decisions(app: AppHandle) -> Vec<Decision> {
    threshold_core::load_decisions(&decisions_path(&app))
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum VerdictArg {
    Approved,
    Rejected,
}

/// Record a verdict in the review's log and in the permanent decision log.
///
/// Shared by the desktop and the phone, so a verdict means the same thing
/// whichever screen gave it.
pub(crate) fn record_verdict(
    decisions: &Path,
    session: Option<&Path>,
    revision: Option<String>,
    summary: DiffFacts,
    verdict: Verdict,
    reason: Option<String>,
    source: Option<String>,
    digest: Option<String>,
) -> Result<Vec<Decision>, String> {
    // The verdict is the terminal event of the conversation, so it is
    // appended to the session log alongside the permanent decision log.
    // An agent watching the log therefore needs to watch only one thing
    // to learn the review ended.
    if let Some(path) = session {
        let _ = threshold_core::append_event(
            path,
            Author::Human,
            revision,
            // The digest of what was on screen, not of what git would
            // return now. A verdict is a statement about what the human
            // read, and if those two have diverged the agent must be
            // able to tell.
            Body::Verdict { verdict, reason: reason.clone(), digest: digest.clone() },
        );
    }
    // The tour's sample is practice: its verdict ends that review and
    // goes no further.
    if session.is_some_and(sample::is_sample) {
        return Ok(threshold_core::load_decisions(decisions));
    }
    let decision = Decision { summary, verdict, reason, decided_at_ms: threshold_core::now_ms(), source, digest };
    threshold_core::append_decision(decisions, decision).map_err(|e| e.to_string())
}

#[tauri::command]
fn decide(app: AppHandle, summary: DiffFacts, verdict: VerdictArg, reason: Option<String>, source: Option<String>, digest: Option<String>) -> Result<Vec<Decision>, String> {
    let verdict = match verdict {
        VerdictArg::Approved => Verdict::Approved,
        VerdictArg::Rejected => Verdict::Rejected,
    };
    let session = session_path_for(&app);
    let revision = app.state::<SessionState>().revision.lock().unwrap().clone();
    record_verdict(&decisions_path(&app), session.as_deref(), revision, summary, verdict, reason, source, digest)
}

/// The revision currently under review.
///
/// Held in app state rather than re-read from the launch arguments,
/// because the agent can move it mid-session with a `revised` event, and
/// every note the human writes afterwards must record the new one.
#[derive(Default)]
struct SessionState {
    revision: Mutex<Option<String>>,
    /// The review being read right now.
    ///
    /// Chosen at runtime from the inbox rather than fixed at launch: the
    /// app stays open and the human moves between reviews, so this is the
    /// one piece of state the whole UI hangs off.
    active: Mutex<Option<PathBuf>>,
    /// The digest of an *unpinned* review's diff, as of the last check.
    ///
    /// Only meaningful for a review that could not be pinned, which is
    /// the one case where the diff can move on its own. Held in memory
    /// rather than in the log because it is not a claim anyone makes —
    /// it is this process noticing something, and the thing worth
    /// recording is the noticing, not the reading that preceded it.
    live_digest: Mutex<Option<String>>,
}

pub(crate) fn inbox() -> PathBuf {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    let dir = threshold_core::inbox_dir(&home);
    // Created eagerly so an agent can write into it without having to
    // know whether Threshold has ever run.
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The session log currently being read: whatever the human selected,
/// else the one passed at launch.
fn session_path_for(app: &AppHandle) -> Option<PathBuf> {
    app.state::<SessionState>()
        .active
        .lock()
        .unwrap()
        .clone()
        .or_else(|| parse_launch_args().session)
}

/// The repo a session log is about, from its brief.
pub(crate) fn repo_of(path: &Path) -> Option<String> {
    threshold_core::read_session(path).into_iter().find_map(|e| match e.body {
        Body::Brief { repo, .. } => repo,
        _ => None,
    })
}

/// Notice that an unpinned review's diff has changed under the reviewer.
///
/// A pinned review cannot do this, which is the point of pinning; this
/// is the consolation prize for the reviews that could not be pinned.
/// Detection after the fact is worth much less than prevention, but it
/// is worth a great deal more than the alternative, which is a reviewer
/// approving a change they last saw ten minutes ago.
///
/// The diagnostic is recorded once, for the agent; the emitted event
/// fires on every change, for the reviewer, who needs to know that what
/// is on their screen is no longer what is on disk. Deliberately no
/// automatic reload: the diff moving is not a reason to throw away where
/// they had read to, or a note they were halfway through writing.
fn check_drift(app: &AppHandle) {
    let Some(path) = session_path_for(app) else { return };
    let revision = app.state::<SessionState>().revision.lock().unwrap().clone();
    if threshold_core::current_pin(&path, revision.as_deref()).is_some() {
        return;
    }
    let Some(repo) = repo_of(&path) else { return };
    let Ok(payload) = diff_at(&repo, false, revision.as_deref(), None) else { return };

    let st = app.state::<SessionState>();
    let mut seen = st.live_digest.lock().unwrap();
    let changed = seen.as_deref().is_some_and(|d| d != payload.digest);
    *seen = Some(payload.digest.clone());
    drop(seen);

    if !changed {
        return;
    }
    let _ = threshold_core::diagnose(
        &path,
        revision,
        "diff_drifted",
        "The diff under review changed while it was being read. This review is not pinned to a pair of commits, so nothing prevents that; anything the reviewer has already read, marked or annotated was about the earlier version.".into(),
        vec![],
    );
    let _ = app.emit("diff-drifted", payload.digest);
}

/// Pin the review to the commits its revision spec names, once.
///
/// The pin is what makes a review a review rather than a standing query.
/// Until it exists, every render re-asks `main...HEAD` and every answer
/// may differ: the reviewer's line annotations re-anchor to whatever now
/// occupies that line number, the narrative describes code that has
/// moved, and the verdict names a spec instead of a change.
///
/// Returns `None` when the review cannot be pinned, having said so in
/// the log. Deliberately not fatal: a working-tree review is a real
/// thing a person may want to read, and refusing to render it would
/// punish the reviewer for the agent's choice of handover. Diagnose
/// loudly, decide freely — the same rule the rest of the checks follow.
fn ensure_pinned(path: &PathBuf, repo: &str, revision: Option<String>) -> Option<Pin> {
    if let Some((pin, _)) = threshold_core::current_pin(path, revision.as_deref()) {
        return Some(pin);
    }

    let unpinned = |detail: Vec<String>| {
        let _ = threshold_core::diagnose(
            path,
            revision.clone(),
            "review_unpinned",
            "This review is not pinned to a pair of commits, so its diff includes the working tree and can change while it is being read. Commit the work and hand over a two-commit range such as `main...HEAD`.".into(),
            detail,
        );
        None
    };

    let Some(spec) = revision.as_deref().map(str::trim).filter(|r| !r.is_empty()) else {
        return unpinned(vec!["This review has no revision; it is a diff of the working tree.".into()]);
    };

    let pin = match resolve_pin(repo, spec) {
        Ok(Some(pin)) => pin,
        Ok(None) => {
            return unpinned(vec![format!(
                "`{spec}` names one commit, which git compares against the working tree rather than against another commit."
            )])
        }
        Err(e) => {
            let _ = threshold_core::diagnose(
                path,
                revision.clone(),
                "pin_failed",
                format!("git could not resolve `{spec}` to a pair of commits, so this review cannot be pinned: {e}"),
                vec![],
            );
            return None;
        }
    };

    // Recorded with the digest of what it produces, so a verdict can
    // quote content and the agent can check it against its own
    // `git diff | shasum -a 256` without trusting anything here.
    let Ok(payload) = diff_at(repo, false, Some(spec), Some(&pin)) else {
        return None;
    };
    let _ = threshold_core::append_event(
        path,
        Author::Threshold,
        revision,
        Body::Pinned { base: pin.base.clone(), head: pin.head.clone(), digest: payload.digest },
    );
    Some(pin)
}

/// Held for the whole of a check.
///
/// The watcher and `open_review` both run checks, on different threads,
/// and a check decides whether to pin or to diagnose by reading the log
/// before appending to it. Interleaved, each sees the other's write as
/// missing and the review is pinned, and diagnosed, twice.
static CHECKING: Mutex<()> = Mutex::new(());

/// How long a review's log must sit still before the background check
/// reads it.
///
/// An agent pushes a brief, a bundle and its flags as a burst, and a
/// check that lands between two of them reports on half a review — a
/// brief whose revision is only named on the bundle reads as
/// `review_unpinned`. Waiting also keeps Threshold's own lines out of the
/// middle of the burst, where they would take a `seq` the agent is about
/// to write.
const SETTLE: Duration = Duration::from_secs(3);

/// Check every waiting review in `dir` that has changed since it was last
/// checked, whether or not anyone has opened it.
///
/// The agent is tailing the log as soon as it pushes, and the checks are
/// written for it: an incomplete bundle or an unpinnable revision is
/// something it can fix before the human arrives. Checking only on open
/// meant it heard about them once somebody had already started reading.
/// Pinning belongs here for the same reason — the commits a review is
/// about are the ones handed over, not whatever the branch points at by
/// the time it is opened.
///
/// `checked` maps a review to its event count as of its last check.
/// Returns whether anything was checked, so the caller knows the inbox
/// may have changed underneath its fingerprint.
fn check_inbox(dir: &Path, checked: &mut HashMap<PathBuf, usize>, settle: Duration) -> bool {
    let mut any = false;
    for r in threshold_core::list_reviews(dir) {
        // Decided: nobody is waiting on the answer. Empty: the agent has
        // created the file and not yet written the brief, and reporting
        // `repo_missing` for that would be a false alarm.
        if r.decided || r.events == 0 || checked.get(&r.path) == Some(&r.events) {
            continue;
        }
        let Ok(modified) = std::fs::metadata(&r.path).and_then(|m| m.modified()) else {
            continue;
        };
        if modified.elapsed().unwrap_or_default() < settle {
            continue;
        }
        run_checks(&r.path, threshold_core::current_revision(&threshold_core::read_session(&r.path)));
        // Counted after the checks, so what they just wrote does not read
        // as the agent having changed something.
        checked.insert(r.path.clone(), threshold_core::read_session(&r.path).len());
        any = true;
    }
    any
}

/// Check a pushed review and report anything wrong back into its log.
///
/// The agent is already tailing the log for notes and a verdict, so this
/// reaches it with no second mechanism. Nothing here blocks the review:
/// the human may reasonably approve after reading Files, and withholding
/// a diff because the agent's narrative is untidy punishes the wrong
/// party. Diagnose loudly, decide freely.
pub(crate) fn run_checks(path: &PathBuf, revision: Option<String>) {
    let _checking = CHECKING.lock().unwrap_or_else(|e| e.into_inner());
    let events = threshold_core::read_session(path);
    let brief = events.iter().find_map(|e| match &e.body {
        Body::Brief { repo, .. } => Some(repo.clone()),
        _ => None,
    });

    let Some(repo) = brief.flatten() else {
        // Worse than an untidy narrative: without a repo the review
        // cannot be rendered at all, and the human has no way to know who
        // to tell.
        let _ = threshold_core::diagnose(
            path,
            revision,
            "repo_missing",
            "This review has no `repo` on its brief, so the diff cannot be loaded. Add an absolute repo path and push again.".into(),
            vec![],
        );
        return;
    };

    if !PathBuf::from(&repo).join(".git").exists() {
        let _ = threshold_core::diagnose(
            path,
            revision,
            "repo_not_a_git_repo",
            format!("`{repo}` is not a git repository, so the diff cannot be loaded."),
            vec![],
        );
        return;
    }

    // Pin before anything reads the diff, so every check below — and the
    // render that follows — is about one fixed pair of commits rather
    // than about whatever `main...HEAD` happens to mean right now.
    let pin = ensure_pinned(path, &repo, revision.clone());

    // Does the revision actually resolve? A review whose diff errors is a
    // dead end for the human.
    let payload = diff_at(&repo, false, revision.as_deref(), pin.as_ref());
    let files = match payload {
        Ok(p) => p.files,
        Err(e) if pin.is_some() => {
            // The pinned commits were resolvable when the review was
            // pinned and are not now: amended away, or pruned. Saying
            // "git could not produce this diff" would send the agent
            // looking at its revision spec, which is fine — the commits
            // behind it are not.
            let _ = threshold_core::diagnose(
                path,
                revision,
                "pin_unreachable",
                format!(
                    "The commits this review was pinned to are no longer in the repository, so its diff cannot be produced: {e}. Push a `revised` event to re-pin it."
                ),
                vec![],
            );
            return;
        }
        Err(e) => {
            let _ = threshold_core::diagnose(
                path,
                revision,
                "revision_unreadable",
                format!("git could not produce this diff: {e}"),
                vec![],
            );
            return;
        }
    };

    if files.is_empty() {
        let _ = threshold_core::diagnose(
            path,
            revision,
            "diff_empty",
            "This revision produces no changes, so there is nothing to review.".into(),
            vec![],
        );
        return;
    }

    // Narrative coverage: the reason this whole channel exists.
    if let Some(chapters) = threshold_core::current_bundle(path, revision.as_deref()) {
        let cov = threshold_core::coverage(&files, &chapters);
        if !cov.unbundled.is_empty() {
            let _ = threshold_core::diagnose(
                path,
                revision.clone(),
                "bundle_incomplete",
                format!(
                    "{} of {} hunks are not in any chapter. The reviewer sees them in an \"unbundled\" section and the narrative does not open by default. Push a corrected bundle covering every hunk.",
                    cov.unbundled.len(),
                    cov.total
                ),
                cov.unbundled.iter().map(|h| format!("{} {}", h.file, h.header)).collect(),
            );
        }
        if !cov.phantom.is_empty() {
            let _ = threshold_core::diagnose(
                path,
                revision,
                "bundle_phantom_hunks",
                format!(
                    "{} hunk reference{} in the bundle are not in this diff, so the narrative describes a different revision.",
                    cov.phantom.len(),
                    if cov.phantom.len() == 1 { "" } else { "s" }
                ),
                cov.phantom.iter().map(|h| format!("{} {}", h.file, h.header)).collect(),
            );
        }
    }
}

/// Where the reviewer was: which review, chapter, mode and scroll.
///
/// Deliberately not in the session log. That log is the conversation
/// between agent and human, and where somebody had scrolled is neither —
/// it is per-user UI state, so it lives beside the decision log instead.
#[derive(Default, Serialize, Deserialize)]
struct Place {
    #[serde(default)]
    review: Option<String>,
    /// Scroll offset per review path, then per chapter index.
    #[serde(default)]
    scroll: std::collections::HashMap<String, std::collections::HashMap<String, f64>>,
    #[serde(default)]
    chapter: std::collections::HashMap<String, usize>,
    #[serde(default)]
    mode: std::collections::HashMap<String, String>,
    /// Per review, per thread root: the highest seq the reviewer has seen.
    /// What makes a reply unread, and it has to survive a restart or every
    /// old answer reads as new again.
    #[serde(default)]
    read: std::collections::HashMap<String, std::collections::HashMap<String, u64>>,
    /// How the review list is arranged: `"date"` or `"project"`.
    #[serde(default)]
    inbox_group: Option<String>,
    /// The one project the list is narrowed to, by repo path, if any.
    #[serde(default)]
    inbox_project: Option<String>,
    /// Markdown files the reviewer switched to the diff, by
    /// `review|path`. Absent means rendered.
    #[serde(default)]
    markdown: std::collections::HashMap<String, String>,
    /// Everything else about how the window was left: which screen, which
    /// panes and toggles were open. Opaque here and shaped by the UI, so a
    /// new toggle is remembered without a change on this side.
    #[serde(default)]
    ui: serde_json::Value,
}

fn place_path(app: &AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("place.json")
}

/// A missing or corrupt file means starting at the top, never a broken
/// review: losing your scroll position is not worth failing over.
#[tauri::command]
fn load_place(app: AppHandle) -> Place {
    std::fs::read_to_string(place_path(&app))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn save_place(app: AppHandle, place: Place) {
    if let Ok(text) = serde_json::to_string(&place) {
        let _ = std::fs::write(place_path(&app), text);
    }
}

/// Reviews waiting and decided, newest first.
#[tauri::command]
fn list_reviews() -> Vec<ReviewSummary> {
    threshold_core::list_reviews(&inbox())
}

/// How big a review's diff is: what the reviewer budgets time by.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
struct DiffSize {
    files: usize,
    added: usize,
    removed: usize,
}

/// Sizes of pinned diffs, by repo and the two commits. Two object ids name
/// the same diff forever, so these never need recomputing.
static PINNED_SIZES: std::sync::LazyLock<Mutex<HashMap<(String, String, String), DiffSize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// The size of the diff a review renders: taken from the same `diff_at`
/// the review pane loads, with the same pin, so the inbox and the open
/// review can never disagree about it.
fn size_of_review(review: &ReviewSummary) -> Option<DiffSize> {
    let repo = review.repo.as_deref()?;
    let revision = review.revision.as_deref().map(str::trim).filter(|r| !r.is_empty());
    let pin = threshold_core::current_pin(&review.path, revision).map(|(p, _)| p);
    let key = pin.as_ref().filter(|p| p.is_valid()).map(|p| (repo.to_string(), p.base.clone(), p.head.clone()));
    if let Some(size) = key.as_ref().and_then(|k| PINNED_SIZES.lock().unwrap().get(k).copied()) {
        return Some(size);
    }
    let summary = diff_at(repo, false, revision, pin.as_ref()).ok()?.summary;
    let size = DiffSize { files: summary.files_changed, added: summary.lines_added, removed: summary.lines_removed };
    if let Some(k) = key {
        PINNED_SIZES.lock().unwrap().insert(k, size);
    }
    Some(size)
}

/// Diff sizes for every review still waiting, by path. Decided and
/// archived reviews are left out: nobody is budgeting time for them.
///
/// Separate from `list_reviews` and off the UI thread, because it runs
/// `git diff` once per unpinned review and the list must stay instant.
#[tauri::command]
async fn review_sizes() -> HashMap<PathBuf, DiffSize> {
    tauri::async_runtime::spawn_blocking(|| sizes_in(&inbox())).await.unwrap_or_default()
}

fn sizes_in(dir: &Path) -> HashMap<PathBuf, DiffSize> {
    threshold_core::list_reviews(dir)
        .into_iter()
        .filter(|r| !r.decided && !r.archived)
        .filter_map(|r| size_of_review(&r).map(|s| (r.path, s)))
        .collect()
}

/// Archive a review without deciding it, or bring it back. Returns the
/// list as it now stands.
///
/// Only a review in the inbox: the path comes from the UI, and the archive
/// is a fact about this directory, so anything outside it is refused
/// rather than recorded against a file name it happens to share.
#[tauri::command]
fn set_archived(app: AppHandle, path: String, archived: bool) -> Result<Vec<ReviewSummary>, String> {
    let dir = inbox();
    let p = PathBuf::from(&path);
    if p.parent() != Some(dir.as_path()) {
        return Err("only a review in the inbox can be archived".into());
    }
    threshold_core::set_archived(&dir, &p, archived).map_err(|e| format!("writing the archive: {e}"))?;
    refresh_badge(&app);
    Ok(threshold_core::list_reviews(&dir))
}

/// The Dock badge: reviews waiting on a decision and not archived.
fn refresh_badge(app: &AppHandle) {
    let waiting = threshold_core::waiting(&inbox());
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if waiting == 0 { None } else { Some(waiting as i64) });
    }
}

/// Switch to a review. Returns its events so the UI can render at once.
#[tauri::command]
fn open_review(app: AppHandle, path: String) -> Result<Vec<Event>, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("no review at {path}"));
    }
    let events = threshold_core::read_session(&p);
    // Adopt the review's own revision: the app is not restarted between
    // reviews, so nothing else would move it off the previous one.
    let revision = threshold_core::current_revision(&events);
    *app.state::<SessionState>().revision.lock().unwrap() = revision.clone();
    *app.state::<SessionState>().active.lock().unwrap() = Some(p.clone());
    // Another review's diff is not this one drifting. Without clearing
    // it, the first drift check after every switch reports a change that
    // is only the reviewer having moved.
    *app.state::<SessionState>().live_digest.lock().unwrap() = None;
    run_checks(&p, revision);
    // Re-read: the checks may have appended diagnostics the UI should
    // show immediately rather than on the next poll.
    Ok(threshold_core::read_session(&p))
}

/// The repo a review is about, taken from its brief.
#[tauri::command]
fn review_repo(app: AppHandle) -> Option<String> {
    let path = session_path_for(&app)?;
    threshold_core::read_session(&path).into_iter().find_map(|e| match e.body {
        Body::Brief { repo, .. } => repo,
        _ => None,
    })
}

/// The narrative for the diff currently on screen, with the audit of how
/// completely it accounts for that diff.
///
/// Coverage travels with the chapters rather than being computed in the
/// UI: whether a narrative hides a hunk is a property of the data, and
/// the one place it must not be possible to get wrong is the place that
/// decides whether to warn.
#[derive(Serialize)]
pub(crate) struct BundlePayload {
    chapters: Vec<Chapter>,
    coverage: Coverage,
}

#[tauri::command]
fn load_bundle(app: AppHandle, path: String, staged: bool, revision: Option<String>) -> Option<BundlePayload> {
    let session = session_path_for(&app)?;
    let current = app.state::<SessionState>().revision.lock().unwrap().clone();
    let chapters = threshold_core::current_bundle(&session, current.as_deref())?;
    // Audited against the same diff the UI is rendering, not against
    // whatever the bundle claims to describe.
    let pin = pin_for(&app, revision.as_deref().map(str::trim).filter(|r| !r.is_empty()));
    let payload = diff_at(&path, staged, revision.as_deref(), pin.as_ref()).ok()?;
    let coverage = threshold_core::coverage(&payload.files, &chapters);
    Some(BundlePayload { chapters, coverage })
}

/// A review's narrative and its coverage, audited against the diff that
/// review renders at `revision`.
pub(crate) fn bundle_at(path: &Path, revision: Option<&str>) -> Option<BundlePayload> {
    let chapters = threshold_core::current_bundle(path, revision)?;
    let (_, _, _, diff) = rendered_at(path, revision)?;
    let coverage = threshold_core::coverage(&diff, &chapters);
    Some(BundlePayload { chapters, coverage })
}

#[tauri::command]
fn session_events(app: AppHandle) -> Vec<Event> {
    session_path_for(&app).map(|p| threshold_core::read_session(&p)).unwrap_or_default()
}

/// Whether this run is a conversation at all. The UI hides the whole
/// thread affordance when Threshold was opened by hand rather than handed
/// a session by an agent.
#[tauri::command]
fn has_session(app: AppHandle) -> bool {
    session_path_for(&app).is_some()
}

/// Path of the review being read, so the UI can name it and offer a way
/// back. A review launched with --session is just as "open" as one picked
/// from the list, and was previously indistinguishable from none at all.
#[tauri::command]
fn active_review(app: AppHandle) -> Option<String> {
    session_path_for(&app).map(|p| p.display().to_string())
}

/// Post a human note — a question or a line annotation — without deciding.
///
/// This is the point of the feature: saying something that is neither
/// approval nor rejection.
#[tauri::command]
fn post_note(
    app: AppHandle,
    text: String,
    file: Option<String>,
    line: Option<u32>,
    line_content: Option<String>,
    chapter: Option<String>,
    in_reply_to: Option<u64>,
) -> Result<Vec<Event>, String> {
    let path = session_path_for(&app).ok_or("this review has no session log to post into")?;
    let revision = app.state::<SessionState>().revision.lock().unwrap().clone();
    append_note(&path, revision, NoteArgs { text, file, line, line_content, chapter, in_reply_to })
}

/// What a human note says and where it is anchored.
#[derive(Deserialize)]
pub(crate) struct NoteArgs {
    pub text: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub line_content: Option<String>,
    #[serde(default)]
    pub chapter: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<u64>,
}

pub(crate) fn append_note(path: &Path, revision: Option<String>, note: NoteArgs) -> Result<Vec<Event>, String> {
    let NoteArgs { text, file, line, line_content, chapter, in_reply_to } = note;
    if text.trim().is_empty() {
        return Err("a note needs some text".into());
    }
    threshold_core::append_event(
        path,
        Author::Human,
        revision,
        // The reviewer never sets a flag: it marks what Claude wanted
        // looked at, and a human note is already the thing being looked at.
        Body::Note { text, file, line, line_content, chapter, in_reply_to, flag: None },
    )
    .map_err(|e| format!("writing to the session log: {e}"))?;
    Ok(threshold_core::read_session(path))
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum MarkArg {
    Ok,
    Concern,
    Clear,
}

/// Record that the reviewer has been through a chapter.
///
/// Deliberately separate from `decide`: this is progress, not a verdict.
#[tauri::command]
fn mark_chapter(app: AppHandle, chapter: String, mark: MarkArg) -> Result<Vec<Event>, String> {
    let path = session_path_for(&app).ok_or("this review has no session log")?;
    let revision = app.state::<SessionState>().revision.lock().unwrap().clone();
    append_mark(&path, revision, chapter, mark)
}

pub(crate) fn append_mark(path: &Path, revision: Option<String>, chapter: String, mark: MarkArg) -> Result<Vec<Event>, String> {
    let mark = match mark {
        MarkArg::Ok => Mark::Ok,
        MarkArg::Concern => Mark::Concern,
        MarkArg::Clear => Mark::Clear,
    };
    threshold_core::append_event(path, Author::Human, revision, Body::ChapterMark { chapter, mark })
        .map_err(|e| format!("writing to the session log: {e}"))?;
    Ok(threshold_core::read_session(path))
}

/// Where every line thread sits in the diff being rendered.
///
/// Notes are carried from the commits they were written against to the
/// ones pinned now, by git's own diff between the two heads; see
/// `threshold_core::resolve_anchors`. Only object ids reach git here — both
/// come from `pinned` events Threshold wrote itself, and are checked again.
pub(crate) fn anchors_for(repo: &str, events: &[Event], pin: Option<&Pin>, diff: &[DiffFile]) -> Vec<threshold_core::Anchor> {
    let mut between = |from: &str, to: &str| {
        if !threshold_core::is_object_id(from) || !threshold_core::is_object_id(to) {
            return None;
        }
        let key = (repo.to_string(), from.to_string(), to.to_string());
        if let Some(d) = BETWEEN.lock().unwrap().get(&key) {
            return Some(d.clone());
        }
        let d = git(repo, &["diff", "-M", "-U0", "--no-color", from, to, "--"]).ok().map(|t| parse_unified_diff(&t))?;
        BETWEEN.lock().unwrap().insert(key, d.clone());
        Some(d)
    };
    threshold_core::resolve_anchors(events, pin, diff, &mut between)
}

/// The review's events, pin and rendered diff, as the UI is showing them.
fn rendered(app: &AppHandle) -> Option<(String, Vec<Event>, Option<Pin>, Vec<DiffFile>)> {
    let path = session_path_for(app)?;
    let revision = app.state::<SessionState>().revision.lock().unwrap().clone();
    rendered_at(&path, revision.as_deref())
}

/// A review's events, pin and diff at `revision`.
pub(crate) fn rendered_at(path: &Path, revision: Option<&str>) -> Option<(String, Vec<Event>, Option<Pin>, Vec<DiffFile>)> {
    let repo = repo_of(path)?;
    let events = threshold_core::read_session(path);
    let pin = threshold_core::current_pin(path, revision).map(|(p, _)| p);
    let diff = diff_at(&repo, false, revision, pin.as_ref()).ok()?.files;
    Some((repo, events, pin, diff))
}

#[tauri::command]
fn line_anchors(app: AppHandle) -> Vec<threshold_core::Anchor> {
    rendered(&app).map(|(repo, events, pin, diff)| anchors_for(&repo, &events, pin.as_ref(), &diff)).unwrap_or_default()
}

/// Per-chapter state, including how many lines Claude flagged in the
/// files that chapter covers.
///
/// `files` comes from the UI because only the bundle knows which files a
/// chapter is made of; the log records a flag's file and nothing about
/// chapters.
#[tauri::command]
fn chapter_states(app: AppHandle, chapters: Vec<String>, files: Vec<Vec<String>>) -> Vec<ChapterState> {
    let Some(path) = session_path_for(&app) else {
        let empty = || ChapterState { mark: None, notes: 0, unanswered: 0, attention: 0, caution: 0, context: 0 };
        return chapters.iter().map(|_| empty()).collect();
    };
    let revision = app.state::<SessionState>().revision.lock().unwrap().clone();
    chapter_states_at(&path, revision.as_deref(), &chapters, &files)
}

pub(crate) fn chapter_states_at(path: &Path, revision: Option<&str>, chapters: &[String], files: &[Vec<String>]) -> Vec<ChapterState> {
    let events = threshold_core::read_session(path);
    // Flags are counted where they are drawn, which is where the anchors
    // put them: a contents list promising a mark the gutter does not show
    // sends the reviewer hunting for it.
    let anchors = rendered_at(path, revision).map(|(repo, events, pin, diff)| anchors_for(&repo, &events, pin.as_ref(), &diff)).unwrap_or_default();
    chapters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut st = threshold_core::chapter_state(&events, c);
            let (a, caution, ctx) = threshold_core::flag_counts(&events, &anchors, files.get(i).map(Vec::as_slice).unwrap_or(&[]));
            st.attention = a;
            st.caution = caution;
            st.context = ctx;
            st
        })
        .collect()
}

/// Watch both the inbox and the review being read, so a pushed review
/// appears and an agent's replies arrive without the human doing
/// anything.
///
/// Polled rather than using a filesystem-notification crate: these are
/// small files, a second of latency is imperceptible, and the transport
/// is already a file being appended to. Watching runs for the life of the
/// app rather than for one review, because the app now outlives any
/// single review — and keeps running whether or not the agent that wrote
/// a review is still alive.
fn watch(app: AppHandle) {
    let mut seen_seq = 0u64;
    let mut seen_inbox = String::new();
    // Per review, how many events it had: so a phone reading any review,
    // not only the one open on the desk, hears that it changed.
    let mut seen_counts: HashMap<PathBuf, usize> = HashMap::new();
    // What has been pushed to phones. Loaded once phone access is up,
    // which the setup hook starts after this thread: checked for on every
    // round rather than once here, or it is never seen.
    let data = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut announced: Option<push::Announced> = None;
    let mut checked = HashMap::new();
    let mut tick = 0u64;
    loop {
        std::thread::sleep(Duration::from_millis(700));
        tick += 1;

        // Every tenth tick rather than every one: this runs `git diff`,
        // which the other two checks do not, and seven seconds of
        // latency on a warning is imperceptible next to the time it
        // takes to read a diff.
        if tick % 10 == 0 {
            check_drift(&app);
        }

        // Before the fingerprint, so what the checks write reaches the
        // list on this tick rather than the next.
        check_inbox(&inbox(), &mut checked, SETTLE);

        // Wake paired phones when Claude has said something new. Checked
        // every few ticks: it reads every waiting log, and a push a couple
        // of seconds later than it could have been is imperceptible.
        if tick % 3 == 0 {
            if announced.is_none() && app.try_state::<std::sync::Arc<push::Push>>().is_some() {
                announced = Some(push::Announced::load(&data, &inbox()));
            }
            if let (Some(a), Some(push)) = (announced.as_mut(), app.try_state::<std::sync::Arc<push::Push>>()) {
                let due = a.due(&inbox(), SETTLE);
                if !due.is_empty() && !push.subscriptions().is_empty() {
                    let push = push.inner().clone();
                    let subject = app
                        .try_state::<std::sync::Arc<mobile::Mobile>>()
                        .and_then(|m| m.config().url.clone())
                        .unwrap_or_else(|| "mailto:threshold@localhost".into());
                    tauri::async_runtime::spawn(async move {
                        // One per review, each quoting what Claude said
                        // there; the phone keeps one notification per
                        // review.
                        for notice in due.iter().take(5) {
                            push.send_all(&subject, notice).await;
                        }
                    });
                }
            }
        }

        // A cheap fingerprint of the inbox: enough to notice a review
        // arriving, being decided, or gaining events.
        let reviews = threshold_core::list_reviews(&inbox());
        let fingerprint: String = reviews
            .iter()
            .map(|r| format!("{}:{}:{}:{};", r.path.display(), r.events, r.decided, r.archived))
            .collect();
        for r in &reviews {
            if seen_counts.insert(r.path.clone(), r.events) != Some(r.events) {
                if let Some(name) = r.path.file_name() {
                    server::publish(server::Pushed::SessionChanged { review: name.to_string_lossy().into_owned() });
                }
            }
        }
        if fingerprint != seen_inbox {
            server::publish(server::Pushed::InboxChanged);
            seen_inbox = fingerprint;
            // On the Dock icon, so the count is legible while the app sits
            // in the background — which is where it is meant to sit.
            refresh_badge(&app);
            let _ = app.emit("inbox-changed", threshold_core::waiting(&inbox()));
        }

        let Some(path) = session_path_for(&app) else {
            seen_seq = 0;
            continue;
        };
        let latest = threshold_core::last_seq(&path);
        if latest == seen_seq {
            continue;
        }
        seen_seq = latest;
        if let Some(to) = threshold_core::current_revision(&threshold_core::read_session(&path)) {
            *app.state::<SessionState>().revision.lock().unwrap() = Some(to);
        }
        // A revised review gets re-checked: the agent may have fixed one
        // problem and introduced another, and its bundle is invalidated
        // by the revision anyway. At once rather than after SETTLE, since
        // somebody is reading this one.
        run_checks(&path, app.state::<SessionState>().revision.lock().unwrap().clone());
        checked.insert(path.clone(), threshold_core::read_session(&path).len());
        let _ = app.emit("session-changed", latest);
    }
}

/// Serve the phone, if setup has been run and the machine has a name.
///
/// Off unless both hold. A server that cannot say which Mac it is would
/// let a phone driving two of them act on the wrong one, so an enabled
/// config with no name serves nothing and says why.
fn start_phone_server(app: &AppHandle) {
    let dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mobile = std::sync::Arc::new(mobile::Mobile::load(&dir));
    app.manage(mobile.clone());
    let config = mobile.config();
    if !config.enabled {
        return;
    }
    if mobile.machine().is_none() {
        eprintln!("threshold: phone access is enabled but this machine has no name; run scripts/threshold-mobile-setup.sh");
        return;
    }
    let port = config.port();
    let push = std::sync::Arc::new(push::Push::load(&dir));
    app.manage(push.clone());
    let ctx = server::Ctx { inbox: inbox(), decisions: decisions_path(app), mobile, push };
    tauri::async_runtime::spawn(async move {
        if let Err(e) = server::serve(ctx, port).await {
            eprintln!("threshold: phone server on 127.0.0.1:{port}: {e}");
        }
    });
}

/// The skill this build ships, for an agent to learn the handover from.
const SKILL: &str = include_str!("../../../skills/threshold-review/SKILL.md");

#[tauri::command]
fn skill_status() -> skill::SkillStatus {
    skill::status(SKILL, &skill::destination())
}

#[tauri::command]
fn install_skill(replace: bool) -> Result<skill::SkillStatus, String> {
    skill::install(SKILL, &skill::destination(), replace)
}

/// Show the skill in Finder, for another agent or for doing it by hand:
/// a copy in Threshold's own folder, so nothing outside it is touched.
#[tauri::command]
fn reveal_skill() -> Result<(), String> {
    let dir = inbox().parent().map(|p| p.join("threshold-review")).ok_or("no home folder")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("SKILL.md");
    std::fs::write(&file, SKILL).map_err(|e| e.to_string())?;
    Command::new("open").arg("-R").arg(&file).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Build the tour's sample repo and put a review of it in the inbox.
/// Returns the review's path.
#[tauri::command]
fn start_sample(app: AppHandle) -> Result<String, String> {
    let inbox = inbox();
    let root = inbox.parent().map(|p| p.join("sample")).ok_or("no home folder")?;
    let stamp = Command::new("date").arg("+%Y-%m-%d-%H%M%S").output().map_err(|e| e.to_string())?;
    let stamp = String::from_utf8_lossy(&stamp.stdout).trim().to_string();
    let log = sample::create(&root, &inbox, &stamp)?;
    // Pinned now rather than when the background check gets to it, so
    // the tour never opens on a "not pinned" warning.
    if let Some(repo) = repo_of(&log) {
        ensure_pinned(&log, &repo, threshold_core::current_revision(&threshold_core::read_session(&log)));
    }
    refresh_badge(&app);
    Ok(log.to_string_lossy().to_string())
}

/// A newer build of Threshold, built and waiting beside this one.
///
/// scripts/auto-install.sh builds after every commit, and stages the build
/// rather than quitting the app while somebody is using it. This is how
/// the app hears about it, so the person can restart into it when it
/// suits them rather than be left on an old build without knowing.
#[derive(Serialize)]
struct Update {
    commit: String,
    subject: String,
}

fn staged_dir() -> PathBuf {
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    home.join("Library/Application Support/threshold/staged")
}

/// The staged record: commit, repo root, subject.
fn staged_record() -> Option<(String, String, String)> {
    let dir = staged_dir();
    if !dir.join("threshold.app").is_dir() {
        return None;
    }
    let text = std::fs::read_to_string(dir.join("threshold")).ok()?;
    let mut parts = text.trim_end().splitn(3, '\t');
    Some((parts.next()?.to_string(), parts.next()?.to_string(), parts.next().unwrap_or("").to_string()))
}

#[tauri::command]
fn update_ready() -> Option<Update> {
    staged_record().map(|(commit, _, subject)| Update { commit: commit.chars().take(7).collect(), subject })
}

/// Quit so the staged build can be installed, and ask for it to be opened
/// once it is. Starts the installer itself, in case none is watching (after
/// a reboot, say); if one is, it picks this up.
#[tauri::command]
fn restart_for_update(app: AppHandle) -> Result<(), String> {
    let (_, root, _) = staged_record().ok_or("no update is waiting")?;
    std::fs::write(staged_dir().join("threshold.restart"), b"").map_err(|e| e.to_string())?;
    let script = PathBuf::from(&root).join("scripts/auto-install.sh");
    let home = std::env::var("HOME").unwrap_or_default();
    let log = std::fs::OpenOptions::new().create(true).append(true).open(PathBuf::from(home).join("Library/Logs/threshold-auto-install.log")).map_err(|e| e.to_string())?;
    Command::new("/usr/bin/nohup")
        .arg(&script)
        .current_dir(&root)
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("starting {}: {e}", script.display()))?;
    app.exit(0);
    Ok(())
}

/// What the desktop shows about phone access: whether it is on, as which
/// machine, where, and which phones are paired.
#[derive(Serialize)]
struct PhoneStatus {
    enabled: bool,
    machine: Option<mobile::Machine>,
    url: Option<String>,
    devices: Vec<mobile::DeviceView>,
}

#[tauri::command]
fn phone_status(state: tauri::State<std::sync::Arc<mobile::Mobile>>) -> PhoneStatus {
    let c = state.config();
    PhoneStatus { enabled: c.enabled && state.machine().is_some(), machine: state.machine().cloned(), url: c.url.clone(), devices: state.devices() }
}

#[derive(Serialize)]
struct Pairing {
    url: String,
    /// The same link as a QR code, as SVG.
    svg: String,
    expires_in_ms: u64,
}

/// A fresh pairing link for the QR: this machine's address with a one-time
/// token that expires in a minute. In the fragment, so it never reaches a
/// server log or a proxy.
#[tauri::command]
fn phone_pairing(state: tauri::State<std::sync::Arc<mobile::Mobile>>) -> Result<Pairing, String> {
    let base = state.config().url.clone().ok_or("phone access has not been set up on this Mac")?;
    state.machine().ok_or("this Mac has no name for the phone to show")?;
    let url = format!("{}/#pair={}", base.trim_end_matches('/'), state.new_pairing_token());
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|e| e.to_string())?;
    let svg = code.render::<qrcode::render::svg::Color>().min_dimensions(240, 240).quiet_zone(true).build();
    Ok(Pairing { url, svg, expires_in_ms: mobile::PAIRING_TTL.as_millis() as u64 })
}

#[tauri::command]
fn phone_revoke(app: AppHandle, state: tauri::State<std::sync::Arc<mobile::Mobile>>, id: String) -> Result<Vec<mobile::DeviceView>, String> {
    state.revoke(&id).map_err(|e| e.to_string())?;
    // A revoked phone must not keep being woken.
    if let Some(push) = app.try_state::<std::sync::Arc<push::Push>>() {
        push.unsubscribe_device(&id);
    }
    Ok(state.devices())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Size, position, and maximised or full-screen state, saved on
        // quit and put back on launch: a native app that forgets where it
        // was put feels broken. Visibility is left out, so a window that
        // was hidden when saved cannot come back hidden.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED
                        | tauri_plugin_window_state::StateFlags::FULLSCREEN,
                )
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // The platform's own menu, spelled out rather than taken from
        // Menu::default and patched: finding its submenus by title didn't
        // reliably reach File and Help. Plus Settings… where a Mac expects
        // it, File ▸ Open Folder… for reading a repo's diff yourself, and
        // Help ▸ Take the Tour.
        .menu(|handle| {
            use tauri::menu::{Menu, MenuItem, PredefinedMenuItem as P, Submenu, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID};
            let settings = MenuItem::with_id(handle, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
            let open_folder = MenuItem::with_id(handle, "open-folder", "Open Folder…", true, Some("CmdOrCtrl+O"))?;
            let tour = MenuItem::with_id(handle, "tour", "Take the Tour", true, None::<&str>)?;
            Menu::with_items(
                handle,
                &[
                    #[cfg(target_os = "macos")]
                    &Submenu::with_items(
                        handle,
                        handle.package_info().name.clone(),
                        true,
                        &[
                            &P::about(handle, None, None)?,
                            &P::separator(handle)?,
                            &settings,
                            &P::separator(handle)?,
                            &P::services(handle, None)?,
                            &P::separator(handle)?,
                            &P::hide(handle, None)?,
                            &P::hide_others(handle, None)?,
                            &P::separator(handle)?,
                            &P::quit(handle, None)?,
                        ],
                    )?,
                    &Submenu::with_items(
                        handle,
                        "File",
                        true,
                        &[
                            &open_folder,
                            &P::separator(handle)?,
                            #[cfg(not(target_os = "macos"))]
                            &settings,
                            &P::close_window(handle, None)?,
                            #[cfg(not(target_os = "macos"))]
                            &P::quit(handle, None)?,
                        ],
                    )?,
                    &Submenu::with_items(
                        handle,
                        "Edit",
                        true,
                        &[
                            &P::undo(handle, None)?,
                            &P::redo(handle, None)?,
                            &P::separator(handle)?,
                            &P::cut(handle, None)?,
                            &P::copy(handle, None)?,
                            &P::paste(handle, None)?,
                            &P::select_all(handle, None)?,
                        ],
                    )?,
                    #[cfg(target_os = "macos")]
                    &Submenu::with_items(handle, "View", true, &[&P::fullscreen(handle, None)?])?,
                    &Submenu::with_id_and_items(
                        handle,
                        WINDOW_SUBMENU_ID,
                        "Window",
                        true,
                        &[&P::minimize(handle, None)?, &P::maximize(handle, None)?, &P::separator(handle)?, &P::close_window(handle, None)?],
                    )?,
                    &Submenu::with_id_and_items(handle, HELP_SUBMENU_ID, "Help", true, &[&tour])?,
                ],
            )
        })
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "settings" => { let _ = app.emit("open-settings", ()); }
                "open-folder" => { let _ = app.emit("open-folder", ()); }
                "tour" => { let _ = app.emit("start-tour", ()); }
                _ => {}
            }
        })
        .setup(|app| {
            // The window starts hidden so it never shows before the
            // frontend has put its theme on, and the frontend shows it.
            // Should the frontend fail before it gets that far, show it
            // anyway: a hidden app is worse than one in the wrong colours.
            let fallback = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(2));
                if let Some(w) = fallback.get_webview_window("main") {
                    let _ = w.show();
                }
            });
            notify::init(app.handle());
            let args = parse_launch_args();
            // A review's revision belongs to the review, not to the
            // command line. Deriving it from the log keeps a directly
            // launched review consistent with one opened from the inbox —
            // otherwise --session without --revision silently diffs the
            // working tree and reports the wrong problem entirely.
            let revision = args
                .session
                .as_ref()
                .and_then(|p| threshold_core::current_revision(&threshold_core::read_session(p)))
                .or_else(|| args.revision.clone());
            app.manage(SessionState {
                revision: Mutex::new(revision),
                active: Mutex::new(args.session.clone()),
                ..Default::default()
            });
            // Always watching: the inbox matters even when no review is
            // open, which is how a pushed review announces itself.
            if let Some(session) = args.session.clone() {
                let rev = app.state::<SessionState>().revision.lock().unwrap().clone();
                run_checks(&session, rev);
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || watch(handle));
            start_phone_server(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            initial_repo,
            initial_revision,
            pick_repo_folder,
            load_diff,
            list_decisions,
            decide,
            session_events,
            load_bundle,
            mark_chapter,
            chapter_states,
            load_place,
            save_place,
            list_reviews,
            review_sizes,
            set_archived,
            open_review,
            review_repo,
            has_session,
            active_review,
            post_note,
            line_anchors,
            file_text,
            file_image,
            notify::notify,
            notify::native_notifications,
            phone_status,
            phone_pairing,
            phone_revoke,
            update_ready,
            skill_status,
            install_skill,
            reveal_skill,
            start_sample,
            restart_for_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running Threshold");
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_macos_git_stub_reads_as_git_missing() {
        // What /usr/bin/git prints on a Mac without the Command Line Tools,
        // before and after they've been removed.
        for stub in [
            "No developer tools were found, requesting install.\nIf developer tools are located at a non-default location on disk, use `sudo xcode-select --switch path/to/Xcode.app` to specify the Xcode that you wish to use for command line developer tools, and cancel the installation dialog.",
            "xcrun: error: invalid active developer path (/Library/Developer/CommandLineTools), missing xcrun at: /Library/Developer/CommandLineTools/usr/bin/xcrun",
        ] {
            assert!(crate::git_error(stub.as_bytes()).contains("xcode-select --install"), "{stub}");
        }
        assert_eq!(crate::git_error(b"fatal: bad revision 'nope'\n"), "fatal: bad revision 'nope'");
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "no git");
        assert!(crate::git_spawn_error(missing).contains("xcode-select --install"));
    }

    #[test]
    fn the_tour_samples_verdict_stays_out_of_history() {
        let tmp = tempfile::tempdir().unwrap();
        let decisions = tmp.path().join("decisions.json");
        let log = crate::sample::create(&tmp.path().join("sample"), &tmp.path().join("reviews"), "2026-01-01-120000").unwrap();
        let summary = threshold_core::summarize(&[]);
        let kept = crate::record_verdict(&decisions, Some(&log), None, summary.clone(), Verdict::Approved, None, None, None).unwrap();
        assert!(kept.is_empty(), "the sample's verdict reached History");
        assert!(threshold_core::session_verdict(&log).is_some(), "the sample's own log has no verdict");

        // An ordinary review's verdict still does.
        let other = tmp.path().join("reviews/2026-01-01-130000-stacks.jsonl");
        std::fs::copy(&log, &other).unwrap();
        let kept = crate::record_verdict(&decisions, Some(&other), None, summary, Verdict::Approved, None, None, None).unwrap();
        assert_eq!(kept.len(), 1);
    }

    use super::*;

    /// A repo with two commits on `main` and two more on a branch, plus
    /// a commit landing on `main` afterwards — the shape that makes the
    /// difference between `..` and `...` visible, and the shape a real
    /// handover has.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().to_str().unwrap().to_string();
        let run = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&p).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "first"]);

        run(&["checkout", "-q", "-b", "work"]);
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["commit", "-qam", "the change under review"]);

        // Landing on main after the branch diverged: the commit that
        // `main..HEAD` would wrongly present as part of the review.
        run(&["checkout", "-q", "main"]);
        std::fs::write(dir.path().join("b.txt"), "someone else\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "unrelated"]);
        run(&["checkout", "-q", "work"]);
        dir
    }

    fn oid(repo: &str, rev: &str) -> String {
        git(repo, &["rev-parse", rev]).unwrap().trim().to_string()
    }

    #[test]
    fn a_symmetric_range_pins_to_the_merge_base_and_the_head() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let pin = resolve_pin(p, "main...HEAD").unwrap().expect("a two-commit range is pinnable");
        assert!(pin.is_valid(), "both sides must be object ids");
        assert_eq!(pin.base, oid(p, "main~1"), "the merge base, not the tip of main");
        assert_eq!(pin.head, oid(p, "HEAD"));
    }

    #[test]
    fn a_pinned_diff_does_not_move_when_the_branch_does() {
        // The whole point. The spec `main...HEAD` answers differently
        // once another commit lands; the pin does not.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let pin = resolve_pin(p, "main...HEAD").unwrap().unwrap();
        let before = diff_at(p, false, Some("main...HEAD"), Some(&pin)).unwrap();
        let unpinned_before = diff_at(p, false, Some("main...HEAD"), None).unwrap();

        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let run = |args: &[&str]| {
            Command::new("git").arg("-C").arg(p).args(args).output().unwrap();
        };
        run(&["commit", "-qam", "more work, after the handover"]);

        let after = diff_at(p, false, Some("main...HEAD"), Some(&pin)).unwrap();
        let unpinned_after = diff_at(p, false, Some("main...HEAD"), None).unwrap();

        assert_eq!(before.digest, after.digest, "a pinned review must render the same diff forever");
        assert_ne!(
            unpinned_before.digest, unpinned_after.digest,
            "and the spec it came from must be shown to move, or this test proves nothing"
        );
    }

    #[test]
    fn a_pinned_diff_keeps_the_source_label_it_was_handed_over_as() {
        // The reviewer should still read "main...HEAD", which is what the
        // review is about; only the commits behind it are fixed.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let pin = resolve_pin(p, "main...HEAD").unwrap().unwrap();
        let payload = diff_at(p, false, Some("main...HEAD"), Some(&pin)).unwrap();
        assert_eq!(payload.source.as_deref(), Some("main...HEAD"));
        assert_eq!(payload.pin.as_ref().map(|x| &x.head), Some(&pin.head));
    }

    #[test]
    fn a_single_revision_cannot_be_pinned_because_the_working_tree_is_half_of_it() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        assert!(resolve_pin(p, "HEAD").unwrap().is_none());
        assert!(resolve_pin(p, "HEAD~1").unwrap().is_none());
    }

    #[test]
    fn an_unpinned_diff_moves_the_moment_the_working_tree_does() {
        // What `review_unpinned` is warning about, demonstrated.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nedited\n").unwrap();
        let before = diff_at(p, false, Some("HEAD"), None).unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nedited again\n").unwrap();
        let after = diff_at(p, false, Some("HEAD"), None).unwrap();
        assert_ne!(before.digest, after.digest);
    }

    #[test]
    fn a_spec_git_cannot_resolve_is_an_error_rather_than_a_silent_fallback() {
        // Falling back to an unpinned render would be the worst outcome:
        // the review would look pinned and not be.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        assert!(resolve_pin(p, "nonexistent...HEAD").is_err());
        assert!(resolve_pin(p, "nope..HEAD").is_err());
    }

    #[test]
    fn a_spec_beginning_with_a_dash_never_reaches_git() {
        // git would read it as an option; --output= writes a file.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        assert!(resolve_pin(p, "--output=/tmp/pwned..HEAD").is_err());
    }

    fn push(inbox: &Path, name: &str, repo: &str) -> PathBuf {
        let p = inbox.join(name);
        let rev = Some("main...HEAD".to_string());
        let brief = Body::Brief { title: name.into(), summary: "s".into(), implications: vec![], repo: Some(repo.into()) };
        threshold_core::append_event(&p, Author::Agent, rev.clone(), brief).unwrap();
        // A chapter that claims nothing, so the one hunk goes unbundled.
        let chapters = vec![Chapter { title: "empty".into(), summary: String::new(), hunks: vec![] }];
        threshold_core::append_event(&p, Author::Agent, rev, Body::Bundle { chapters }).unwrap();
        p
    }

    fn codes(path: &Path) -> Vec<String> {
        threshold_core::read_session(path)
            .into_iter()
            .filter_map(|e| match e.body {
                Body::Diagnostic { code, .. } => Some(code),
                Body::Pinned { .. } => Some("pinned".into()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_review_nobody_has_opened_is_pinned_and_diagnosed_for_the_agent() {
        let dir = repo();
        let inbox = tempfile::tempdir().unwrap();
        let p = push(inbox.path(), "unopened.jsonl", dir.path().to_str().unwrap());
        let mut checked = HashMap::new();

        assert!(check_inbox(inbox.path(), &mut checked, Duration::ZERO));
        assert_eq!(codes(&p), vec!["pinned", "bundle_incomplete"]);

        // Its own writes are not a change to check again.
        let len = threshold_core::read_session(&p).len();
        assert!(!check_inbox(inbox.path(), &mut checked, Duration::ZERO));
        assert_eq!(threshold_core::read_session(&p).len(), len);
    }

    #[test]
    fn a_corrected_bundle_is_checked_again_and_a_still_broken_one_is_reported() {
        // An incomplete bundle, then a corrected one at the same revision,
        // then a broken one again: each is checked, and only the broken
        // ones produce a finding.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let inbox = tempfile::tempdir().unwrap();
        let review = push(inbox.path(), "rebundled.jsonl", p);
        let mut checked = HashMap::new();
        check_inbox(inbox.path(), &mut checked, Duration::ZERO);
        assert_eq!(codes(&review), vec!["pinned", "bundle_incomplete"]);

        let rev = Some("main...HEAD".to_string());
        let pin = threshold_core::current_pin(&review, rev.as_deref()).unwrap().0;
        let hunks = threshold_core::hunk_refs(&diff_at(p, false, rev.as_deref(), Some(&pin)).unwrap().files);
        let complete = vec![Chapter { title: "all of it".into(), summary: String::new(), hunks }];
        threshold_core::append_event(&review, Author::Agent, rev.clone(), Body::Bundle { chapters: complete }).unwrap();
        assert!(check_inbox(inbox.path(), &mut checked, Duration::ZERO), "a new bundle is a change to check");
        assert_eq!(codes(&review), vec!["pinned", "bundle_incomplete"], "nothing new to say about a complete bundle");

        let empty = vec![Chapter { title: "empty".into(), summary: String::new(), hunks: vec![] }];
        threshold_core::append_event(&review, Author::Agent, rev, Body::Bundle { chapters: empty }).unwrap();
        check_inbox(inbox.path(), &mut checked, Duration::ZERO);
        assert_eq!(codes(&review), vec!["pinned", "bundle_incomplete", "bundle_incomplete"], "the broken bundle is news");
    }

    #[test]
    fn a_waiting_review_is_sized_by_the_diff_it_renders_and_a_pinned_one_stays_put() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let inbox = tempfile::tempdir().unwrap();
        let review = push(inbox.path(), "sized.jsonl", p);
        check_inbox(inbox.path(), &mut HashMap::new(), Duration::ZERO);
        let decided = push(inbox.path(), "decided.jsonl", p);
        threshold_core::append_event(&decided, Author::Human, None, Body::Verdict { verdict: Verdict::Approved, reason: None, digest: None }).unwrap();

        let pin = threshold_core::current_pin(&review, Some("main...HEAD")).unwrap().0;
        let rendered = diff_at(p, false, Some("main...HEAD"), Some(&pin)).unwrap().summary;
        let sizes = sizes_in(inbox.path());
        assert_eq!(
            sizes.get(&review),
            Some(&DiffSize { files: rendered.files_changed, added: rendered.lines_added, removed: rendered.lines_removed }),
            "the inbox must agree with the review pane"
        );
        assert_eq!(sizes.get(&review).map(|s| (s.added, s.removed)), Some((1, 0)));
        assert!(!sizes.contains_key(&decided), "nobody budgets time for a decided review");

        std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        Command::new("git").arg("-C").arg(p).args(["commit", "-qam", "after the handover"]).output().unwrap();
        assert_eq!(sizes_in(inbox.path()).get(&review), sizes.get(&review), "a pinned review is the size it was handed over at");
    }

    #[test]
    fn a_review_still_being_pushed_is_left_alone() {
        // Checked between the brief and the bundle, a review reports on
        // half of itself, and Threshold's lines take the agent's next seq.
        let dir = repo();
        let inbox = tempfile::tempdir().unwrap();
        let p = push(inbox.path(), "arriving.jsonl", dir.path().to_str().unwrap());
        assert!(!check_inbox(inbox.path(), &mut HashMap::new(), Duration::from_secs(3600)));
        assert!(codes(&p).is_empty());
    }

    #[test]
    fn empty_and_decided_reviews_are_not_checked() {
        let dir = repo();
        let inbox = tempfile::tempdir().unwrap();
        // Created, brief not yet written: not a review missing its repo.
        let empty = inbox.path().join("empty.jsonl");
        std::fs::write(&empty, "").unwrap();
        let decided = push(inbox.path(), "decided.jsonl", dir.path().to_str().unwrap());
        threshold_core::append_event(&decided, Author::Human, None, Body::Verdict { verdict: Verdict::Approved, reason: None, digest: None }).unwrap();

        assert!(!check_inbox(inbox.path(), &mut HashMap::new(), Duration::ZERO));
        assert!(threshold_core::read_session(&empty).is_empty());
        assert!(codes(&decided).is_empty());
    }

    #[test]
    fn a_line_note_follows_its_code_through_a_real_amend() {
        // Written on line 2 ("two") against the first pin. The agent
        // amends, adding a line above it; the review re-pins. The note must
        // now sit on line 3, which still reads "two" — not on line 2, which
        // now reads "one".
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let session = dir.path().join("s.jsonl");
        let rev = Some("main...HEAD".to_string());
        let first = resolve_pin(p, "main...HEAD").unwrap().unwrap();
        threshold_core::append_event(&session, Author::Threshold, rev.clone(), Body::Pinned { base: first.base.clone(), head: first.head.clone(), digest: "d".into() }).unwrap();
        let note = Body::Note { text: "why?".into(), file: Some("a.txt".into()), line: Some(2), line_content: Some("two".into()), chapter: None, in_reply_to: None, flag: None };
        let n = threshold_core::append_event(&session, Author::Human, rev.clone(), note).unwrap();

        std::fs::write(dir.path().join("a.txt"), "zero\none\ntwo\n").unwrap();
        let out = Command::new("git").arg("-C").arg(p).args(["commit", "-q", "--amend", "-am", "the change, amended"]).output().unwrap();
        assert!(out.status.success());
        threshold_core::append_event(&session, Author::Agent, rev.clone(), Body::Revised { to: "main...HEAD".into(), note: None }).unwrap();
        let second = resolve_pin(p, "main...HEAD").unwrap().unwrap();
        assert_ne!(first.head, second.head);
        threshold_core::append_event(&session, Author::Threshold, rev.clone(), Body::Pinned { base: second.base.clone(), head: second.head.clone(), digest: "d".into() }).unwrap();

        let events = threshold_core::read_session(&session);
        let diff = diff_at(p, false, Some("main...HEAD"), Some(&second)).unwrap().files;
        let anchors = anchors_for(p, &events, Some(&second), &diff);
        let a = anchors.iter().find(|a| a.seq == n.seq).unwrap();
        assert_eq!((a.file.as_str(), a.line, a.status, a.in_diff), ("a.txt", 3, threshold_core::AnchorStatus::Moved, true));
    }

    #[test]
    fn context_comes_from_the_pinned_head_not_the_working_tree() {
        // The unchanged lines around a hunk must belong to the same commits
        // as the hunk. A working tree edited since the pin is not them.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let pin = resolve_pin(p, "main...HEAD").unwrap().unwrap();
        std::fs::write(dir.path().join("a.txt"), "edited on disk afterwards\n").unwrap();
        let lines = file_text_at(p, false, Some("main...HEAD"), Some(&pin), "a.txt").unwrap();
        assert_eq!(lines, vec!["one", "two"]);
        assert_eq!(file_text_at(p, false, None, None, "a.txt").unwrap(), vec!["edited on disk afterwards"], "an unpinned working-tree diff reads the disk");
    }

    #[test]
    fn context_cannot_be_read_from_outside_the_repository() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        // A real file beside the repository, reachable by `..`.
        let beside = tempfile::tempdir_in(dir.path().parent().unwrap()).unwrap();
        std::fs::write(beside.path().join("secret"), "not yours\n").unwrap();
        let escape = format!("../{}/secret", beside.path().file_name().unwrap().to_str().unwrap());
        assert!(dir.path().join(&escape).exists(), "the escape path must point at something, or this proves nothing");
        assert!(file_text_at(p, false, None, None, &escape).is_err());
        assert!(file_text_at(p, false, None, None, "/etc/hosts").is_err());
    }

    #[test]
    fn an_image_comes_from_the_pinned_commits_on_each_side() {
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(p).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        // Two different "images", committed before and after on the branch.
        std::fs::write(dir.path().join("fig.png"), b"\x89PNG-before").unwrap();
        run(&["add", "fig.png"]);
        run(&["commit", "-qm", "before"]);
        let base = oid(p, "HEAD");
        std::fs::write(dir.path().join("fig.png"), b"\x89PNG-after").unwrap();
        run(&["commit", "-qam", "after"]);
        let pin = Pin { base, head: oid(p, "HEAD") };
        // The working tree moves on; the pinned sides must not.
        std::fs::write(dir.path().join("fig.png"), b"\x89PNG-on-disk").unwrap();

        use base64::Engine;
        let decode = |url: String| base64::engine::general_purpose::STANDARD.decode(url.split_once(',').unwrap().1).unwrap();
        let new = image_at(p, false, Some("x"), Some(&pin), "fig.png", false).unwrap();
        assert!(new.starts_with("data:image/png;base64,"));
        assert_eq!(decode(new), b"\x89PNG-after");
        assert_eq!(decode(image_at(p, false, Some("x"), Some(&pin), "fig.png", true).unwrap()), b"\x89PNG-before");
        assert!(image_at(p, false, Some("x"), Some(&pin), "a.txt", false).is_err(), "only images are shipped to the UI");
        assert!(image_at(p, false, None, None, "../../etc/hosts.png", false).is_err());
    }

    #[test]
    fn two_dots_and_three_dots_pin_to_different_bases() {
        // `main..HEAD` compares against the tip of main, which now holds
        // a commit the reviewer never wrote. `main...HEAD` does not.
        let dir = repo();
        let p = dir.path().to_str().unwrap();
        let two = resolve_pin(p, "main..HEAD").unwrap().unwrap();
        let three = resolve_pin(p, "main...HEAD").unwrap().unwrap();
        assert_eq!(two.base, oid(p, "main"));
        assert_eq!(three.base, oid(p, "main~1"));
        assert_ne!(two.base, three.base, "the distinction must survive resolution");
    }
}
