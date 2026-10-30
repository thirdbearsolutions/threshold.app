//! The phone's way in: reviews over HTTP, from inside the desktop app.
//!
//! Inside the app rather than a daemon because `session::append` numbers
//! events as last-seen plus one, so a second human-side writer would race
//! the desktop on every append. One process, one writer. The consequence
//! is that the phone works only while Threshold is running on the Mac,
//! and the phone has to say so plainly.
//!
//! Listens on loopback only. `tailscale serve` is what puts it on the
//! tailnet, with a real certificate; nothing here ever binds a wider
//! address.
//!
//! Every review is named by its file name in the inbox and checked by
//! `resolve_review_name`. Never a caller-supplied path or repo: loading a
//! diff runs `git` in the repo a review names, so from the network a path
//! would be a command.
//!
//! Approve and Reject both, with a plain Approve button rather than a
//! passkey or a gate on every chapter being marked. What guards it is what
//! guards Reject: it names the diff the phone rendered, and is refused if
//! that has since changed, and it names the machine.

use axum::extract::{FromRequestParts, Path as UrlPath, State};
use axum::http::{request::Parts, HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use threshold_core::{Body, Event, ReviewNameError, Verdict};
use tokio::sync::broadcast;

use crate::mobile::Mobile;
use crate::push::Push;

/// Header every write must carry: the id of the machine the phone thinks
/// it is talking to.
pub const MACHINE_HEADER: &str = "x-threshold-machine";

#[derive(Clone)]
pub struct Ctx {
    pub inbox: PathBuf,
    pub decisions: PathBuf,
    pub mobile: Arc<Mobile>,
    pub push: Arc<Push>,
}

/// Something the phone should refetch.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Pushed {
    InboxChanged,
    SessionChanged { review: String },
}

static EVENTS: OnceLock<broadcast::Sender<Pushed>> = OnceLock::new();

fn events() -> &'static broadcast::Sender<Pushed> {
    EVENTS.get_or_init(|| broadcast::channel(64).0)
}

/// Tell every connected phone. A no-op when none are.
pub fn publish(p: Pushed) {
    let _ = events().send(p);
}

#[derive(Debug)]
pub enum ApiError {
    /// No body: an unpaired caller learns nothing, not even the route.
    Unauthorized,
    NotFound,
    /// Not found, saying what: a file, not a review.
    NotFoundMsg(String),
    BadRequest(String),
    Forbidden(String),
    Conflict(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Unauthorized => return StatusCode::UNAUTHORIZED.into_response(),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "no such review".to_string()),
            ApiError::NotFoundMsg(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

/// A request from a paired phone, carrying which one.
pub struct Paired(pub String);

impl FromRequestParts<Ctx> for Paired {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> Result<Self, ApiError> {
        let credential = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        ctx.mobile.authenticate(credential).map(Paired).ok_or(ApiError::Unauthorized)
    }
}

/// A write from a paired phone that knows which machine it is writing to.
///
/// The phone's own origin already separates machines; this catches what
/// the origin cannot, such as a host name that has since moved to another
/// Mac, or a shell cached from before it did.
pub struct Bound(pub String);

impl FromRequestParts<Ctx> for Bound {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, ctx: &Ctx) -> Result<Self, ApiError> {
        let Paired(device) = Paired::from_request_parts(parts, ctx).await?;
        check_machine(&parts.headers, &ctx.mobile)?;
        Ok(Bound(device))
    }
}

fn check_machine(headers: &HeaderMap, mobile: &Mobile) -> Result<(), ApiError> {
    let ours = mobile.machine().ok_or_else(|| ApiError::Internal("this machine has not been named".into()))?;
    match headers.get(MACHINE_HEADER).and_then(|v| v.to_str().ok()) {
        Some(theirs) if theirs == ours.id => Ok(()),
        _ => Err(ApiError::Conflict(format!(
            "This is Threshold on “{}”, not the machine this page was loaded for. Nothing was written. Reload to continue on “{}”.",
            ours.name, ours.name
        ))),
    }
}

fn review_path(ctx: &Ctx, name: &str) -> Result<PathBuf, ApiError> {
    threshold_core::resolve_review_name(&ctx.inbox, name).map_err(|e| match e {
        ReviewNameError::NotAReviewName => ApiError::BadRequest(e.to_string()),
        ReviewNameError::NotFound => ApiError::NotFound,
    })
}

/// Run blocking work (file reads, `git`) off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError::Internal(e.to_string()))?
}

fn revision_of(path: &std::path::Path) -> Option<String> {
    threshold_core::current_revision(&threshold_core::read_session(path))
}

#[derive(Serialize)]
struct MachineView {
    id: String,
    name: String,
    color: String,
}

async fn machine(State(ctx): State<Ctx>) -> ApiResult<MachineView> {
    let m = ctx.mobile.machine().ok_or_else(|| ApiError::Internal("this machine has not been named".into()))?;
    Ok(Json(MachineView { id: m.id.clone(), name: m.name.clone(), color: m.color.clone() }))
}

#[derive(Deserialize)]
struct PairRequest {
    token: String,
    #[serde(default)]
    name: String,
}

#[derive(Serialize)]
struct PairResponse {
    credential: String,
    device: crate::mobile::DeviceView,
    machine: MachineView,
}

async fn pair(State(ctx): State<Ctx>, Json(req): Json<PairRequest>) -> ApiResult<PairResponse> {
    let m = ctx.mobile.machine().ok_or_else(|| ApiError::Internal("this machine has not been named".into()))?;
    let machine = MachineView { id: m.id.clone(), name: m.name.clone(), color: m.color.clone() };
    let (device, credential) = ctx
        .mobile
        .pair(&req.token, &req.name)
        .ok_or_else(|| ApiError::Forbidden("That pairing code has expired or was already used. Show a fresh one on the Mac.".into()))?;
    Ok(Json(PairResponse { credential, device, machine }))
}

/// One row of the phone's inbox. The repo is only its last component: the
/// phone needs to know which project, not where your home directory is.
#[derive(Serialize)]
struct ReviewRow {
    name: String,
    title: String,
    project: Option<String>,
    revision: Option<String>,
    decided: bool,
    archived: bool,
    last_at_ms: u64,
}

async fn reviews(_: Paired, State(ctx): State<Ctx>) -> ApiResult<Vec<ReviewRow>> {
    let rows = blocking(move || {
        Ok(threshold_core::list_reviews(&ctx.inbox)
            .into_iter()
            .filter_map(|r| {
                Some(ReviewRow {
                    name: r.path.file_name()?.to_string_lossy().into_owned(),
                    project: r.repo.as_deref().and_then(|p| std::path::Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned()),
                    title: r.title,
                    revision: r.revision,
                    decided: r.decided,
                    archived: r.archived,
                    last_at_ms: r.last_at_ms,
                })
            })
            .collect())
    })
    .await?;
    Ok(Json(rows))
}

/// A review's events, checked first as opening it on the desktop does.
async fn review(_: Paired, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>) -> ApiResult<Vec<Event>> {
    let path = review_path(&ctx, &name)?;
    let events = blocking(move || {
        crate::run_checks(&path, revision_of(&path));
        Ok(threshold_core::read_session(&path))
    })
    .await?;
    Ok(Json(events))
}

async fn diff(_: Paired, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>) -> Result<Json<crate::DiffPayload>, ApiError> {
    let path = review_path(&ctx, &name)?;
    Ok(Json(blocking(move || current_diff(&path)).await?))
}

fn current_diff(path: &std::path::Path) -> Result<crate::DiffPayload, ApiError> {
    let repo = crate::repo_of(path).ok_or_else(|| ApiError::Conflict("this review names no repo".into()))?;
    let revision = revision_of(path);
    let pin = threshold_core::current_pin(path, revision.as_deref()).map(|(p, _)| p);
    crate::diff_at(&repo, false, revision.as_deref(), pin.as_ref()).map_err(ApiError::Conflict)
}

async fn bundle(_: Paired, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>) -> Result<Json<Option<crate::BundlePayload>>, ApiError> {
    let path = review_path(&ctx, &name)?;
    Ok(Json(blocking(move || Ok(crate::bundle_at(&path, revision_of(&path).as_deref()))).await?))
}

#[derive(Serialize)]
struct ChapterRow {
    title: String,
    #[serde(flatten)]
    state: threshold_core::ChapterState,
}

/// Chapter states, with each chapter's files taken from the bundle rather
/// than from the caller, which is the one thing that knows them.
async fn chapters(_: Paired, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>) -> ApiResult<Vec<ChapterRow>> {
    let path = review_path(&ctx, &name)?;
    let rows = blocking(move || {
        let revision = revision_of(&path);
        let Some(chapters) = threshold_core::current_bundle(&path, revision.as_deref()) else {
            return Ok(vec![]);
        };
        let titles: Vec<String> = chapters.iter().map(|c| c.title.clone()).collect();
        let files: Vec<Vec<String>> = chapters
            .iter()
            .map(|c| {
                let mut f: Vec<String> = c.hunks.iter().map(|h| h.file.clone()).collect();
                f.dedup();
                f
            })
            .collect();
        let states = crate::chapter_states_at(&path, revision.as_deref(), &titles, &files);
        Ok(titles.into_iter().zip(states).map(|(title, state)| ChapterRow { title, state }).collect())
    })
    .await?;
    Ok(Json(rows))
}

async fn anchors(_: Paired, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>) -> ApiResult<Vec<threshold_core::Anchor>> {
    let path = review_path(&ctx, &name)?;
    let out = blocking(move || {
        Ok(crate::rendered_at(&path, revision_of(&path).as_deref())
            .map(|(repo, events, pin, diff)| crate::anchors_for(&repo, &events, pin.as_ref(), &diff))
            .unwrap_or_default())
    })
    .await?;
    Ok(Json(out))
}

async fn note(_: Bound, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>, Json(note): Json<crate::NoteArgs>) -> ApiResult<Vec<Event>> {
    let path = review_path(&ctx, &name)?;
    let events = blocking(move || crate::append_note(&path, revision_of(&path), note).map_err(ApiError::BadRequest)).await?;
    Ok(Json(events))
}

#[derive(Deserialize)]
struct MarkRequest {
    chapter: String,
    mark: crate::MarkArg,
}

async fn mark(_: Bound, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>, Json(req): Json<MarkRequest>) -> ApiResult<Vec<Event>> {
    let path = review_path(&ctx, &name)?;
    let events = blocking(move || crate::append_mark(&path, revision_of(&path), req.chapter, req.mark).map_err(ApiError::BadRequest)).await?;
    Ok(Json(events))
}

#[derive(Deserialize)]
struct VerdictRequest {
    verdict: String,
    #[serde(default)]
    reason: Option<String>,
    /// The digest of the diff the phone rendered. Checked against the
    /// diff as it stands, so a verdict names what was actually read.
    digest: String,
}

async fn verdict(_: Bound, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>, Json(req): Json<VerdictRequest>) -> ApiResult<Vec<Event>> {
    let verdict = match req.verdict.as_str() {
        "rejected" => Verdict::Rejected,
        "approved" => Verdict::Approved,
        _ => return Err(ApiError::BadRequest("verdict must be \"approved\" or \"rejected\"".into())),
    };
    let reason = req.reason.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    // A rejection must say what to fix; an approval may say nothing.
    if verdict == Verdict::Rejected && reason.is_none() {
        return Err(ApiError::BadRequest("a rejection needs a reason, so the agent knows what to fix".into()));
    }
    let path = review_path(&ctx, &name)?;
    let decisions = ctx.decisions.clone();
    let events = blocking(move || {
        if threshold_core::session_verdict(&path).is_some() {
            return Err(ApiError::Conflict("This review has already been decided.".into()));
        }
        let now = current_diff(&path)?;
        if now.digest != req.digest {
            return Err(ApiError::Conflict("The diff changed since this page loaded it. Nothing was recorded; reload and read it again.".into()));
        }
        let revision = revision_of(&path);
        crate::record_verdict(&decisions, Some(&path), revision, now.summary, verdict, reason, now.source, Some(now.digest))
            .map_err(ApiError::Internal)?;
        Ok(threshold_core::read_session(&path))
    })
    .await?;
    Ok(Json(events))
}

/// One message in a thread, as the phone shows it.
#[derive(Serialize)]
struct Message {
    seq: u64,
    from: threshold_core::Author,
    text: String,
    at_ms: u64,
}

/// A conversation in some review, for the phone's threads screen.
#[derive(Serialize)]
struct ThreadCard {
    review: String,
    title: String,
    project: Option<String>,
    root: u64,
    /// Where it is: a line (with the code as written then), a chapter, or
    /// neither for the whole change.
    file: Option<String>,
    line: Option<u32>,
    code: Option<String>,
    chapter: Option<String>,
    flag: Option<threshold_core::Flag>,
    messages: Vec<Message>,
}

/// A review nobody has said anything in yet.
#[derive(Serialize)]
struct FreshReview {
    review: String,
    title: String,
    project: Option<String>,
    summary: String,
    flags: usize,
    at_ms: u64,
}

#[derive(Serialize)]
struct Threads {
    threads: Vec<ThreadCard>,
    fresh: Vec<FreshReview>,
}

/// Every conversation in every open review, and the reviews not yet
/// spoken in: the phone's home screen. Read from the session logs alone,
/// with no git, so it stays fast however many reviews are open. Which of
/// them are the reviewer's move is the phone's call, since only it knows
/// what it has marked seen.
fn threads_in(inbox: &std::path::Path) -> Threads {
    let mut out = Threads { threads: vec![], fresh: vec![] };
    for r in threshold_core::list_reviews(inbox) {
        if r.decided || r.archived {
            continue;
        }
        let Some(name) = r.path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let project = r.repo.as_deref().and_then(|p| std::path::Path::new(p).file_name()).map(|n| n.to_string_lossy().into_owned());
        let events = threshold_core::read_session(&r.path);
        let spoken_by_human = events.iter().any(|e| e.from == threshold_core::Author::Human);
        let flags = events.iter().filter(|e| matches!(&e.body, Body::Note { flag: Some(_), in_reply_to: None, .. })).count();
        if !spoken_by_human {
            let summary = events.iter().find_map(|e| match &e.body {
                Body::Brief { summary, .. } => Some(summary.clone()),
                _ => None,
            });
            out.fresh.push(FreshReview { review: name.clone(), title: r.title.clone(), project: project.clone(), summary: summary.unwrap_or_default(), flags, at_ms: r.last_at_ms });
        }
        for root in &events {
            let (file, line, code, chapter, flag) = match &root.body {
                Body::Note { in_reply_to: None, file, line, line_content, chapter, flag, .. } => (file.clone(), *line, line_content.clone(), chapter.clone(), *flag),
                // A top-level message from Claude is a thread of its own.
                Body::Reply { in_reply_to: None, .. } if root.from == threshold_core::Author::Agent => (None, None, None, None, None),
                _ => continue,
            };
            let thread = threshold_core::thread(&events, root.seq);
            // A bare flag points at a line; it is not a conversation until
            // someone answers it. Counted on the review instead.
            if flag.is_some() && thread.len() < 2 {
                continue;
            }
            let messages = thread
                .iter()
                .filter_map(|e| match &e.body {
                    Body::Note { text, .. } | Body::Reply { text, .. } => Some(Message { seq: e.seq, from: e.from, text: text.clone(), at_ms: e.at_ms }),
                    _ => None,
                })
                .collect();
            out.threads.push(ThreadCard { review: name.clone(), title: r.title.clone(), project: project.clone(), root: root.seq, file, line, code, chapter, flag, messages });
        }
    }
    out
}

async fn threads(_: Paired, State(ctx): State<Ctx>) -> ApiResult<Threads> {
    Ok(Json(blocking(move || Ok(threads_in(&ctx.inbox))).await?))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
    #[serde(default)]
    old: bool,
}

/// Where a review's files come from: its repo, revision and pin.
fn source_of(path: &std::path::Path) -> Result<(String, Option<String>, Option<threshold_core::Pin>), ApiError> {
    let repo = crate::repo_of(path).ok_or_else(|| ApiError::Conflict("this review names no repo".into()))?;
    let revision = revision_of(path);
    let pin = threshold_core::current_pin(path, revision.as_deref()).map(|(p, _)| p);
    Ok((repo, revision, pin))
}

/// A file's new side, one entry per line, from the review's own commits:
/// what the phone renders a Markdown file from.
async fn file_text(_: Paired, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>, axum::extract::Query(q): axum::extract::Query<FileQuery>) -> ApiResult<Vec<String>> {
    let path = review_path(&ctx, &name)?;
    let lines = blocking(move || {
        let (repo, revision, pin) = source_of(&path)?;
        crate::file_text_at(&repo, false, revision.as_deref(), pin.as_ref(), &q.path).map_err(ApiError::NotFoundMsg)
    })
    .await?;
    Ok(Json(lines))
}

/// An image from one side of a review, as a data: URL. The phone cannot
/// point an <img> here, since an image request carries no credential, so
/// it fetches the URL and hands the data to the image itself.
async fn file_image(_: Paired, State(ctx): State<Ctx>, UrlPath(name): UrlPath<String>, axum::extract::Query(q): axum::extract::Query<FileQuery>) -> ApiResult<String> {
    let path = review_path(&ctx, &name)?;
    let url = blocking(move || {
        let (repo, revision, pin) = source_of(&path)?;
        crate::image_at(&repo, false, revision.as_deref(), pin.as_ref(), &q.path, q.old).map_err(ApiError::NotFoundMsg)
    })
    .await?;
    Ok(Json(url))
}

#[derive(Serialize)]
struct PushKey {
    key: String,
    subscribed: bool,
}

/// The key a phone subscribes with, and whether this phone already has.
async fn push_key(Paired(device): Paired, State(ctx): State<Ctx>) -> ApiResult<PushKey> {
    let subscribed = ctx.push.subscriptions().iter().any(|s| s.device == device);
    Ok(Json(PushKey { key: ctx.push.public_key(), subscribed }))
}

#[derive(Deserialize)]
struct SubscribeRequest {
    endpoint: String,
    /// From PushSubscription.toJSON(): the keys a payload is encrypted to.
    #[serde(default)]
    keys: Option<SubscriptionKeys>,
}

#[derive(Deserialize)]
struct SubscriptionKeys {
    p256dh: String,
    auth: String,
}

async fn push_subscribe(Bound(device): Bound, State(ctx): State<Ctx>, Json(req): Json<SubscribeRequest>) -> ApiResult<PushKey> {
    let (p256dh, auth) = req.keys.as_ref().map(|k| (Some(k.p256dh.as_str()), Some(k.auth.as_str()))).unwrap_or((None, None));
    ctx.push.subscribe(&device, &req.endpoint, p256dh, auth).map_err(ApiError::BadRequest)?;
    Ok(Json(PushKey { key: ctx.push.public_key(), subscribed: true }))
}

async fn push_unsubscribe(Bound(device): Bound, State(ctx): State<Ctx>) -> ApiResult<PushKey> {
    ctx.push.unsubscribe_device(&device);
    Ok(Json(PushKey { key: ctx.push.public_key(), subscribed: false }))
}

/// Server-sent events: what changed, never what it says. The phone
/// refetches through the authenticated routes.
async fn stream(_: Paired) -> Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = events().subscribe();
    let s = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(p) => {
                    let ev = SseEvent::default().json_data(&p).unwrap_or_default();
                    return Some((Ok(ev), rx));
                }
                // Missed some: say the whole inbox changed, which makes
                // the phone refetch everything it shows.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let ev = SseEvent::default().json_data(Pushed::InboxChanged).unwrap_or_default();
                    return Some((Ok(ev), rx));
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(s).keep_alive(KeepAlive::default())
}

mod assets {
    include!(concat!(env!("OUT_DIR"), "/mobile_assets.rs"));
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// This machine's name and colour, safe to put in a page. The colour is
/// re-spelled from its parsed bytes, so nothing but `#rrggbb` can reach CSS.
fn identity(ctx: &Ctx) -> (String, String) {
    match ctx.mobile.machine() {
        Some(m) => {
            let [r, g, b] = crate::icon::parse_color(&m.color);
            (m.name.clone(), format!("#{r:02x}{g:02x}{b:02x}"))
        }
        None => ("Threshold".into(), "#555555".into()),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("json") | Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

fn respond(body: Vec<u8>, kind: &str, immutable: bool) -> Response {
    let cache = if immutable { "public, max-age=31536000, immutable" } else { "no-cache" };
    ([(axum::http::header::CONTENT_TYPE, kind.to_string()), (axum::http::header::CACHE_CONTROL, cache.to_string())], body).into_response()
}

/// The phone app, its manifest and its icons. Everything not under /api.
///
/// The page, the manifest and the icons are filled in with this machine's
/// name and colour as they are served, so the home-screen label, the icon
/// and the browser chrome name the Mac before any script runs.
async fn shell(State(ctx): State<Ctx>, uri: axum::http::Uri) -> Response {
    let path = uri.path();
    let (name, color) = identity(&ctx);
    match path {
        "/manifest.webmanifest" => {
            let manifest = serde_json::json!({
                "name": format!("Threshold · {name}"),
                "short_name": name,
                "start_url": "/",
                "scope": "/",
                "display": "standalone",
                "theme_color": color,
                "background_color": color,
                // Maskable as well: Android crops icons to the launcher's
                // shape, and without it shrinks this one onto a white
                // disc, losing most of the colour that says which Mac.
                // The letter sits in the middle half, inside the safe zone.
                "icons": [
                    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
                    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any" },
                    { "src": "/icon-192.png", "sizes": "192x192", "type": "image/png", "purpose": "maskable" },
                    { "src": "/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" },
                    { "src": "/icon.svg", "sizes": "any", "type": "image/svg+xml" },
                ],
            });
            return respond(manifest.to_string().into_bytes(), content_type(path), false);
        }
        "/icon.svg" => return respond(crate::icon::svg(&name, &color).into_bytes(), "image/svg+xml", false),
        "/icon-180.png" => return respond(crate::icon::png(&name, &color, 180), "image/png", false),
        "/icon-192.png" => return respond(crate::icon::png(&name, &color, 192), "image/png", false),
        "/icon-512.png" => return respond(crate::icon::png(&name, &color, 512), "image/png", false),
        _ => {}
    }
    if let Some((_, bytes)) = assets::FILES.iter().find(|(p, _)| *p == path && *p != "/index.html") {
        return respond(bytes.to_vec(), content_type(path), path.starts_with("/assets/"));
    }
    if path.starts_with("/assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    // Any other path is the app, which routes itself.
    let Some((_, page)) = assets::FILES.iter().find(|(p, _)| *p == "/index.html") else {
        let msg = format!("<!doctype html><meta name=viewport content='width=device-width'><p style='font:18px sans-serif;padding:20px'>Threshold on {} is running, but its phone app was not built into it. On the Mac: <code>npm run build</code> in threshold/, then reinstall.</p>", escape_html(&name));
        return respond(msg.into_bytes(), "text/html; charset=utf-8", false);
    };
    let html = String::from_utf8_lossy(page).replace("__MACHINE_NAME__", &escape_html(&name)).replace("__MACHINE_COLOR__", &color);
    respond(html.into_bytes(), "text/html; charset=utf-8", false)
}

pub fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/api/machine", get(machine))
        .route("/api/pair", post(pair))
        .route("/api/reviews", get(reviews))
        .route("/api/threads", get(threads))
        .route("/api/reviews/{name}", get(review))
        .route("/api/reviews/{name}/diff", get(diff))
        .route("/api/reviews/{name}/bundle", get(bundle))
        .route("/api/reviews/{name}/chapters", get(chapters))
        .route("/api/reviews/{name}/anchors", get(anchors))
        .route("/api/reviews/{name}/notes", post(note))
        .route("/api/reviews/{name}/marks", post(mark))
        .route("/api/reviews/{name}/verdict", post(verdict))
        .route("/api/reviews/{name}/file", get(file_text))
        .route("/api/reviews/{name}/image", get(file_image))
        .route("/api/events", get(stream))
        .route("/api/push/key", get(push_key))
        .route("/api/push/subscribe", post(push_subscribe))
        .route("/api/push/unsubscribe", post(push_unsubscribe))
        .route("/api/{*rest}", get(|| async { ApiError::NotFound }).post(|| async { ApiError::NotFound }))
        .fallback(shell)
        // A review's diff is JSON and compresses about tenfold, which is
        // the difference that matters off Wi-Fi. The default predicate
        // leaves the event stream and images alone.
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(ctx)
}

/// Listen on loopback until the app exits.
pub async fn serve(ctx: Ctx, port: u16) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    axum::serve(listener, router(ctx)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::{Config, Machine};
    use axum::body::Body as HttpBody;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use std::process::Command;
    use threshold_core::Author;
    use tower::ServiceExt;

    struct World {
        _dirs: Vec<tempfile::TempDir>,
        ctx: Ctx,
        credential: String,
        machine: String,
    }

    fn world() -> World {
        let repo = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(repo.path()).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "Test"]);
        std::fs::write(repo.path().join("a.txt"), "one\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "first"]);
        run(&["checkout", "-qb", "work"]);
        std::fs::write(repo.path().join("a.txt"), "one\ntwo\n").unwrap();
        run(&["commit", "-qam", "change"]);

        let inbox = tempfile::tempdir().unwrap();
        let brief = Body::Brief { title: "t".into(), summary: "s".into(), implications: vec![], repo: Some(repo.path().to_str().unwrap().into()) };
        threshold_core::append_event(&inbox.path().join("r.jsonl"), Author::Agent, Some("main...HEAD".into()), brief).unwrap();

        let data = tempfile::tempdir().unwrap();
        let config = Config { enabled: true, port: None, machine: Some(Machine { id: String::new(), name: "studio".into(), color: "#2f6fdf".into() }), url: None };
        std::fs::write(data.path().join("mobile.json"), serde_json::to_string(&config).unwrap()).unwrap();
        let mobile = Arc::new(Mobile::load(data.path()));
        let (_, credential) = mobile.pair(&mobile.new_pairing_token(), "test").unwrap();
        let machine = mobile.machine().unwrap().id.clone();
        let push = Arc::new(Push::load(data.path()));
        let ctx = Ctx { inbox: inbox.path().to_path_buf(), decisions: data.path().join("decisions.json"), mobile, push };
        World { _dirs: vec![repo, inbox, data], ctx, credential, machine }
    }

    async fn call(w: &World, method: &str, uri: &str, body: Option<serde_json::Value>, auth: bool, machine: Option<&str>) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if auth {
            req = req.header("authorization", format!("Bearer {}", w.credential));
        }
        if let Some(m) = machine {
            req = req.header(MACHINE_HEADER, m);
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(HttpBody::from(b.to_string())).unwrap(),
            None => req.body(HttpBody::empty()).unwrap(),
        };
        let res = router(w.ctx.clone()).oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn an_unpaired_caller_gets_401_and_nothing_else() {
        let w = world();
        for uri in ["/api/reviews", "/api/reviews/r.jsonl", "/api/reviews/r.jsonl/diff", "/api/events"] {
            let (status, body) = call(&w, "GET", uri, None, false, None).await;
            assert_eq!((status, body), (StatusCode::UNAUTHORIZED, serde_json::Value::Null), "{uri}");
        }
    }

    #[tokio::test]
    async fn the_machine_says_its_name_to_anyone_on_the_tailnet() {
        // The pairing screen must show which Mac it is about to pair with,
        // before it has a credential.
        let w = world();
        let (status, body) = call(&w, "GET", "/api/machine", None, false, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["name"], "studio");
        assert_eq!(body["id"], w.machine);
    }

    #[tokio::test]
    async fn the_inbox_lists_reviews_by_name_and_project_not_by_path() {
        let w = world();
        let (status, body) = call(&w, "GET", "/api/reviews", None, true, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body[0]["name"], "r.jsonl");
        assert!(!body.to_string().contains(w.ctx.inbox.to_str().unwrap()), "no inbox path reaches the phone");
    }

    #[tokio::test]
    async fn a_review_name_that_escapes_the_inbox_is_refused() {
        let w = world();
        let (status, _) = call(&w, "GET", "/api/reviews/..%2Fescape.jsonl/diff", None, true, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&w, "GET", "/api/reviews/missing.jsonl/diff", None, true, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_write_meant_for_another_machine_is_refused_and_says_which_this_is() {
        let w = world();
        let note = serde_json::json!({ "text": "why?" });
        let (status, body) = call(&w, "POST", "/api/reviews/r.jsonl/notes", Some(note.clone()), true, Some("someone-else")).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body["error"].as_str().unwrap().contains("studio"));
        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/notes", Some(note.clone()), true, None).await;
        assert_eq!(status, StatusCode::CONFLICT, "a write that names no machine is not assumed to mean this one");
        assert_eq!(threshold_core::read_session(&w.ctx.inbox.join("r.jsonl")).len(), 1, "nothing was written");

        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/notes", Some(note), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::OK);
    }

    /// Serve a copy of the real inbox on 127.0.0.1:7458 for a person (or
    /// a browser) to try the phone app against. Writes land in the copy.
    ///
    /// `cargo test -p threshold-desktop serve_a_copy_of_the_inbox -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn serve_a_copy_of_the_inbox() {
        let copy = tempfile::tempdir().unwrap();
        let real = threshold_core::inbox_dir(&PathBuf::from(std::env::var("HOME").unwrap()));
        for e in std::fs::read_dir(real).unwrap().flatten() {
            std::fs::copy(e.path(), copy.path().join(e.file_name())).unwrap();
        }
        // Kept across runs, so a restart is the same machine with the same
        // paired phones, as it is for the real app.
        let data = std::env::temp_dir().join("threshold-phone-try");
        std::fs::create_dir_all(&data).unwrap();
        if !data.join("mobile.json").exists() {
            let config = Config { enabled: true, port: None, machine: Some(Machine { id: String::new(), name: "studio".into(), color: "#8a4fd8".into() }), url: None };
            std::fs::write(data.join("mobile.json"), serde_json::to_string(&config).unwrap()).unwrap();
        }
        let mobile = Arc::new(Mobile::load(&data));
        let token = mobile.new_pairing_token();
        println!("pair at http://127.0.0.1:7458/#pair={token}");
        let push = Arc::new(Push::load(&data));
        let ctx = Ctx { inbox: copy.path().to_path_buf(), decisions: data.join("decisions.json"), mobile, push };
        serve(ctx, 7458).await.unwrap();
    }

    #[tokio::test]
    async fn the_page_manifest_and_icon_are_this_machine_s() {
        let w = world();
        let res = router(w.ctx.clone()).oneshot(Request::get("/manifest.webmanifest").body(HttpBody::empty()).unwrap()).await.unwrap();
        let m: serde_json::Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        assert_eq!(m["short_name"], "studio", "the home-screen label names the Mac");
        assert_eq!(m["theme_color"], "#2f6fdf");
        let res = router(w.ctx.clone()).oneshot(Request::get("/icon-180.png").body(HttpBody::empty()).unwrap()).await.unwrap();
        assert_eq!(res.headers()["content-type"], "image/png");
        let res = router(w.ctx.clone()).oneshot(Request::get("/some/deep/link").body(HttpBody::empty()).unwrap()).await.unwrap();
        let page = String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
        assert!(page.contains("studio"), "every page names the machine");
        assert!(!page.contains("__MACHINE_"), "no placeholder survives");
    }

    #[tokio::test]
    async fn a_phone_subscribes_to_push_on_this_machine_only() {
        let w = world();
        let (status, key) = call(&w, "GET", "/api/push/key", None, true, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(key["subscribed"], false);
        let sub = serde_json::json!({ "endpoint": "https://fcm.googleapis.com/fcm/send/x" });
        let (status, _) = call(&w, "POST", "/api/push/subscribe", Some(sub.clone()), true, Some("another-mac")).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = call(&w, "POST", "/api/push/subscribe", Some(sub), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(w.ctx.push.subscriptions().len(), 1);
        let (_, key) = call(&w, "GET", "/api/push/key", None, true, None).await;
        assert_eq!(key["subscribed"], true);
    }

    #[tokio::test]
    async fn a_file_is_read_from_the_review_s_commits_and_only_with_a_credential() {
        let w = world();
        call(&w, "GET", "/api/reviews/r.jsonl", None, true, None).await;
        let (status, lines) = call(&w, "GET", "/api/reviews/r.jsonl/file?path=a.txt", None, true, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(lines, serde_json::json!(["one", "two"]));
        let (status, _) = call(&w, "GET", "/api/reviews/r.jsonl/file?path=a.txt", None, false, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = call(&w, "GET", "/api/reviews/r.jsonl/image?path=a.txt", None, true, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "only images are served as images");
    }

    #[tokio::test]
    async fn threads_are_gathered_across_reviews_and_a_bare_flag_is_not_one() {
        let w = world();
        let p = w.ctx.inbox.join("r.jsonl");
        let note = |text: &str, reply: Option<u64>, flag: Option<threshold_core::Flag>| Body::Note { text: text.into(), file: Some("a.txt".into()), line: Some(2), line_content: Some("two".into()), chapter: None, in_reply_to: reply, flag };
        let flag = threshold_core::append_event(&p, Author::Agent, None, note("look", None, Some(threshold_core::Flag::Caution))).unwrap();
        let q = threshold_core::append_event(&p, Author::Human, None, note("why two?", None, None)).unwrap();
        threshold_core::append_event(&p, Author::Agent, None, Body::Reply { text: "because".into(), in_reply_to: Some(q.seq) }).unwrap();
        threshold_core::append_event(&p, Author::Agent, None, Body::Reply { text: "heads up".into(), in_reply_to: None }).unwrap();

        // A second review nobody has spoken in.
        let brief = Body::Brief { title: "fresh one".into(), summary: "s".into(), implications: vec![], repo: Some("/x/proj".into()) };
        threshold_core::append_event(&w.ctx.inbox.join("s.jsonl"), Author::Agent, None, brief).unwrap();

        let (status, body) = call(&w, "GET", "/api/threads", None, true, None).await;
        assert_eq!(status, StatusCode::OK);
        let threads = body["threads"].as_array().unwrap();
        let roots: Vec<u64> = threads.iter().map(|t| t["root"].as_u64().unwrap()).collect();
        assert!(!roots.contains(&flag.seq), "a flag nobody answered is not a thread");
        assert!(roots.contains(&q.seq));
        let asked = threads.iter().find(|t| t["root"] == q.seq).unwrap();
        assert_eq!(asked["messages"].as_array().unwrap().len(), 2);
        assert_eq!(asked["code"], "two");
        assert_eq!(asked["messages"][1]["from"], "agent");
        assert!(threads.iter().any(|t| t["messages"][0]["text"] == "heads up"), "a top-level message from Claude is a thread");
        let fresh = body["fresh"].as_array().unwrap();
        assert_eq!(fresh.len(), 1);
        assert_eq!((fresh[0]["title"].as_str(), fresh[0]["project"].as_str()), (Some("fresh one"), Some("proj")));
    }

    #[tokio::test]
    async fn the_phone_can_approve_what_it_read_with_or_without_a_reason() {
        let w = world();
        call(&w, "GET", "/api/reviews/r.jsonl", None, true, None).await;
        let (_, diff) = call(&w, "GET", "/api/reviews/r.jsonl/diff", None, true, None).await;

        let stale = serde_json::json!({ "verdict": "approved", "digest": "0000" });
        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/verdict", Some(stale), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::CONFLICT, "an approval of a diff the phone did not read is not recorded");
        let elsewhere = serde_json::json!({ "verdict": "approved", "digest": diff["digest"] });
        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/verdict", Some(elsewhere.clone()), true, Some("another-mac")).await;
        assert_eq!(status, StatusCode::CONFLICT, "nor one meant for another machine");
        assert!(threshold_core::session_verdict(&w.ctx.inbox.join("r.jsonl")).is_none());

        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/verdict", Some(elsewhere), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::OK);
        let v = threshold_core::session_verdict(&w.ctx.inbox.join("r.jsonl")).unwrap();
        assert!(matches!(v.body, Body::Verdict { verdict: Verdict::Approved, ref reason, ref digest } if reason.is_none() && digest.as_deref() == diff["digest"].as_str()));
    }

    #[tokio::test]
    async fn a_reject_names_the_diff_that_was_read_and_needs_a_reason() {
        let w = world();
        // Opening pins it, as on the desktop.
        call(&w, "GET", "/api/reviews/r.jsonl", None, true, None).await;
        let (_, diff) = call(&w, "GET", "/api/reviews/r.jsonl/diff", None, true, None).await;

        let no_reason = serde_json::json!({ "verdict": "rejected", "digest": diff["digest"] });
        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/verdict", Some(no_reason), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let stale = serde_json::json!({ "verdict": "rejected", "reason": "no", "digest": "0000" });
        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/verdict", Some(stale), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::CONFLICT, "a reject of a diff the phone did not read is not recorded");

        let ok = serde_json::json!({ "verdict": "rejected", "reason": "drops the index", "digest": diff["digest"] });
        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/verdict", Some(ok.clone()), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::OK);
        let v = threshold_core::session_verdict(&w.ctx.inbox.join("r.jsonl")).unwrap();
        assert!(matches!(v.body, Body::Verdict { verdict: Verdict::Rejected, ref digest, .. } if digest.as_deref() == diff["digest"].as_str()));
        assert_eq!(threshold_core::load_decisions(&w.ctx.decisions).len(), 1);

        let (status, _) = call(&w, "POST", "/api/reviews/r.jsonl/verdict", Some(ok), true, Some(&w.machine)).await;
        assert_eq!(status, StatusCode::CONFLICT, "decided once");
    }
}
