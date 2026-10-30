// Threshold app shell: the inbox of reviews agents have handed over, and
// the review itself (brief, narrative, threads, verdict). Also reviews a
// repo's working tree, staged or revision diff directly. No framework.

import type { BundlePayload, Pairing, PhoneStatus, UiPlace, Update, Chapter, ChapterState, Decision, DiffFile, DiffSize, HunkRef, LineAnchor, Mark, NotificationTarget, Pin, Place, ReviewSummary, DiffFacts, SessionEvent, Verdict } from "./api";
import { activeReview, chapterStates, decide, hasSession, loadPlace, savePlace, initialRepo, initialRevision, lineAnchors, listDecisions, listReviews, fileText, loadBundle, loadDiff, markChapter, nativeNotifications, notify, openReview, phonePairing, phoneRevoke, phoneStatus, pickRepoFolder, postNote, reviewRepo, reviewSizes, sessionEvents, setArchived, fileImage, updateReady, restartForUpdate, skillStatus, installSkill, revealSkill, startSample, isSample, type SkillStatus } from "./api";
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { gapsAround, layout, open as openGap, sizeOf, spanOf, STEP, type Gap, type Opened } from "./context";
import { DraftMemory } from "./drafts";
import { arrange, projectName, projectsIn, type InboxView } from "./inbox";
import { fileRun, pathOf, runsOf, type FileRun, type HunkEntry } from "./outline";
import { ScrollMemory } from "./scroll";
import { changesOf, isMarkdown, renderMarkdown, resolveRepoPath } from "./markdown";
import { foldUnchanged } from "./mdfold";
import { formatTime, h } from "./util";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { initTheme, isDark } from "./theme";
import { openPicker } from "./picker";
import { runTour, tourRunning, type TourStep } from "./tour";
import { isRetired } from "./diagnostics";

const state = {
  repoPath: null as string | null,
  staged: false,
  // The revision spec being reviewed, e.g. "main...HEAD". Non-empty means
  // the working-tree/staged toggle does not apply.
  revision: "",
  // What the loaded diff actually came from, as reported by the backend,
  // rather than what the input currently reads — the two differ while a
  // spec is half-typed, and the decision must record the former.
  source: null as string | null,
  // The two commits the diff on screen was actually taken between. null
  // means the working tree is one side of it, and so what is on screen
  // can change while it is being read.
  pin: null as Pin | null,
  // SHA-256 of the diff on screen, quoted by the verdict so an approval
  // names the change rather than the question that produced it.
  digest: "",
  // Set when the backend reports that an unpinned diff has moved since
  // this one was rendered. Cleared only by reloading, so it cannot be
  // scrolled past.
  drifted: false,
  loading: false,
  error: null as string | null,
  files: [] as DiffFile[],
  summary: null as DiffFacts | null,
  // Whether a long list of deleted files is spelled out. Closed, the
  // count still says what matters; open, the list scrolls in its own box
  // so it cannot push the rest of the rail off screen.
  deletedOpen: false,
  selectedFile: 0,
  reason: "",
  decisions: [] as Decision[],
  view: "diff" as "diff" | "history" | "phone",
  // Phone access on this Mac: which machine it is, and which phones may
  // reach it. null until first asked.
  phone: null as PhoneStatus | null,
  // Whether Claude Code has the review skill; null until asked.
  skill: null as SkillStatus | null,
  skillBusy: false,
  // The QR on screen while the Phone view is open. Replaced before its
  // token expires, so what is on screen always works.
  pairing: null as Pairing | { error: string } | null,
  decided: null as Verdict | null,
  submitting: false,
  // The review conversation. `session` is false when Threshold was opened
  // by hand rather than handed a session by an agent, and the whole thread
  // affordance is hidden.
  session: false,
  events: [] as SessionEvent[],
  // The line the human is annotating, if any.
  composing: null as { file: string; line: number; content: string } | null,
  // The agent's narrative reading of this diff, and how completely it
  // accounts for it. null when it has not sent one for this revision.
  bundle: null as BundlePayload | null,
  // "files" is the ground truth and stays one click away at all times.
  view2: "files" as "files" | "narrative",
  // The inbox. Present whether or not any agent is running.
  reviews: [] as ReviewSummary[],
  // Diff size of each waiting review, by path. Arrives after the list.
  sizes: {} as Record<string, DiffSize>,
  activeReview: null as string | null,
  showInbox: true,
  // The inbox, or what has been archived. Not remembered: the archive is
  // somewhere you visit, and reopening on it would hide what is waiting.
  inboxView: "inbox" as InboxView,
  // Count of waiting reviews at last check, so a genuinely new one can be
  // distinguished from a review merely gaining events.
  waitingSeen: 0,
  seenEvents: new Map<string, number>(),
  toasts: [] as { id: number; text: string; path?: string; thread?: number | null }[],
  // Per-chapter mark and outstanding-note counts, folded from the log.
  chapterStates: [] as ChapterState[],
  // Where each line thread sits in the diff on screen, by root seq. Never
  // the line number a note was written with: that is only true of the
  // commits it was written against.
  anchors: new Map<number, LineAnchor>(),
  activeChapter: 0,
  // The chapter whose comment box is open, if any.
  composingChapter: null as string | null,
  // The thread open as a modal over the review pane, by its root seq.
  // Belongs to the review pane: opened from a note where it sits, and it
  // leaves the chat pane exactly as it was.
  openThread: null as number | null,
  // The thread open in the chat pane, or null for the channel. Belongs to
  // the chat pane, and nothing the review pane does moves it.
  chatThread: null as number | null,
  // Unchanged code opened around hunks, by diff digest and file, then gap
  // index. Keyed by digest so a revision, whose hunks sit elsewhere, starts
  // closed rather than opening the wrong lines.
  opened: {} as Record<string, Record<number, Opened>>,
  // The new side of each file whose context has been asked for.
  fileText: new Map<string, string[] | "loading" | { error: string }>(),
  // Images from the diff's commits, by digest, side and path.
  images: new Map<string, string | "loading" | { error: string }>(),
  // Rendered Markdown runs the reviewer has unfolded in full, by run.
  mdUnfolded: new Set<string>(),
  // Whether OS notifications will be shown, and report their clicks.
  native: false,
  // Where the reviewer was, per review and chapter. Persisted so a review
  // read across two sittings does not start over.
  place: { review: null, scroll: {}, chapter: {}, mode: {}, read: {} } as Place,
  // A newer build waiting to be restarted into, if any.
  update: null as Update | null,
};
const app = document.getElementById("app")!;

/** Load the diff for `path`.
 *
 *  `keepPlace` is for reloading the review already being read — a
 *  revision arriving — where the reviewer's file, view and chapter are
 *  theirs, not something the reload gets to choose again. */
async function loadFor(path: string, keepPlace = false): Promise<void> {
  const keepFile = keepPlace ? fileKey(state.files[state.selectedFile]) : null;
  state.repoPath = path;
  state.loading = true;
  state.error = null;
  state.decided = null;
  render();
  try {
    const payload = await loadDiff(path, state.staged, state.revision.trim() || null);
    state.files = payload.files;
    state.summary = payload.summary;
    state.source = payload.source;
    state.pin = payload.pin;
    if (payload.digest !== state.digest) {
      state.opened = {};
      state.fileText.clear();
      state.images.clear();
    }
    state.digest = payload.digest;
    state.drifted = false;
    state.selectedFile = Math.max(0, keepFile ? state.files.findIndex((f) => fileKey(f) === keepFile) : 0);
    state.bundle = state.session ? await loadBundle(path, state.staged, state.revision.trim() || null) : null;
    await refreshChapterStates();
    // A narrative that no longer exists for this revision must not leave
    // the reviewer looking at a stale one.
    // Open on the narrative when there is one — it is the better first
    // read — but only when it accounts for the whole diff. A partial
    // narrative must not be the first thing seen.
    if (!state.bundle) state.view2 = "files";
    // Only when the reviewer has not already chosen for this review: a
    // saved choice is theirs, and overriding it on every open is why a
    // restart used to put them back on the narrative.
    else if (!keepPlace && !(state.activeReview && state.place.mode[state.activeReview]) && state.bundle.coverage.unbundled.length === 0 && state.bundle.coverage.phantom.length === 0) state.view2 = "narrative";
    if (state.bundle) state.activeChapter = Math.max(0, Math.min(state.activeChapter, lastChapterIndex()));
  } catch (e) {
    state.error = String(e);
    state.files = [];
    state.summary = null;
    state.source = null;
    state.pin = null;
    state.digest = "";
  } finally {
    state.loading = false;
    render();
  }
}

async function refreshEvents(): Promise<void> {
  if (!state.session) return;
  state.events = await sessionEvents();
  await refreshChapterStates();
  render();
}

/** Everything folded from the log and the diff together: chapter tallies,
 *  and where each line thread now sits. Refreshed whenever either moves. */
async function refreshChapterStates(): Promise<void> {
  state.anchors = state.session ? new Map((await lineAnchors().catch(() => [])).map((a) => [a.seq, a])) : new Map();
  const chs = state.bundle?.chapters ?? [];
  // Each chapter's files, so flags can be tallied per chapter: the log
  // knows which file a flag is on and nothing about chapters.
  const files = chs.map((c) => [...new Set(c.hunks.map((hk) => hk.file))]);
  state.chapterStates = chs.length ? await chapterStates(chs.map((c) => c.title), files) : [];
}

// Every composer's unsent text, by key. See drafts.ts for the rule;
// render() is the only thing that applies it.
const drafts = new DraftMemory();
// A render that arrived while an input method was composing, owed once it ends.
let renderWaiting = false;

/** Set once launch has put the saved place back. Until then the screen
 *  is the default one, and saving it would overwrite what was saved. */
let restored = false;
let savedUi = "";

/** Remember how the window looks now, if that changed since it was last
 *  saved. Called after every render, so a toggle is remembered by being
 *  on screen rather than by each toggle remembering to save itself. */
function persistUi(): void {
  if (!restored) return;
  const ui: UiPlace = { ...(state.place.ui ?? {}) };
  ui.view = state.view;
  ui.show_inbox = state.showInbox;
  ui.deleted_open = state.deletedOpen;
  ui.folder = !state.activeReview && state.repoPath ? { repo: state.repoPath, staged: state.staged, revision: state.revision } : null;
  const review = state.activeReview;
  if (review) {
    const file = fileKey(state.files[state.selectedFile]);
    if (file && !state.loading) ui.file = { ...ui.file, [review]: file };
    const thread = { ...ui.thread };
    if (state.chatThread != null) thread[review] = state.chatThread;
    else delete thread[review];
    ui.thread = thread;
    state.place.review = review;
    state.place.chapter[review] = state.activeChapter;
  }
  const text = JSON.stringify([ui, review, state.activeChapter, review ? state.place.mode[review] : null]);
  if (text === savedUi) return;
  savedUi = text;
  state.place.ui = ui;
  void savePlace(state.place);
}

// Every scrolling region's position, by key. See scroll.ts for the rule;
// render() is the only thing that applies it.
const scroll = new ScrollMemory();

/** A scroll key scoped to the review being read, so two reviews' chapter
 *  threes are different places. */
function scrollKey(region: string): string {
  return `${state.activeReview ?? ""}|${region}`;
}

/** The key of the review pane's reading surface at a location. */
function readingKey(loc: { view2: "files" | "narrative"; chapter: number; file: number }): string {
  return scrollKey(loc.view2 === "narrative" ? `narrative:${loc.chapter}` : `files:${fileKey(state.files[loc.file]) ?? ""}`);
}

/** Persist where the reviewer is: review, chapter, view, and every scroll
 *  position in it, so a review read across two sittings does not start
 *  over. */
function rememberPlace(): void {
  const key = state.activeReview;
  if (!key) return;
  state.place.review = key;
  state.place.chapter[key] = state.activeChapter;
  state.place.scroll[key] = scroll.snapshot(`${key}|`);
  void savePlace(state.place);
}

/** The index of the last page of the narrative, counting the unbundled
 *  section when there is one. */
function lastChapterIndex(): number {
  return (state.bundle?.chapters.length ?? 0) + (state.bundle?.coverage.unbundled.length ? 1 : 0) - 1;
}

function fileKey(f: DiffFile | undefined): string | null {
  return f ? f.new_path ?? f.old_path ?? null : null;
}

/** A place in the review pane: enough to put the reader back exactly. */
interface Loc {
  view2: "files" | "narrative";
  chapter: number;
  file: number;
  scroll: number;
}

// Back and forward for the review pane, like a browser's. Only the review
// pane has history: the chat pane moving is never a reason to lose where
// you were reading, and going back in one must not move the other.
const nav = { back: [] as Loc[], forward: [] as Loc[] };

function here(): Loc {
  const loc = { view2: state.view2, chapter: state.activeChapter, file: state.selectedFile };
  return { ...loc, scroll: scroll.get(readingKey(loc)) };
}

function samePage(a: Loc, b: Loc): boolean {
  return a.view2 === b.view2 && (a.view2 === "narrative" ? a.chapter === b.chapter : a.file === b.file);
}

/** Record where the reader is before taking them somewhere else. */
function pushNav(): void {
  const at = here();
  const top = nav.back[nav.back.length - 1];
  // Twenty pixels of scrolling on one page is not a place worth a step.
  if (top && samePage(top, at) && Math.abs(top.scroll - at.scroll) < 20) return;
  nav.back.push(at);
  if (nav.back.length > 100) nav.back.shift();
  nav.forward = [];
}

function showLoc(loc: Loc): void {
  state.view2 = loc.view2;
  state.activeChapter = loc.chapter;
  state.selectedFile = loc.file;
  scroll.land(readingKey(loc), loc.scroll);
  render();
  rememberPlace();
}

function navBack(): void {
  const to = nav.back.pop();
  if (!to) return;
  nav.forward.push(here());
  showLoc(to);
}

function navForward(): void {
  const to = nav.forward.pop();
  if (!to) return;
  nav.back.push(here());
  showLoc(to);
}

/** Show a chapter, restoring where you last were in it.
 *
 *  Its own remembered offset, never the previous chapter's — arriving
 *  mid-chapter at an offset that belonged to something else is the
 *  disorientation that made chapters pages in the first place. A chapter
 *  opened for the first time starts at the top. */
function jumpToChapter(i: number): void {
  const to = Math.max(0, i);
  if (state.view2 === "narrative" && state.activeChapter === to) return;
  pushNav();
  state.activeChapter = to;
  state.view2 = "narrative";
  render();
  rememberPlace();
}

function selectFile(i: number): void {
  if (state.view2 === "files" && state.selectedFile === i) return;
  pushNav();
  state.selectedFile = i;
  state.view2 = "files";
  render();
  rememberPlace();
}

function setView(v: "files" | "narrative"): void {
  if (state.view2 === v) return;
  pushNav();
  state.view2 = v;
  // Saved only here, where the reviewer chose it. A review with no
  // narrative opens on Files because it has to; saving that as a choice
  // would keep it there after Claude sends one.
  if (state.activeReview) state.place.mode[state.activeReview] = v;
  render();
  rememberPlace();
}

function stepChapter(delta: number): void {
  if (!state.bundle?.chapters.length) return;
  jumpToChapter(Math.min(lastChapterIndex(), Math.max(0, state.activeChapter + delta)));
}

async function setMark(chapter: string, mark: Mark, advance = false): Promise<void> {
  state.events = await markChapter(chapter, mark);
  await refreshChapterStates();
  // Reading and signing off is a sequence, so marking a chapter read
  // carries you to the next one. Flagging a concern does not — you are
  // stopping to say something, not moving on.
  if (advance && state.activeChapter < lastChapterIndex()) jumpToChapter(state.activeChapter + 1);
  else render();
}

/** Post a note and fold the result in, without disturbing either pane. */
async function send(
  draftKey: string,
  at: { file?: string | null; line?: number | null; content?: string | null; chapter?: string | null; parent?: number | null } = {},
): Promise<boolean> {
  const taken = drafts.take(draftKey);
  const text = taken.trim();
  if (!text) {
    drafts.putBack(draftKey, taken);
    return false;
  }
  try {
    state.events = await postNote(text, at.file ?? null, at.line ?? null, at.content ?? null, at.chapter ?? null, at.parent ?? null);
    await refreshChapterStates();
  } catch (e) {
    // Put it back: a failed send must not cost the reviewer what they wrote.
    drafts.putBack(draftKey, taken);
    state.error = String(e);
  }
  render();
  return true;
}

/** A textarea bound to a named draft. Its text, focus and cursor belong
 *  to drafts.ts, which render() applies; nothing here sets them. */
function composer(key: string, placeholder: string, rows: number, onSubmit: () => void, onCancel: () => void): HTMLTextAreaElement {
  return h("textarea", {
    class: "field composer",
    placeholder,
    rows: String(rows),
    "data-draft": key,
    onInput: (e) => drafts.input(key, (e.target as HTMLTextAreaElement).value),
    onCompositionStart: () => drafts.compositionStart(key),
    onCompositionEnd: () => { drafts.compositionEnd(); if (renderWaiting) render(); },
    onKeyDown: (e) => {
      const ev = e as KeyboardEvent;
      if (ev.isComposing) return;
      if (ev.key === "Enter" && !ev.shiftKey) { ev.preventDefault(); onSubmit(); }
      if (ev.key === "Escape") { ev.preventDefault(); ev.stopPropagation(); onCancel(); }
    },
  });
}

/** Reviews waiting on a decision. Archived ones have been put away. */
function waitingCount(): number {
  return state.reviews.filter((r) => !r.decided && !r.archived).length;
}

function currentReviewTitle(): string {
  return state.reviews.find((r) => r.path === state.activeReview)?.title ?? "Review";
}

let toastId = 0;
let notifyAllowed = false;

/** Ask once, at startup, so the first real notification is not also a
 *  permission prompt the user has to dismiss before seeing it. */
async function initNotifications(): Promise<void> {
  try {
    notifyAllowed = await isPermissionGranted();
    if (!notifyAllowed) notifyAllowed = (await requestPermission()) === "granted";
  } catch {
    notifyAllowed = false;
  }
}

/** Tell the user something arrived, and where a click should take them.
 *
 *  Through the OS whenever it can, whether or not Threshold is in front:
 *  the reviewer is reading the diff, not watching the conversation, and a
 *  native banner is the one thing that reaches them without moving
 *  anything on screen. Clicking it comes back as `notification-open`.
 *
 *  Without native notifications — an unbundled dev build, or permission
 *  refused — an in-app toast does the same job, plus the plugin's
 *  notification when the window is behind other work. */
function toast(title: string, body: string, target: NotificationTarget, subtitle: string | null = null): void {
  void (async () => {
    if (state.native) {
      try {
        if (await notify(title, subtitle, body, target.review, target.thread)) return;
      } catch {
        // Fall through: a failed notification must not lose the message.
      }
    }
    const id = ++toastId;
    const text = subtitle ? `${title} · ${subtitle}` : title;
    state.toasts.push({ id, text, path: target.review ?? undefined, thread: target.thread });
    render();
    setTimeout(() => {
      state.toasts = state.toasts.filter((t) => t.id !== id);
      render();
    }, 6000);
    if (notifyAllowed && !document.hasFocus()) {
      try {
        sendNotification({ title: "Threshold", body: `${text}\n${body}` });
      } catch {
        // A failed notification must never take the in-app toast with it.
      }
    }
  })();
}

/** Follow a notification: to its thread, in the chat pane.
 *
 *  The review pane stays where it is. Reviewing chapter four while
 *  answering a question about chapter one is the point of having two
 *  panes; a notification that dragged the diff to chapter one would make
 *  them one pane again. Only a different review moves it, because the
 *  thread cannot be shown without its review. */
async function openTarget(target: NotificationTarget): Promise<void> {
  if (target.review && target.review !== state.activeReview) {
    const r = state.reviews.find((x) => x.path === target.review);
    if (!r) return;
    await selectReview(r);
  }
  if (target.thread != null) openChatThread(target.thread);
  else render();
}

function renderToasts(): HTMLElement | string {
  if (!state.toasts.length) return "";
  return h(
    "div",
    { class: "toasts" },
    ...state.toasts.map((t) =>
      h(
        "button",
        {
          class: "toast",
          onClick: () => {
            state.toasts = state.toasts.filter((x) => x.id !== t.id);
            void openTarget({ review: t.path ?? null, thread: t.thread ?? null });
          },
        },
        h("span", { class: "toast-dot" }),
        h("span", { text: t.text }),
      ),
    ),
  );
}

async function refreshReviews(): Promise<void> {
  state.reviews = await listReviews();
  render();
  void refreshSizes();
}

// One sizing at a time: it runs git once per unpinned review, and the
// inbox can change several times a second while an agent is talking.
// A change arriving mid-run is owed one more run, not one each.
let sizing = false;
let sizingOwed = false;
async function refreshSizes(): Promise<void> {
  if (sizing) { sizingOwed = true; return; }
  sizing = true;
  try {
    do {
      sizingOwed = false;
      state.sizes = await reviewSizes().catch(() => state.sizes);
      render();
    } while (sizingOwed);
  } finally {
    sizing = false;
  }
}

/** "1,240 lines · +1,100 −140": the total first, since it is what time is
 *  budgeted by, then which way it cuts. */
function sizeParts(added: number, removed: number): HTMLElement[] {
  const n = (x: number) => x.toLocaleString();
  return [
    h("span", { class: "size-total", text: `${n(added + removed)} line${added + removed === 1 ? "" : "s"}` }),
    h("span", { class: "file-stat added", text: `+${n(added)}` }),
    h("span", { class: "file-stat removed", text: `−${n(removed)}` }),
  ];
}

/** Switch the whole review context: conversation, repo, revision, diff. */
async function selectReview(r: ReviewSummary): Promise<void> {
  rememberPlace();
  state.activeReview = r.path;
  scroll.load(`${r.path}|`, state.place.scroll[r.path]);
  // Both panes' positions belong to the review they were in.
  state.chatThread = null;
  state.openThread = null;
  nav.back = [];
  nav.forward = [];
  state.activeChapter = state.place.chapter[r.path] ?? 0;
  const savedMode = state.place.mode[r.path];
  if (savedMode === "files" || savedMode === "narrative") state.view2 = savedMode;
  // Reviews navigates between reviews; the contents navigates within one.
  // Both open at once leaves the diff — the evidence — as the narrowest
  // column on screen. Entering a review is going a level down, so the
  // list steps out of the way; the header button brings it back.
  state.showInbox = false;
  state.events = await openReview(r.path);
  state.session = true;
  state.decided = null;
  state.revision = r.revision ?? [...state.events].reverse().find((e) => e.revision)?.revision ?? "";
  const repo = (await reviewRepo()) ?? r.repo;
  if (repo) await loadFor(repo);
  // Where the reviewer was inside this review: the file they had
  // selected and the thread they had open, when both still exist.
  const ui = state.place.ui;
  const file = ui?.file?.[r.path];
  const f = file ? state.files.findIndex((x) => fileKey(x) === file) : -1;
  if (f >= 0) state.selectedFile = f;
  const thread = ui?.thread?.[r.path];
  if (thread != null && state.events.some((e) => e.seq === thread)) state.chatThread = thread;
  render();
}

async function openFolder(): Promise<void> {
  const folder = await pickRepoFolder();
  if (folder) await loadFor(folder);
}

async function toggleStaged(staged: boolean): Promise<void> {
  if (state.staged === staged) return;
  state.staged = staged;
  // Picking working-tree or staged means abandoning a revision: the
  // backend ignores `staged` whenever a revision is set, so leaving the
  // field populated would show a diff the button did not ask for.
  state.revision = "";
  if (state.repoPath) await loadFor(state.repoPath);
}

async function applyRevision(spec: string): Promise<void> {
  if (state.revision === spec) return;
  state.revision = spec;
  if (state.repoPath) await loadFor(state.repoPath);
}

async function refresh(): Promise<void> {
  if (state.repoPath) await loadFor(state.repoPath);
}

async function submitDecision(verdict: Verdict): Promise<void> {
  if (!state.summary) return;
  state.submitting = true;
  state.error = null;
  render();
  try {
    state.decisions = await decide(state.summary, verdict, state.reason.trim() || null, state.source, state.digest || null);
    state.decided = verdict;
    state.reason = "";
  } catch (e) {
    state.error = String(e);
  } finally {
    state.submitting = false;
    render();
  }
}

async function openHistory(): Promise<void> {
  state.decisions = await listDecisions();
  state.view = "history";
  render();
}

let pairingTimer: number | undefined;

async function openPhone(): Promise<void> {
  state.view = "phone";
  window.clearInterval(pairingTimer);
  const tick = async () => {
    // Left the view: stop minting tokens nobody will scan.
    if (state.view !== "phone") return window.clearInterval(pairingTimer);
    state.phone = await phoneStatus();
    state.pairing = state.phone.enabled ? await phonePairing().catch((e) => ({ error: String(e) })) : null;
    render();
  };
  await tick();
  // Refreshed well inside the token's minute, and the device list with
  // it, so a phone that has just paired appears without a click.
  pairingTimer = window.setInterval(() => void tick(), 20_000);
}

async function revokeDevice(id: string): Promise<void> {
  const devices = await phoneRevoke(id);
  if (state.phone) state.phone = { ...state.phone, devices };
  render();
}

/** An SVG produced by the backend, as a node. Parsed rather than set as
 *  HTML, so nothing in it can run. */
function svgNode(svg: string): Node {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  return document.importNode(doc.documentElement, true);
}

/** The Phone view: which Mac this is, a QR to pair a phone, and the
 *  phones already paired.
 *
 *  The machine's name and colour lead, because they are what the phone
 *  will show on every screen; seeing them here first is how the person
 *  learns which colour means this Mac. */
function renderPhone(): HTMLElement {
  const back = h("button", { class: "btn ghost small back-btn", text: "← Back", onClick: () => { state.view = "diff"; render(); } });
  const st = state.phone;
  if (!st) return h("div", { class: "phone-view" }, back, h("p", { class: "muted", text: "Loading…" }));
  if (!st.enabled || !st.machine) {
    return h(
      "div",
      { class: "phone-view" },
      back,
      h("h2", { text: "Phone access is off" }),
      h("p", { class: "muted", text: "Reach this Mac's reviews from your phone over Tailscale. From the Threshold repo, run:" }),
      h("pre", { class: "phone-cmd", text: "scripts/threshold-mobile-setup.sh" }),
      h("p", { class: "muted", text: "It installs Tailscale if needed, names this Mac, and restarts Threshold." }),
    );
  }
  const m = st.machine;
  const p = state.pairing;
  return h(
    "div",
    { class: "phone-view" },
    back,
    h(
      "div",
      { class: "phone-machine", style: `--machine: ${m.color}` },
      h("span", { class: "phone-swatch" }),
      h("div", {}, h("div", { class: "phone-name", text: m.name }), h("div", { class: "muted phone-url", text: st.url ?? "" })),
    ),
    h("p", { class: "muted", text: `Your phone shows “${m.name}” and this colour on every screen for this Mac.` }),
    h("h3", { text: "Pair a phone" }),
    !p
      ? h("p", { class: "muted", text: "Loading…" })
      : "error" in p
        ? h("div", { class: "error", text: p.error })
        : h(
            "div",
            { class: "phone-pair" },
            h("div", { class: "phone-qr" }, svgNode(p.svg)),
            h(
              "ol",
              { class: "phone-steps" },
              h("li", { text: "Install Tailscale on the phone and log in with the same account." }),
              h("li", { text: "Scan this code with the camera. It works once, and is replaced every 20 seconds." }),
              h("li", { text: `In Chrome: ⋮ → Add to Home screen. The icon is labelled “${m.name}”. (iPhone: Safari → Share → Add to Home Screen.)` }),
            ),
          ),
    h("h3", { text: "Paired phones" }),
    st.devices.length
      ? h(
          "div",
          {},
          ...st.devices.map((d) =>
            h(
              "div",
              { class: "history-row" },
              h("span", { class: "history-reason", text: d.name }),
              h("span", { class: "history-time muted", text: `paired ${formatTime(d.paired_at_ms)}` }),
              h("span", { class: "history-time muted", text: `seen ${ago(d.last_seen_ms)}` }),
              h("button", { class: "btn ghost small", text: "Revoke", onClick: () => void revokeDevice(d.id) }),
            ),
          ),
        )
      : h("p", { class: "muted", text: "None yet." }),
  );
}

/** Past this many deleted files the list folds behind its count. */
const DELETED_INLINE = 3;

/** The diff's facts, rendered at the foot of the rail directly above the
 *  buttons: the last thing read before deciding, not a banner already
 *  scrolled past. */
function renderFacts(summary: DiffFacts): HTMLElement {
  const label = state.revision ? state.revision : state.staged ? "staged" : "working tree";
  const warnings: HTMLElement[] = [];
  if (summary.has_deletions) {
    const n = summary.deleted_files.length;
    const count = `deletes ${n} file${n === 1 ? "" : "s"}`;
    warnings.push(
      n <= DELETED_INLINE
        ? h("div", { class: "facts-warn" }, h("span", { text: "⚠" }), h("span", { text: `${count}: ${summary.deleted_files.join(", ")}` }))
        : h(
            "div",
            { class: "facts-warn" },
            h("span", { text: "⚠" }),
            h(
              "div",
              { class: "facts-deleted" },
              h("button", {
                class: "facts-toggle",
                text: `${count} ${state.deletedOpen ? "▾" : "▸"}`,
                onClick: () => {
                  state.deletedOpen = !state.deletedOpen;
                  render();
                },
              }),
              state.deletedOpen ? h("ul", { class: "facts-files" }, summary.deleted_files.map((f) => h("li", { text: f, title: f }))) : "",
            ),
          ),
    );
  }
  if (summary.touches_tests) {
    warnings.push(h("div", { class: "facts-warn tests" }, h("span", { text: "⚠" }), h("span", { text: `touches ${summary.test_files.length} test file${summary.test_files.length === 1 ? "" : "s"}` })));
  }
  // Whether what is on screen can move is a fact about the change worth
  // as much as how many files it touches, and it belongs in the same
  // place: the last thing read before deciding.
  if (!state.pin && state.session) {
    warnings.push(
      h(
        "div",
        { class: "facts-warn unpinned" },
        h("span", { text: "⚠" }),
        h("span", { text: "not pinned: the working tree is one side of this diff, so it can change while you read it" }),
      ),
    );
  }
  return h(
    "div",
    { class: "facts" },
    h(
      "div",
      { class: "facts-source" },
      h("span", { text: label }),
      // The commit, abbreviated the way git does. Quiet, because a pinned
      // review is the expected case — it is its absence that needs to
      // shout, and that is the warning above.
      state.pin ? h("span", { class: "facts-pin", title: `pinned to ${state.pin.base} … ${state.pin.head}`, text: `pinned ${state.pin.head.slice(0, 7)}` }) : "",
    ),
    h(
      "div",
      { class: "facts-counts" },
      h("span", { text: `${summary.files_changed} file${summary.files_changed === 1 ? "" : "s"}` }),
      h("span", { class: "added", text: `+${summary.lines_added}` }),
      h("span", { class: "removed", text: `−${summary.lines_removed}` }),
    ),
    ...warnings,
  );
}

function statusBadge(status: DiffFile["status"]): HTMLElement {
  return h("span", { class: `status-badge status-${status}`, text: status[0].toUpperCase() });
}

/** Take the reading pane to a file run or a hunk on the page already
 *  shown. Recorded in the review pane's history first, like any other
 *  jump, so Back returns to where the reader was. */
function jumpWithin(selector: string): void {
  const el = document.querySelector<HTMLElement>(`.reading ${selector}`);
  if (!el) return;
  pushNav();
  // Instant, for the reason jumpToAnchor gives.
  el.scrollIntoView({ block: "start" });
  const pane = el.closest<HTMLElement>("[data-scroll]");
  if (pane) scroll.record(pane);
  const divider = el.querySelector<HTMLElement>(":scope > .file-band, :scope > .hunk-header");
  if (divider) {
    divider.classList.add("just-jumped");
    setTimeout(() => divider.classList.remove("just-jumped"), 1600);
  }
  spy();
}

/** The page's files and hunks, beneath its row in the navigator: the
 *  third and fourth levels, under the review and the chapter. `files` is
 *  false in the Files view, where the file is the row itself. */
function renderOutline(runs: FileRun[], files: boolean): HTMLElement | string {
  if (!runs.some((r) => r.hunks.length)) return "";
  return h(
    "div",
    { class: `outline${files ? "" : " hunks-only"}` },
    ...runs.flatMap((run, k) => [
      files
        ? h(
            "button",
            { class: "outline-file", "data-nav-run": String(k), title: run.path, onClick: () => jumpWithin(`[data-run="${k}"]`) },
            statusBadge(run.file.status),
            h("span", { class: "outline-name", text: run.path.slice(run.path.lastIndexOf("/") + 1) }),
            h("span", { class: "outline-counts" }, ...(run.file.binary ? [h("span", { text: "binary" })] : [h("span", { class: "file-stat added", text: `+${run.added}` }), h("span", { class: "file-stat removed", text: `−${run.removed}` })])),
          )
        : "",
      // A run of one hunk is its file row already; a hunk row under it
      // would only say the same thing twice.
      ...(files && run.hunks.length === 1 ? [] : run.hunks).map((e) =>
        h(
          "button",
          {
            class: "outline-hunk",
            "data-nav-hunk": e.key,
            title: `${run.path} ${e.lines}${e.context ? `  ${e.context}` : ""}`,
            onClick: () => jumpWithin(`[data-hunk="${CSS.escape(e.key)}"]`),
          },
          h("span", { class: "outline-lines", text: e.lines }),
          h("span", { class: "outline-context", text: e.context }),
          h("span", { class: "outline-counts" }, h("span", { class: "file-stat added", text: `+${e.added}` }), h("span", { class: "file-stat removed", text: `−${e.removed}` })),
        ),
      ),
    ]),
  );
}

// What the navigator last marked as on screen, so it is only brought into
// view when that changes.
let spied = "";

/** Mark, in the navigator, the file and hunk the reader is looking at.
 *
 *  The hunk is the last one whose top has passed a line a little below the
 *  top of the pane; at the very bottom, where a short last hunk can never
 *  reach that line, the last one on screen. Done by toggling classes rather
 *  than rendering: it runs on every scroll frame. */
function spy(): void {
  const pane = document.querySelector<HTMLElement>(".reading .diff-view");
  const nav = document.querySelector<HTMLElement>(".main-col > .toc, .main-col > .file-list");
  if (!pane || !nav) return;
  const box = pane.getBoundingClientRect();
  const atBottom = pane.scrollTop + pane.clientHeight >= pane.scrollHeight - 2;
  const line = atBottom ? box.bottom - 1 : box.top + Math.min(120, pane.clientHeight / 3);
  let hunk: HTMLElement | null = null;
  for (const el of pane.querySelectorAll<HTMLElement>("[data-hunk]")) {
    if (el.getBoundingClientRect().top <= line) hunk = el;
    else break;
  }
  const run = hunk?.closest<HTMLElement>("[data-run]")?.dataset.run;
  const key = hunk ? `${state.view2}|${state.activeChapter}|${state.selectedFile}|${run}|${hunk.dataset.hunk}` : "";

  for (const el of nav.querySelectorAll(".in-view")) el.classList.remove("in-view");
  if (!hunk) { spied = ""; return; }
  const runRow = nav.querySelector<HTMLElement>(`[data-nav-run="${run}"]`);
  // A chapter can list the same hunk twice; the one between this run's
  // row and the next run's. A run of one hunk has no hunk row at all.
  const nextRun = nav.querySelector<HTMLElement>(`[data-nav-run="${Number(run) + 1}"]`);
  const hunkRow = [...nav.querySelectorAll<HTMLElement>(`[data-nav-hunk="${CSS.escape(hunk.dataset.hunk ?? "")}"]`)].find(
    (r) =>
      (!runRow || r.compareDocumentPosition(runRow) & Node.DOCUMENT_POSITION_PRECEDING) &&
      (!nextRun || r.compareDocumentPosition(nextRun) & Node.DOCUMENT_POSITION_FOLLOWING),
  );
  runRow?.classList.add("in-view");
  hunkRow?.classList.add("in-view");
  // Keep it visible as the reading moves on, but never tug the list while
  // the reader has the pointer in it.
  const shown = hunkRow ?? runRow;
  if (key !== spied && shown && !nav.matches(":hover")) shown.scrollIntoView({ block: "nearest" });
  spied = key;
}

function renderFileList(): HTMLElement {
  const rows = state.files.map((f, i) => {
    const path = f.old_path && f.new_path && f.old_path !== f.new_path ? `${f.old_path} → ${f.new_path}` : (f.new_path ?? f.old_path ?? "");
    // Name first, directory demoted beneath: a truncated full path makes
    // two files in the same directory indistinguishable, which is the
    // common case in a real change.
    const cut = path.lastIndexOf("/");
    const base = cut >= 0 ? path.slice(cut + 1) : path;
    const dir = cut >= 0 ? path.slice(0, cut) : "";
    const active = state.selectedFile === i;
    return [
      h(
        "button",
        {
          class: `file-row${active ? " active" : ""}`,
          title: path,
          // The file already open: back to its top.
          onClick: () => (active && state.view2 === "files" ? jumpWithin(`[data-run="0"]`) : selectFile(i)),
        },
        h("div", { class: "file-name" }, statusBadge(f.status), h("span", { class: "file-base", text: base })),
        dir ? h("div", { class: "file-dir", text: dir }) : "",
        h(
          "div",
          { class: "file-counts" },
          ...(f.binary
            ? [h("span", { class: "file-hunks", text: "binary" })]
            : [h("span", { class: "file-stat added", text: `+${f.additions}` }), h("span", { class: "file-stat removed", text: `−${f.deletions}` })]),
          f.hunks.length > 1 ? h("span", { class: "file-hunks", text: `${f.hunks.length} hunks` }) : "",
        ),
      ),
      // The open file's hunks, under it. Only when this is the page being
      // read: a row in the solo layout's list is still the file view.
      active && (state.view2 === "files" || !state.session) ? renderOutline([fileRun(f)], false) : "",
    ];
  });
  return h("div", { class: "file-list", "data-scroll": scrollKey("file-list") }, ...rows.flat());
}

/** A thread where it was written. Compact on purpose — enough to see
 *  that a line was questioned and roughly what about, with the modal for
 *  actually reading and continuing it. */
const FLAG_GLYPH: Record<string, string> = { attention: "◆", caution: "▲", context: "•" };
const FLAG_WORD: Record<string, string> = { attention: "look here", caution: "careful", context: "for context" };

function flagGlyph(f: string): string {
  return FLAG_GLYPH[f] ?? "•";
}

/** Where a line thread is drawn: its resolved anchor, or — before
 *  anchors have loaded — where it was written. */
function placeOf(note: SessionEvent): { file: string; line: number; status: LineAnchor["status"] } | null {
  const a = state.anchors.get(note.seq);
  if (a) return a;
  return note.file && note.line != null ? { file: note.file, line: note.line, status: "current" } : null;
}

/** The note's own record of the code it was about, for a note whose code
 *  the revision changed. It is still worth reading; it is no longer a
 *  claim about the line it sits beside. */
function renderStaleAnchor(note: SessionEvent): HTMLElement {
  return h(
    "div",
    { class: "note-stale" },
    h("span", { class: "who stale", text: "outdated" }),
    h("span", { text: `written about: ${note.line_content ?? ""}` }),
  );
}

function renderInlineNote(note: SessionEvent, stale = false): HTMLElement {
  const rest = threadOf(note.seq).slice(1);
  const latest = rest[rest.length - 1];
  const unread = unreadIn(note.seq);
  return h(
    "button",
    {
      class: `inline-note${stale ? " stale" : ""}${unread ? " unread" : ""}`,
      title: stale ? "This annotation was written against a different version of this line" : "Open this thread",
      // Here, over the review pane, rather than in the chat pane: reading
      // a note where it sits is part of reading the code, and the chat
      // pane may be in the middle of a different conversation.
      onClick: () => openThread(note.seq),
    },
    stale ? renderStaleAnchor(note) : "",
    h(
      "div",
      { class: "note-body" },
      note.flag
        ? h("span", { class: `who flag ${note.flag}`, text: `${flagGlyph(note.flag)} ${FLAG_WORD[note.flag] ?? ""}` })
        : h("span", { class: `who ${note.from === "agent" ? "agent" : "human"}`, text: note.from === "agent" ? "claude" : "you" }),
      h("span", { text: note.text ?? "" }),
    ),
    latest
      ? h(
          "div",
          { class: "note-body reply" },
          h("span", { class: `who ${latest.from}`, text: latest.from === "agent" ? "claude" : "you" }),
          h("span", { class: "note-latest", text: latest.text ?? "" }),
        )
      // Only a question is waiting on an answer. A flag is Claude's own
      // pointer, so saying it awaits Claude reads as a stuck thread when
      // nothing is stuck.
      : note.flag || note.from === "agent"
        ? h("div", { class: "note-more", text: "reply" })
        : h("div", { class: "note-pending", text: "waiting for Claude…" }),
    rest.length > 1 || unread
      ? h("div", { class: "note-more" }, unread ? h("span", { class: "unread-dot" }) : "", h("span", { text: unread ? `${unread} new — open` : `${rest.length} replies — open` }))
      : "",
  );
}

/** Every event in this thread, root first. */
function threadOf(root: number): SessionEvent[] {
  const ids = new Set<number>([root]);
  for (const e of state.events) {
    if (e.in_reply_to != null && ids.has(e.in_reply_to)) ids.add(e.seq);
  }
  return state.events.filter((e) => ids.has(e.seq));
}

/** The root of the thread an event belongs to. */
function rootOf(e: SessionEvent): number {
  let cur = e;
  const bySeq = new Map(state.events.map((x) => [x.seq, x]));
  for (let guard = 0; cur.in_reply_to != null && guard < 1000; guard++) {
    const parent = bySeq.get(cur.in_reply_to);
    if (!parent) break;
    cur = parent;
  }
  return cur.seq;
}

/** How many of Claude's messages in a thread the reviewer has not seen.
 *
 *  A flag is not counted: it is a pointer written before the review began,
 *  and forty of them reading as forty unread messages would bury the one
 *  reply that matters. */
function unreadIn(root: number): number {
  const key = state.activeReview;
  if (!key) return 0;
  const seen = state.place.read[key]?.[String(root)] ?? 0;
  return threadOf(root).filter((e) => e.from === "agent" && e.seq > seen && !(e.seq === root && e.flag)).length;
}

function markRead(root: number): void {
  const key = state.activeReview;
  if (!key) return;
  const last = Math.max(...threadOf(root).map((e) => e.seq));
  const read = (state.place.read[key] ??= {});
  if ((read[String(root)] ?? 0) >= last) return;
  read[String(root)] = last;
  void savePlace(state.place);
}

/** The threads being looked at right now, in either pane. */
function visibleThreads(): Set<number> {
  const out = new Set<number>();
  if (state.openThread != null) out.add(state.openThread);
  if (state.chatThread != null) out.add(state.chatThread);
  return out;
}

function openThread(root: number): void {
  state.openThread = root;
  drafts.focus(`modal:${root}`);
  markRead(root);
  render();
}

function closeThread(): void {
  state.openThread = null;
  render();
}

/** Open a thread in the chat pane. Never touches the review pane. */
function openChatThread(root: number): void {
  state.chatThread = root;
  drafts.focus(`chat:${root}`);
  markRead(root);
  render();
}

function closeChatThread(): void {
  state.chatThread = null;
  render();
}

/** Where a thread is anchored, restated: read outside the diff it would
 *  otherwise lose the context that gives it meaning. */
function threadAnchor(root: SessionEvent): string {
  const at = placeOf(root);
  if (at) return `${at.file}:${at.line}${at.status === "outdated" ? " · outdated" : ""}`;
  if (root.chapter) return root.chapter;
  return "The whole change";
}

/** The messages of a thread, and the reply box that continues it. Shared
 *  by the modal and the chat pane so a thread reads the same in both. */
function renderThreadMessages(root: SessionEvent, events: SessionEvent[]): HTMLElement[] {
  const out: (HTMLElement | "")[] = [
    ...events.map((e) =>
      h(
        "div",
        { class: `msg from-${e.from}${e.flag ? " flagged" : ""}` },
        h(
          "div",
          { class: "msg-head" },
          e.flag
            ? h("span", { class: `who flag ${e.flag}`, text: `${flagGlyph(e.flag)} claude · ${FLAG_WORD[e.flag] ?? ""}` })
            : h("span", { class: `who ${e.from === "agent" ? "agent" : e.from === "threshold" ? "threshold" : "human"}`, text: e.from === "agent" ? "claude" : e.from === "threshold" ? "threshold" : "you" }),
          h("span", { class: "msg-time", text: ago(e.at_ms) }),
        ),
        h("div", { class: "msg-text", text: e.text ?? "" }),
      ),
    ),
    events.length === 1 && !root.flag && root.from !== "agent" ? h("div", { class: "note-pending", text: "Claude has not replied yet." }) : "",
  ];
  return out.filter((x): x is HTMLElement => x !== "");
}

function replyComposer(key: string, root: SessionEvent, events: SessionEvent[], placeholder: string, onCancel: () => void): HTMLTextAreaElement {
  return composer(key, placeholder, 3, () => {
    // Anchored to the same place as the root so the follow-up stays with
    // the code it is about, and chained to the last message so the
    // exchange reads in order.
    const parent = events[events.length - 1].seq;
    void send(key, { file: root.file, line: root.line, content: root.line_content, chapter: root.chapter, parent }).then(() => markRead(root.seq));
  }, onCancel);
}

/** A thread, full size, over the review pane.
 *
 *  Covers the review pane only. The chat pane stays visible and usable
 *  beside it, because opening a note in the code is not a reason to lose
 *  your place in a different conversation. */
function renderThreadModal(): HTMLElement | string {
  if (state.openThread == null) return "";
  const events = threadOf(state.openThread);
  const root = events[0];
  if (!root) return "";

  return h(
    "div",
    { class: "modal-backdrop", onClick: (e) => { if (e.target === e.currentTarget) closeThread(); } },
    h(
      "div",
      { class: "modal", role: "dialog", "aria-modal": "true" },
      h(
        "div",
        { class: "modal-head" },
        h(
          "div",
          { class: "modal-anchor" },
          h("div", { class: "modal-where", text: threadAnchor(root) }),
          root.line_content ? h("div", { class: "modal-line", text: root.line_content }) : "",
        ),
        h("button", {
          class: "btn ghost small",
          text: "In chat",
          title: "Continue this thread in the chat pane",
          onClick: () => { const r = root.seq; state.openThread = null; openChatThread(r); },
        }),
        h("button", { class: "btn ghost small", text: "Close", onClick: closeThread }),
      ),
      h("div", { class: "modal-thread", "data-scroll": scrollKey(`modal:${root.seq}`), "data-stick": "bottom" }, ...renderThreadMessages(root, events)),
      h("div", { class: "modal-foot" }, replyComposer(`modal:${root.seq}`, root, events, "Reply. Enter to send, Esc to close.", closeThread)),
    ),
  );
}

function renderComposer(): HTMLElement {
  const at = state.composing!;
  const key = `line:${at.file}#${at.line}`;
  const cancel = () => { state.composing = null; render(); };
  return h(
    "div",
    { class: "composer-row" },
    composer(key, "Ask about this line, or say what should change. Enter to send, Esc to cancel.", 2, () => {
      state.composing = null;
      void send(key, { file: at.file, line: at.line, content: at.content });
    }, cancel),
  );
}

/** How a Markdown file is shown in this review: rendered unless the
 *  reviewer has asked for the diff. */
function mdMode(path: string): "rendered" | "diff" {
  if (!isMarkdown(path)) return "diff";
  return state.place.markdown?.[`${state.activeReview ?? ""}|${path}`] === "diff" ? "diff" : "rendered";
}

function setMdMode(path: string, mode: "rendered" | "diff", draw = true): void {
  state.place.markdown ??= {};
  const key = `${state.activeReview ?? ""}|${path}`;
  if (mode === "rendered") delete state.place.markdown[key];
  else state.place.markdown[key] = "diff";
  void savePlace(state.place);
  if (draw) render();
}

/** The new side of `path`, fetched once per diff. */
async function loadFileText(path: string): Promise<void> {
  const key = contextKey(path);
  if (Array.isArray(state.fileText.get(key)) || state.fileText.get(key) === "loading" || !state.repoPath) return;
  state.fileText.set(key, "loading");
  try {
    state.fileText.set(key, await fileText(state.repoPath, state.staged, state.revision.trim() || null, path));
  } catch (e) {
    state.fileText.set(key, { error: String(e) });
  }
  render();
}

/** Whether a run is shown rendered: a Markdown file with a new side to
 *  render, and the reviewer has not asked for the diff. */
function showsRendered(run: FileRun): boolean {
  return isMarkdown(run.path) && run.file.status !== "deleted" && mdMode(run.path) === "rendered";
}

/** A Markdown file, rendered whole, with the hunks of this run marked in
 *  the margin. The whole file rather than the hunks, since a fragment of
 *  Markdown does not render as the document does. */
function renderMarkdownRun(run: FileRun): HTMLElement {
  const text = state.fileText.get(contextKey(run.path));
  if (!Array.isArray(text)) {
    if (text === undefined) void loadFileText(run.path);
    return h("div", { class: "md-doc muted", text: text && typeof text === "object" ? `Could not read this file: ${text.error}` : "Loading…" });
  }
  const noted = state.events
    .filter((e) => e.kind === "note" && e.in_reply_to == null)
    .map(placeOf)
    .filter((at): at is NonNullable<typeof at> => !!at && at.file === run.path && at.status !== "outdated")
    .map((at) => at.line);
  const html = renderMarkdown(text.join("\n"), changesOf(run.file, run.hunks.map((e) => e.index)), noted);
  const doc = h("div", {
    class: "md-doc",
    onClick: (e) => {
      const target = e.target as HTMLElement;
      // A link would navigate the app itself away from the review.
      if (target.closest("a")) e.preventDefault();
      const noteBtn = target.closest<HTMLElement>(".md-note-btn");
      if (noteBtn) {
        const line = Number(noteBtn.dataset.line);
        const note = state.events.find((x) => x.kind === "note" && x.in_reply_to == null && placeOf(x)?.file === run.path && placeOf(x)?.line === line);
        if (note) jumpToAnchor(note);
      }
    },
  });
  // markdown-it with HTML disabled: every tag here is one it wrote.
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  for (const a of tpl.content.querySelectorAll("a")) a.title = a.getAttribute("href") ?? "";
  // Images the document keeps in the repo, from the review's commit.
  for (const span of tpl.content.querySelectorAll<HTMLElement>("[data-local-src]")) {
    const at = resolveRepoPath(run.path, span.dataset.localSrc!);
    if (!at || !isImage(at)) continue;
    const img = imageAt(at, false);
    if (typeof img === "string" && img !== "loading") span.replaceWith(h("img", { class: "md-img", src: img, alt: span.textContent ?? "", title: at }));
    else if (typeof img === "object") span.title = `${at}: ${img.error}`;
  }
  for (const el of tpl.content.querySelectorAll<HTMLElement>("[data-note-line]")) {
    el.prepend(h("button", { class: "md-note-btn", "data-line": el.dataset.noteLine!, title: "Show the note in the diff", text: "Note ›" }));
  }
  const runKey = `${contextKey(run.path)}|${run.hunks.map((e) => e.index).join(",")}`;
  doc.append(...foldUnchanged([...tpl.content.children] as HTMLElement[], state.mdUnfolded.has(runKey), () => { state.mdUnfolded.add(runKey); render(); }));
  return doc;
}


/** Hunks in the order a chapter or section lists them, each with the gaps
 *  around it, a file band opening each file's run of them.
 *
 *  A hunk standing alone can open the code on either side of itself. When
 *  the next hunk listed is the very next one in the same file, the gap
 *  between them is one gap, drawn once and openable from both edges, as
 *  in the file view — not two bars offering the same lines. */
function renderHunkRefs(refs: HunkRef[]): HTMLElement[] {
  return runsOf(refs, state.files).map((run, k) =>
    h(
      "section",
      { class: "file-run", "data-run": String(k) },
      renderFileBand(run),
      ...(showsRendered(run) ? [renderMarkdownRun(run)] : run.hunks.map((entry, j) => {
        const joinedAbove = run.hunks[j - 1]?.index === entry.index - 1;
        const joinedBelow = run.hunks[j + 1]?.index === entry.index + 1;
        return h(
          "div",
          { class: "hunk-block" },
          ...(joinedAbove ? [] : renderGap(run.file, entry.index, "above")),
          renderHunk(run.file, entry),
          ...renderGap(run.file, entry.index + 1, joinedBelow ? "full" : "below"),
        );
      })),
    ),
  );
}

/** Where one file's run of hunks begins. Sticky, so a long file is never
 *  scrolled into without saying which file it is, and a chapter that
 *  crosses files says so at every crossing rather than in a header line
 *  that reads like code. */
function renderFileBand(run: FileRun): HTMLElement {
  const cut = run.path.lastIndexOf("/");
  const shown = run.hunks.length;
  const total = run.file.hunks.length;
  return h(
    "div",
    { class: "file-band", title: run.path },
    statusBadge(run.file.status),
    h(
      "span",
      { class: "file-band-path" },
      cut >= 0 ? h("span", { class: "file-band-dir", text: run.path.slice(0, cut + 1) }) : "",
      h("span", { class: "file-band-base", text: cut >= 0 ? run.path.slice(cut + 1) : run.path }),
    ),
    h(
      "span",
      { class: "file-band-meta" },
      // Only worth saying when this page shows part of the file: the rest
      // of it is in another chapter.
      ...(run.file.binary
        ? [h("span", { text: "binary" })]
        : [
            shown < total ? h("span", { class: "file-band-part", text: `${shown} of ${total} hunks` }) : h("span", { text: `${total} hunk${total === 1 ? "" : "s"}` }),
            h("span", { class: "file-stat added", text: `+${run.added}` }),
            h("span", { class: "file-stat removed", text: `−${run.removed}` }),
          ]),
      isMarkdown(run.path) && run.file.status !== "deleted"
        ? h(
            "span",
            { class: "toggle-group md-toggle" },
            h("button", { class: `toggle-btn${mdMode(run.path) === "rendered" ? " active" : ""}`, text: "Rendered", onClick: () => setMdMode(run.path, "rendered") }),
            h("button", { class: `toggle-btn${mdMode(run.path) === "diff" ? " active" : ""}`, text: "Diff", onClick: () => setMdMode(run.path, "diff") }),
          )
        : "",
    ),
  );
}

function renderChapterFooter(ch: Chapter, st: ChapterState | undefined): HTMLElement {
  const notes = state.events.filter((e) => e.kind === "note" && e.chapter === ch.title && e.in_reply_to == null);
  const composing = state.composingChapter === ch.title;
  const key = `chapter:${ch.title}`;
  const input = composer(key, `Comment on “${ch.title}”. Enter to send, Esc to cancel.`, 2, () => {
    state.composingChapter = null;
    void send(key, { chapter: ch.title });
  }, () => { state.composingChapter = null; render(); });

  return h(
    "div",
    { class: "chapter-foot" },
    ...notes.map((n) => renderInlineNote(n)),
    composing
      ? h("div", { class: "chapter-composer" }, input)
      : h(
          "div",
          { class: "chapter-actions" },
          h("button", {
            class: `btn small${st?.mark === "ok" ? " marked" : ""}`,
            text: st?.mark === "ok" ? "✓ Looks right" : "Looks right",
            title: "Mark this chapter read. Progress only — it does not approve the change.",
            onClick: () => void setMark(ch.title, st?.mark === "ok" ? "clear" : "ok", st?.mark !== "ok"),
          }),
          h("button", {
            class: `btn small${st?.mark === "concern" ? " flagged" : ""}`,
            text: st?.mark === "concern" ? "! Flagged" : "Flag a concern",
            onClick: () => void setMark(ch.title, st?.mark === "concern" ? "clear" : "concern"),
          }),
          h("button", {
            class: "btn small ghost",
            text: "Comment",
            onClick: () => { state.composingChapter = ch.title; drafts.focus(key); render(); },
          }),
        ),
  );
}

function renderChapter(ch: Chapter, index: number): HTMLElement {
  const hunks = renderHunkRefs(ch.hunks);
  const st = state.chapterStates[index];
  return h(
    "section",
    { class: `chapter${st?.mark ? ` mark-${st.mark}` : ""}`, id: `chapter-${index}` },
    h(
      "div",
      { class: "chapter-head" },
      h("span", { class: "chapter-n", text: String(index + 1) }),
      h(
        "div",
        {},
        h("div", { class: "chapter-title", text: ch.title }),
        ch.summary ? h("div", { class: "chapter-summary", text: ch.summary }) : "",
      ),
    ),
    ...(hunks.length ? hunks : [h("div", { class: "chapter-empty muted", text: "No hunks in this chapter." })]),
    renderChapterFooter(ch, st),
  );
}

/** One chapter, filling the pane.
 *
 *  Every chapter in one scroll made the divisions decorative: you could
 *  not reach the top or the bottom of anything, and a chapter's summary
 *  and its sign-off were separated by however much code lay between. A
 *  chapter is a page now — description at the top, its hunks, its
 *  controls at the end — and moving between them replaces the pane
 *  instead of scrolling through everything. */
function renderNarrative(): HTMLElement {
  const b = state.bundle;
  if (!b) return h("div", { class: "diff-empty muted", text: "Claude has not sent a narrative for this revision." });

  const n = b.chapters.length;
  const showingUnbundled = state.activeChapter >= n;

  const body = showingUnbundled
    ? h(
        "section",
        { class: "chapter unbundled" },
        h(
          "div",
          { class: "chapter-head" },
          h("span", { class: "chapter-n warn", text: "!" }),
          h(
            "div",
            {},
            h("div", { class: "chapter-title", text: `Not in any chapter — ${b.coverage.unbundled.length} hunk${b.coverage.unbundled.length === 1 ? "" : "s"}` }),
            h("div", { class: "chapter-summary", text: "Claude's narrative does not account for these. Read them here, or switch to Files to see the change whole." }),
          ),
        ),
        ...renderHunkRefs(b.coverage.unbundled),
      )
    : renderChapter(b.chapters[state.activeChapter], state.activeChapter);

  return h("div", { class: "diff-view narrative", "data-scroll": readingKey({ view2: "narrative", chapter: state.activeChapter, file: state.selectedFile }) }, body, renderChapterNav());
}

/** Move to the next chapter without going back to the contents list. */
function renderChapterNav(): HTMLElement | string {
  const n = state.bundle?.chapters.length ?? 0;
  const last = n + (state.bundle?.coverage.unbundled.length ? 1 : 0) - 1;
  if (last <= 0) return "";
  const at = state.activeChapter;
  return h(
    "div",
    { class: "chapter-nav" },
    h("button", {
      class: "btn small ghost",
      text: "← Previous",
      disabled: at <= 0,
      onClick: () => jumpToChapter(at - 1),
    }),
    h("span", { class: "chapter-nav-pos muted", text: `${Math.min(at + 1, last + 1)} of ${last + 1}` }),
    h("button", {
      class: "btn small ghost",
      text: "Next →",
      disabled: at >= last,
      onClick: () => jumpToChapter(at + 1),
    }),
  );
}

function renderDiffView(): HTMLElement {
  const file = state.files[state.selectedFile];
  if (!file) return h("div", { class: "diff-empty muted", text: "No files in this diff." });

  // Every gap, then every hunk: the file reads top to bottom, and each gap
  // opens in place between the hunks it separates.
  const run = fileRun(file);
  if (showsRendered(run)) {
    return h(
      "div",
      { class: "diff-view", "data-scroll": readingKey({ view2: "files", chapter: state.activeChapter, file: state.selectedFile }) },
      h("section", { class: "file-run", "data-run": "0" }, renderFileBand(run), renderMarkdownRun(run)),
    );
  }
  const parts: HTMLElement[] = [];
  run.hunks.forEach((entry) => parts.push(...renderGap(file, entry.index, "full"), renderHunk(file, entry)));
  if (file.hunks.length) parts.push(...renderGap(file, file.hunks.length, "full"));
  return h(
    "div",
    { class: "diff-view", "data-scroll": readingKey({ view2: "files", chapter: state.activeChapter, file: state.selectedFile }) },
    h("section", { class: "file-run", "data-run": "0" }, renderFileBand(run), ...(parts.length ? parts : [h("p", { class: "empty muted", text: "No content change (pure rename)." })])),
  );
}

/** Thread roots drawn on a line: wherever their anchors put them. Roots
 *  only — a follow-up is part of its thread's summary, not a second
 *  annotation on the same line. */
function rootNotesAt(path: string, line: number): SessionEvent[] {
  return state.events.filter((e) => {
    if (e.kind !== "note" || e.in_reply_to != null) return false;
    const at = placeOf(e);
    return !!at && at.file === path && at.line === line;
  });
}

/** One line of code with its gutter, annotations and composer. Shared by
 *  hunks and expanded context, so a line behaves identically however it
 *  came to be on screen. */
function renderLine(path: string, kind: "added" | "removed" | "context", content: string, n: number | null, extra = ""): HTMLElement[] {
  const prefix = kind === "added" ? "+" : kind === "removed" ? "-" : " ";
  const notes = n === null ? [] : rootNotesAt(path, n);
  // A flag that has drifted off its line is worse than no flag: it
  // points confidently at code Claude never looked at.
  const flag = notes.find((x) => x.flag && placeOf(x)?.status !== "outdated")?.flag ?? undefined;
  const composing = state.composing?.file === path && state.composing?.line === n;
  const row = h(
    "div",
    {
      class: `diff-line diff-${kind}${extra}${state.session && n !== null ? " annotatable" : ""}${composing ? " composing" : ""}${flag ? ` flagged flag-${flag}` : ""}`,
      // Addressable, so a thread listed in the chat pane can scroll to the
      // line it is about.
      "data-line": n === null ? undefined : `${path}#${n}`,
      title: state.session && n !== null ? "Comment on this line" : undefined,
      onClick: state.session && n !== null
        ? () => { state.composing = { file: path, line: n, content }; drafts.focus(`line:${path}#${n}`); render(); }
        : undefined,
    },
    h(
      "span",
      { class: "diff-gutter" },
      // The mark sits in the gutter so a flagged line is findable while
      // scrolling, not only when it happens to be on screen.
      flag ? h("span", { class: `gutter-flag ${flag}`, text: flagGlyph(flag) }) : "",
      h("span", { class: "gutter-n", text: n === null ? "" : String(n) }),
    ),
    h("span", { class: "diff-text", text: `${prefix}${content}` }),
  );
  const attached = notes.map((note) => renderInlineNote(note, placeOf(note)?.status === "outdated"));
  return [row, ...attached, ...(composing ? [renderComposer()] : [])];
}

/** One hunk with its gutter, annotations and composer. Shared by both
 *  views, so a line behaves identically however it was reached. */
/** An image from the diff's commits, fetched once per diff. */
function imageAt(path: string, old: boolean): string | "loading" | { error: string } {
  const key = `${state.digest}|${old ? "old" : "new"}|${path}`;
  const have = state.images.get(key);
  if (have !== undefined) return have;
  state.images.set(key, "loading");
  if (state.repoPath) {
    void fileImage(state.repoPath, state.staged, state.revision.trim() || null, path, old)
      .then((url) => state.images.set(key, url))
      .catch((e) => state.images.set(key, { error: String(e) }))
      .then(() => render());
  }
  return "loading";
}

/** Whether the webview can show `path` as an image. Matches the list the
 *  backend will serve. */
function isImage(path: string | null): boolean {
  return !!path && /\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$/i.test(path);
}

function renderImage(path: string, old: boolean, label: string): HTMLElement {
  const img = imageAt(path, old);
  return h(
    "figure",
    { class: `bin-side bin-${old ? "old" : "new"}` },
    h("figcaption", { text: label }),
    typeof img === "string" && img !== "loading"
      ? h("img", { src: img, alt: path })
      : h("div", { class: "bin-placeholder muted", text: img === "loading" ? "Loading…" : img.error }),
  );
}

/** A binary file's one hunk: the image itself, both sides when it
 *  changed, or a line saying there is nothing to read. */
function renderBinary(file: DiffFile, entry: HunkEntry): HTMLElement {
  const sides: HTMLElement[] = [];
  if (isImage(file.old_path ?? file.new_path)) {
    if (file.status !== "added" && file.old_path) sides.push(renderImage(file.old_path, true, file.status === "deleted" ? "Deleted" : "Before"));
    if (file.status !== "deleted" && file.new_path) sides.push(renderImage(file.new_path, false, file.status === "added" ? "Added" : "After"));
  }
  return h(
    "div",
    { class: "hunk binary-hunk", "data-hunk": entry.key },
    sides.length
      ? h("div", { class: "bin-sides" }, ...sides)
      : h("div", { class: "bin-placeholder muted", text: "Binary file, not shown." }),
  );
}

function renderHunk(file: DiffFile, entry: HunkEntry): HTMLElement {
  if (file.binary) return renderBinary(file, entry);
  const path = pathOf(file);
  const hunk = file.hunks[entry.index];
  // The new-file line number, tracked across the hunk so an annotation
  // refers to a line the agent can actually find. Removed lines have no
  // new-file number and are not annotatable.
  let lineNo = spanOf(hunk.header).start;
  return h(
    "div",
    // Addressable, so the navigator can jump to it and say which one is
    // on screen.
    { class: "hunk", "data-hunk": entry.key },
    // Sticks under the file band. The file is the band's to name; this
    // says where in the file, and which hunk of how many.
    h(
      "div",
      { class: "hunk-header", title: hunk.header },
      h("span", { class: "hunk-lines", text: entry.lines }),
      h("span", { class: "hunk-context", text: entry.context }),
      h(
        "span",
        { class: "hunk-meta" },
        h("span", { class: "file-stat added", text: `+${entry.added}` }),
        h("span", { class: "file-stat removed", text: `−${entry.removed}` }),
        h("span", { class: "hunk-pos", text: `${entry.index + 1}/${file.hunks.length}` }),
      ),
    ),
    ...hunk.lines.flatMap((line) => renderLine(path, line.kind, line.content, line.kind === "removed" ? null : lineNo++)),
  );
}

function contextKey(path: string): string {
  return `${state.digest}|${path}`;
}

function gapsOf(file: DiffFile): Gap[] {
  const text = state.fileText.get(contextKey(file.new_path ?? ""));
  return gapsAround(file.hunks.map((hk) => spanOf(hk.header)), Array.isArray(text) ? text.length : null);
}

/** Open more of a gap, fetching the file the first time. Lines appear
 *  below the bar that was clicked, so nothing above it moves and the
 *  reader's place holds without any scroll correction. */
async function expandGap(file: DiffFile, index: number, edge: "top" | "bottom" | "all", n = STEP): Promise<void> {
  const path = file.new_path;
  if (!path || !state.repoPath) return;
  const key = contextKey(path);
  if (!Array.isArray(state.fileText.get(key))) {
    state.fileText.set(key, "loading");
    render();
    try {
      state.fileText.set(key, await fileText(state.repoPath, state.staged, state.revision.trim() || null, path));
    } catch (e) {
      state.fileText.set(key, { error: String(e) });
      render();
      return;
    }
  }
  const gap = gapsOf(file)[index];
  if (!gap) return;
  const per = (state.opened[key] ??= {});
  per[index] = openGap(gap, per[index] ?? { top: 0, bottom: 0 }, edge, n);
  render();
}

/** The unchanged code in gap `index` of `file`, and the bar that opens it.
 *
 *  `full` is the file view: the whole gap, opened from either edge.
 *  `above` and `below` are the narrative, where a hunk stands alone in its
 *  chapter: only the edge touching that hunk is offered, because the code
 *  at the far edge belongs to a hunk that may be in another chapter. */
function renderGap(file: DiffFile, index: number, mode: "full" | "above" | "below"): HTMLElement[] {
  const path = file.new_path;
  // A deleted file has no new side to show, and an added one is already
  // shown whole.
  // A binary file has no lines, so no gaps between them.
  if (!path || file.status === "deleted" || file.status === "added" || file.binary) return [];
  const gaps = gapsOf(file);
  const gap = gaps[index];
  if (!gap || sizeOf(gap) === 0) return [];
  const key = contextKey(path);
  const text = state.fileText.get(key);
  const lines = Array.isArray(text) ? text : null;
  const opened = state.opened[key]?.[index] ?? { top: 0, bottom: 0 };
  const lay = layout(gap, opened);
  const isFirst = index === 0;
  const isLast = index === gaps.length - 1;

  const rows = (range: [number, number] | null): HTMLElement[] => {
    if (!range || !lines) return [];
    const out: HTMLElement[] = [];
    for (let n = range[0]; n <= range[1] && n <= lines.length; n++) out.push(...renderLine(path, "context", lines[n - 1], n, " expanded"));
    return out;
  };

  const top = mode === "above" ? [] : rows(lay.top);
  const bottom = mode === "below" ? [] : rows(lay.bottom);
  // Lines no edge has opened. Not "lines this view is not showing": in the
  // narrative, lines opened from the far edge sit beside the other hunk,
  // and offering to open them again is a button that does nothing.
  const remaining = lay.hidden;

  const canTop = mode !== "above" && !isFirst;
  const canBottom = mode !== "below" && !isLast;
  // "All" in the narrative opens from the edge this hunk touches, so the
  // lines land beside it rather than beside a hunk in another chapter.
  const allEdge = mode === "full" ? "all" : mode === "above" ? "bottom" : "top";
  const button = (text: string, title: string, edge: "top" | "bottom" | "all", n = STEP) =>
    h("button", { class: "gap-btn", text, title, onClick: (e) => { e.stopPropagation(); void expandGap(file, index, edge, n); } });

  let bar: HTMLElement | null = null;
  if (remaining == null || remaining > 0) {
    const label = remaining == null ? "more lines" : `${remaining} unchanged line${remaining === 1 ? "" : "s"}`;
    const buttons: HTMLElement[] = [];
    if (text === "loading") buttons.push(h("span", { class: "gap-note", text: "Loading…" }));
    else if (text && !Array.isArray(text)) buttons.push(h("span", { class: "gap-note", text: text.error }));
    else if (remaining != null && remaining <= STEP) buttons.push(button(`Show ${label}`, "Show them all", allEdge, remaining));
    else {
      if (canTop) buttons.push(button(`↓ ${STEP} more`, "Show the lines below the code above", "top"));
      if (canBottom) buttons.push(button(`↑ ${STEP} more`, "Show the lines above the code below", "bottom"));
      if (remaining != null) buttons.push(button(`Show all ${remaining}`, "Show the whole gap", allEdge, remaining));
      else buttons.push(h("span", { class: "gap-note", text: "to the end of the file" }));
    }
    bar = h("div", { class: "gap-bar", "data-gap": `${path}#${index}` }, h("span", { class: "gap-gutter", text: "⋯" }), ...buttons);
  }
  return [h("div", { class: "gap" }, ...top, bar, ...bottom)];
}

/** Switch between the agent's reading of the diff and the plain one.
 *
 *  Files is the ground truth and is always one click away — the narrative
 *  is a claim about the change, and a reviewer must never be more than a
 *  moment from the unarguable version. */
function ago(ms: number): string {
  if (!ms) return "";
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}

/** The inbox: every review on disk, waiting ones first.
 *
 *  Built from the directory alone, so a review outlives the agent that
 *  wrote it — a crashed or closed session leaves a review you can still
 *  open, read in full, and decide. Or archive, when it does not need a
 *  decision at all. How it is arranged is inbox.ts. */
function renderInbox(): HTMLElement {
  const view = state.inboxView;
  const grouping = state.place.inbox_group === "project" ? "project" : "date";
  const projects = projectsIn(state.reviews, view);
  // A remembered filter for a project with nothing in this view would show
  // an empty list with no reason why; it falls back to every project.
  const project = state.place.inbox_project != null && projects.some((p) => p.repo === state.place.inbox_project) ? state.place.inbox_project : null;
  const sections = arrange(state.reviews, view, grouping, project);
  const archivedCount = state.reviews.filter((r) => r.archived).length;

  const archive = async (r: ReviewSummary, archived: boolean) => {
    try {
      state.reviews = await setArchived(r.path, archived);
    } catch (e) {
      state.error = String(e);
    }
    render();
    // Unarchived, it is waiting again, and sized like the rest.
    if (!archived) void refreshSizes();
  };

  // The row is a container, not a button: opening the review and archiving
  // it are two actions, and a button inside a button is not a thing.
  const row = (r: ReviewSummary) =>
    h(
      "div",
      { class: `review-row${state.activeReview === r.path ? " active" : ""}${r.decided ? " decided" : ""}` },
      h(
        "button",
        { class: "review-open", title: r.path, onClick: () => void selectReview(r) },
        h("div", { class: "review-title", text: r.title }),
        h(
          "div",
          { class: "review-meta" },
          // The project is the section heading when grouped by it.
          r.repo && grouping !== "project" ? h("span", { class: "review-repo", text: projectName(r.repo) }) : "",
          r.revision ? h("span", { class: "review-rev", text: r.revision }) : "",
          h("span", { class: "review-age", text: ago(r.last_at_ms) }),
        ),
        // Only for what is waiting: the size is for budgeting the read.
        !r.decided && !r.archived && state.sizes[r.path]
          ? h(
              "div",
              { class: "review-size", title: `${state.sizes[r.path].files} file${state.sizes[r.path].files === 1 ? "" : "s"} changed` },
              ...sizeParts(state.sizes[r.path].added, state.sizes[r.path].removed),
            )
          : "",
      ),
      h("button", {
        class: "review-archive",
        text: r.archived ? "Unarchive" : "Archive",
        title: r.archived
          ? "Back to the inbox"
          : "Put away without approving or rejecting. It comes back if Claude says anything more.",
        onClick: () => void archive(r, !r.archived),
      }),
    );

  const groups: (HTMLElement | string)[] = sections.flatMap((sec) => [
    h(
      "div",
      { class: "inbox-group" },
      h("span", { text: grouping === "project" ? `${sec.title} · ${sec.reviews.length}` : sec.title }),
      sec.detail ? h("span", { class: "inbox-group-detail", text: sec.detail }) : "",
    ),
    ...sec.reviews.map(row),
  ]);
  if (!sections.length) {
    groups.push(
      view === "archived"
        ? h("div", { class: "inbox-empty" }, h("p", { text: project ? "Nothing archived in this project." : "Nothing archived." }))
        : state.reviews.length
          ? h("div", { class: "inbox-empty" }, h("p", { text: "Nothing in the inbox." }), archivedCount ? h("p", { class: "muted", text: `${archivedCount} archived.` }) : "")
          : h("div", { class: "inbox-empty" },
              h("p", { text: "No reviews yet." }),
              h("p", { class: "muted", text: "Claude puts them in ~/.threshold/reviews. Leave Threshold open and they show up here." })),
    );
  }

  const setPlace = (patch: Partial<typeof state.place>) => {
    Object.assign(state.place, patch);
    void savePlace(state.place);
    render();
  };

  return h(
    "aside",
    { class: "inbox" },
    h(
      "div",
      { class: "inbox-head" },
      h(
        "div",
        { class: "toggle-group inbox-views" },
        h("button", { class: `toggle-btn${view === "inbox" ? " active" : ""}`, text: `Inbox${waitingCount() ? ` ${waitingCount()}` : ""}`, onClick: () => { state.inboxView = "inbox"; render(); } }),
        h("button", { class: `toggle-btn${view === "archived" ? " active" : ""}`, text: `Archived${archivedCount ? ` ${archivedCount}` : ""}`, onClick: () => { state.inboxView = "archived"; render(); } }),
      ),
      h("button", { class: "btn ghost small", text: "↻", title: "Refresh", onClick: () => void refreshReviews() }),
    ),
    h(
      "div",
      { class: "inbox-tools" },
      h(
        "select",
        {
          class: "field inbox-project",
          title: "Show one project",
          onChange: (e) => { const v = (e.target as HTMLSelectElement).value; setPlace({ inbox_project: v === "*" ? null : v }); },
        },
        h("option", { value: "*", text: "All projects", selected: project == null }),
        ...projects.map((p) => h("option", { value: p.repo, text: `${p.name} (${p.count})`, selected: project === p.repo, title: p.repo || undefined })),
      ),
      h(
        "div",
        { class: "toggle-group" },
        h("button", { class: `toggle-btn${grouping === "date" ? " active" : ""}`, text: "Date", title: "Waiting, then decided, newest first", onClick: () => setPlace({ inbox_group: "date" }) }),
        h("button", { class: `toggle-btn${grouping === "project" ? " active" : ""}`, text: "Project", title: "One section per project, the most recently active first", onClick: () => setPlace({ inbox_group: "project" }) }),
      ),
    ),
    h("div", { class: "inbox-list", "data-scroll": `|inbox:${view}` }, ...groups),
  );
}

/** Back and forward, then the view toggle and the narrative's coverage.
 *
 *  Back is how a jump stops being a commitment: follow a thread to its
 *  line in chapter one, then return to the exact spot in chapter four. */
function renderModeBar(): HTMLElement | string {
  if (!state.session) return "";
  const cov = state.bundle?.coverage;
  const complete = !!cov && cov.unbundled.length === 0 && cov.phantom.length === 0;
  return h(
    "div",
    { class: "modebar" },
    h(
      "div",
      { class: "nav-group" },
      h("button", { class: "btn ghost small nav-btn", text: "←", title: "Back (⌘[)", disabled: !nav.back.length, onClick: navBack }),
      h("button", { class: "btn ghost small nav-btn", text: "→", title: "Forward (⌘])", disabled: !nav.forward.length, onClick: navForward }),
    ),
    state.bundle
      ? h(
          "div",
          { class: "toggle-group" },
          h("button", { class: `toggle-btn${state.view2 === "narrative" ? " active" : ""}`, text: "Narrative", onClick: () => setView("narrative") }),
          h("button", { class: `toggle-btn${state.view2 === "files" ? " active" : ""}`, text: "Files", onClick: () => setView("files") }),
        )
      : "",
    cov ? h("span", { class: `coverage${complete ? "" : " warn"}`, text: `${cov.claimed} of ${cov.total} hunks in a chapter` }) : "",
    cov?.phantom.length
      ? h("span", { class: "coverage warn", text: `· ${cov.phantom.length} chapter reference${cov.phantom.length === 1 ? "" : "s"} not in this diff` })
      : "",
  );
}

function renderBrief(): HTMLElement | string {
  const brief = state.events.find((e) => e.kind === "brief");
  if (!brief) return "";
  return h(
    "div",
    { class: "brief" },
    h("div", { class: "brief-title", text: brief.title ?? "" }),
    h("div", { class: "brief-summary", text: brief.summary ?? "" }),
    ...(brief.implications ?? []).map((im) => h("div", { class: "brief-implication", text: im })),
  );
}

/** The table of contents: the shape of the review, and where you are in
 *  it. Doubles as progress, which a long narrative otherwise has none of. */
function renderToc(): HTMLElement | string {
  const chapters = state.bundle?.chapters ?? [];
  if (!chapters.length) return "";
  const read = state.chapterStates.filter((c) => c.mark === "ok").length;
  const concerns = state.chapterStates.filter((c) => c.mark === "concern").length;

  return h(
    "nav",
    { class: "toc" },
    h(
      "div",
      { class: "toc-head" },
      h("span", { class: "toc-progress", text: `${read} of ${chapters.length} read` }),
      h("span", { class: "toc-keys", text: "[ ]" , title: "Step between chapters" }),
    ),
    h(
      "div",
      { class: "toc-list", "data-scroll": scrollKey("toc-list") },
      ...chapters.flatMap((ch, i) => {
        const st = state.chapterStates[i];
        const runs = runsOf(ch.hunks, state.files);
        const active = state.activeChapter === i;
        const row = h(
          "button",
          {
            class: `toc-row${active ? " active" : ""}${st?.mark ? ` mark-${st.mark}` : ""}`,
            // The chapter already open: back to its top.
            onClick: () => (active ? jumpWithin(".chapter-head") : jumpToChapter(i)),
          },
          // The number never goes away. It is how you navigate, and
          // losing it the moment a chapter is dealt with is exactly
          // backwards — the mark rides alongside instead.
          h("span", { class: "toc-n", text: String(i + 1) }),
          h(
            "span",
            { class: "toc-body" },
            h("span", { class: "toc-label", text: ch.title }),
            h(
              "span",
              { class: "toc-meta" },
              h("span", { text: `${ch.hunks.length} hunk${ch.hunks.length === 1 ? "" : "s"}` }),
              h("span", { class: "file-stat added", text: `+${runs.reduce((n, r) => n + r.added, 0)}` }),
              h("span", { class: "file-stat removed", text: `−${runs.reduce((n, r) => n + r.removed, 0)}` }),
              st?.unanswered ? h("span", { class: "toc-open", text: `${st.unanswered} open` }) : "",
              st?.caution ? h("span", { class: "toc-flag caution", text: `${flagGlyph("caution")}${st.caution}` }) : "",
              st?.attention ? h("span", { class: "toc-flag attention", text: `${flagGlyph("attention")}${st.attention}` }) : "",
            ),
          ),
          st?.mark ? h("span", { class: `toc-mark ${st.mark}`, text: st.mark === "ok" ? "✓" : "!" }) : "",
        );
        // Only the chapter being read opens out: every chapter's files and
        // hunks at once would bury the chapters, which are the way through.
        return [row, active ? renderOutline(runs, true) : ""];
      }),
      state.bundle && state.bundle.coverage.unbundled.length
        ? [
            h(
              "button",
              {
                class: `toc-row unbundled${state.activeChapter === chapters.length ? " active" : ""}`,
                onClick: () => (state.activeChapter === chapters.length ? jumpWithin(".chapter-head") : jumpToChapter(chapters.length)),
              },
              h("span", { class: "toc-n warn", text: "!" }),
              h(
                "span",
                { class: "toc-body" },
                h("span", { class: "toc-label", text: "Not in any chapter" }),
                h("span", { class: "toc-meta" }, h("span", { text: `${state.bundle.coverage.unbundled.length} hunks` })),
              ),
            ),
            state.activeChapter === chapters.length ? renderOutline(runsOf(state.bundle.coverage.unbundled, state.files), true) : "",
          ]
        : "",
    ),
    concerns
      ? h("div", { class: "toc-concerns", text: `${concerns} flagged` })
      : "",
  );
}

/** Threshold's own findings about what the agent pushed.
 *
 *  Rendered apart from both voices: this is the tool reporting a fact,
 *  not a participant in the conversation. It sits above the thread
 *  because it usually explains something odd the reviewer is already
 *  looking at. */
/** What is on screen is no longer what is on disk.
 *
 *  Sits directly above the diff rather than in the rail with the other
 *  diagnostics, because it is not a remark about the review — it is a
 *  statement that the thing being read is out of date, and a reviewer
 *  who scrolls past it may approve a change that no longer exists.
 *
 *  Reloading is offered, never done: the diff moving is not a reason to
 *  throw away where they had read to. */
function renderDrift(): HTMLElement | string {
  if (!state.drifted) return "";
  return h(
    "div",
    { class: "drift" },
    h("span", { class: "drift-mark", text: "⚠" }),
    h("span", { class: "drift-text", text: "The code changed since this diff was loaded. What you are reading is no longer what is on disk." }),
    h("button", {
      class: "btn small",
      text: "Reload",
      onClick: () => { if (state.repoPath) void loadFor(state.repoPath); },
    }),
  );
}

/** One of Threshold's findings, where it happened in the channel. Folded
 *  to its message: the detail is for the agent, and a list of forty hunk
 *  headers would push every conversation off the pane. */
function renderDiagnostic(d: SessionEvent): HTMLElement {
  const detail = d.detail ?? [];
  // Kept where it happened, but marked: a finding about a bundle or
  // revision the agent has since replaced is history, not a warning.
  const retired = isRetired(state.events, d);
  return h(
    "details",
    { class: `diagnostic chat-diagnostic${retired ? " retired" : ""}` },
    h(
      "summary",
      {},
      h("span", { class: "diag-code", text: d.code ?? "" }),
      h("span", { class: "diag-message", text: d.message ?? "" }),
      retired ? h("span", { class: "diag-retired", text: "fixed since" }) : "",
    ),
    ...detail.slice(0, 20).map((line) => h("div", { class: "diag-detail", text: line })),
    detail.length > 20 ? h("div", { class: "diag-detail", text: `… and ${detail.length - 20} more` }) : "",
  );
}

/** Notes not tied to a line, plus revisions, in order. */
/** Which chapter holds a file, by the hunks it covers. */
function chapterOfFile(file: string): number {
  return (state.bundle?.chapters ?? []).findIndex((c) => c.hunks.some((hk) => hk.file === file));
}

/** Take the review pane to where a thread is anchored.
 *
 *  Only ever on an explicit click — "Show in review" — and recorded in
 *  the review pane's history first, so Back returns the reader to exactly
 *  where they were. */
function jumpToAnchor(note: SessionEvent): void {
  const before = nav.back.length;
  if (note.chapter) {
    const i = (state.bundle?.chapters ?? []).findIndex((c) => c.title === note.chapter);
    if (i >= 0) jumpToChapter(i);
    return;
  }
  const at = placeOf(note);
  if (!at) return;
  // A note sits on a source line, which only the diff has.
  if (mdMode(at.file) === "rendered") setMdMode(at.file, "diff");
  const i = chapterOfFile(at.file);
  if (state.view2 === "narrative" && i >= 0) jumpToChapter(i);
  else {
    const f = state.files.findIndex((x) => fileKey(x) === at.file);
    if (f >= 0) selectFile(f);
  }
  // Already on the right page: scrolling to the line is still a move
  // worth being able to undo.
  if (nav.back.length === before) pushNav();
  // render() is synchronous, so the row already exists. Instant rather
  // than smooth: an animation still running when the next render lands is
  // cut off halfway, and the reader is left at neither place.
  const el = document.querySelector<HTMLElement>(`[data-line="${CSS.escape(`${at.file}#${at.line}`)}"]`);
  if (!el) return;
  el.scrollIntoView({ block: "center" });
  const pane = el.closest<HTMLElement>("[data-scroll]");
  if (pane) scroll.record(pane);
  el.classList.add("just-jumped");
  setTimeout(() => el.classList.remove("just-jumped"), 1600);
}

/** How a thread's anchor reads in the list. */
function anchorLabel(note: SessionEvent): string {
  const at = placeOf(note);
  if (at) return `${at.file.split("/").pop()}:${at.line}${at.status === "outdated" ? " (outdated)" : ""}`;
  if (note.chapter) return note.chapter;
  return "whole change";
}

function anchorKind(note: SessionEvent): "line" | "chapter" | "global" {
  return note.file ? "line" : note.chapter ? "chapter" : "global";
}

/** Waiting on Claude: the last word in the thread is the reviewer's. */
function waitingOnClaude(root: number): boolean {
  const t = threadOf(root);
  const last = t[t.length - 1];
  return !!last && last.from === "human";
}

/** A thread is a conversation once someone has spoken in it. A flag
 *  nobody has answered is a pointer, and is listed apart. */
function isConversation(root: SessionEvent): boolean {
  return !root.flag || threadOf(root.seq).length > 1;
}

/** The chat pane: every conversation about this review, in one channel.
 *
 *  Deliberately a second place with its own position, not a view of the
 *  review pane. Opening a thread here, replying, or following a
 *  notification never moves the diff; the only way from here to the code
 *  is "Show in review", which the review pane's Back undoes.
 *
 *  Threads keep the position their first message gave them, as in a chat
 *  channel. Re-sorting by activity would move the row under the pointer
 *  every time Claude answered anything. What is new is marked instead. */
function renderChat(): HTMLElement {
  return state.chatThread != null && threadOf(state.chatThread)[0] ? renderChatThread(state.chatThread) : renderChannel();
}

function renderChannel(): HTMLElement {
  const roots = state.events.filter((e) => e.kind === "note" && e.in_reply_to == null);
  const conversations = roots.filter(isConversation);
  const pointers = roots.filter((r) => !isConversation(r));
  const unreadRoots = conversations.filter((r) => unreadIn(r.seq) > 0);
  const waiting = conversations.filter((r) => waitingOnClaude(r.seq));

  const card = (e: SessionEvent) => {
    const rest = threadOf(e.seq).slice(1);
    const latest = rest[rest.length - 1];
    const unread = unreadIn(e.seq);
    const kind = anchorKind(e);
    return h(
      "div",
      { class: `chat-card${unread ? " unread" : ""}${waitingOnClaude(e.seq) ? " waiting" : ""}`, "data-thread": String(e.seq) },
      h(
        "div",
        { class: "chat-card-top" },
        h("button", {
          class: `anchor-chip ${kind}`,
          text: kind === "line" ? `↳ ${anchorLabel(e)}` : kind === "chapter" ? `§ ${anchorLabel(e)}` : "◎ whole change",
          title: kind === "global" ? "About the change as a whole" : "Show in review",
          disabled: kind === "global",
          onClick: () => jumpToAnchor(e),
        }),
        unread ? h("span", { class: "unread-badge", text: `${unread} new` }) : "",
        h("span", { class: "msg-time", text: ago((latest ?? e).at_ms) }),
      ),
      h(
        "button",
        { class: "chat-card-body", title: "Open this thread", onClick: () => openChatThread(e.seq) },
        h(
          "div",
          { class: "thread-line" },
          e.flag
            ? h("span", { class: `who flag ${e.flag}`, text: flagGlyph(e.flag) })
            : h("span", { class: `who ${e.from === "agent" ? "agent" : "human"}`, text: e.from === "agent" ? "claude" : "you" }),
          h("span", { class: "chat-root-text", text: e.text ?? "" }),
        ),
        latest
          ? h(
              "div",
              { class: "thread-line latest" },
              h("span", { class: `who ${latest.from === "agent" ? "agent" : "human"}`, text: latest.from === "agent" ? "claude" : "you" }),
              h("span", { class: "note-latest", text: latest.text ?? "" }),
            )
          : "",
        h(
          "div",
          { class: "chat-card-foot" },
          rest.length ? h("span", { text: `${rest.length} repl${rest.length === 1 ? "y" : "ies"}` }) : "",
          waitingOnClaude(e.seq) ? h("span", { class: "note-pending", text: "waiting for Claude…" }) : "",
        ),
      ),
    );
  };

  // The channel in log order: the brief opens it, revisions and
  // Threshold's findings sit where they happened, and every thread where
  // it began.
  const items: HTMLElement[] = [];
  let pointersShown = false;
  for (const e of state.events) {
    if (e.kind === "brief") {
      items.push(h("div", { class: "chat-brief" }, renderBrief()));
    } else if (e.kind === "revised") {
      items.push(h("div", { class: "chat-divider" }, h("span", { text: `Claude revised → ${e.to ?? ""}${e.note ? ` · ${e.note}` : ""}` })));
    } else if (e.kind === "diagnostic") {
      items.push(renderDiagnostic(e));
    } else if (e.kind === "note" && e.in_reply_to == null) {
      if (isConversation(e)) items.push(card(e));
      else if (!pointersShown && pointers.length) {
        pointersShown = true;
        items.push(renderPointers(pointers));
      }
    }
  }

  const nextUnread = () => {
    const first = unreadRoots[0];
    if (first) openChatThread(first.seq);
  };

  return h(
    "div",
    { class: "chat" },
    h(
      "div",
      { class: "chat-head" },
      h("span", { class: "thread-title", text: "Conversation" }),
      h(
        "span",
        { class: "chat-head-meta" },
        waiting.length ? h("span", { class: "thread-count waiting", text: `${waiting.length} waiting on Claude` }) : "",
        unreadRoots.length ? h("button", { class: "unread-badge button", text: `${unreadRoots.length} unread ↓`, title: "Open the first thread with new replies", onClick: nextUnread }) : "",
      ),
    ),
    h(
      "div",
      { class: "chat-scroll", "data-scroll": scrollKey("channel"), "data-stick": "bottom" },
      ...items,
      conversations.length ? "" : h("div", { class: "thread-empty", text: "No conversations yet. Click a line or a chapter's Comment to start one, or write below about the change as a whole." }),
    ),
    h(
      "div",
      { class: "chat-foot" },
      composer("global", "Message Claude about the whole change…", 2, () => void send("global"), () => (document.activeElement as HTMLElement | null)?.blur()),
    ),
  );
}

/** Claude's flags that nobody has answered, folded into one row. They are
 *  pointers into the code, and forty of them as cards would bury every
 *  actual conversation. Answering one turns it into a card. */
function renderPointers(pointers: SessionEvent[]): HTMLElement {
  return h(
    "details",
    { class: "chat-pointers" },
    h("summary", { text: `Claude flagged ${pointers.length} line${pointers.length === 1 ? "" : "s"}` }),
    ...pointers.map((p) =>
      h(
        "div",
        { class: "pointer-row" },
        h("button", { class: `anchor-chip line`, text: `${flagGlyph(p.flag ?? "context")} ${anchorLabel(p)}`, title: "Show in review", onClick: () => jumpToAnchor(p) }),
        h("button", { class: "pointer-text", text: p.text ?? "", title: "Open this thread", onClick: () => openChatThread(p.seq) }),
      ),
    ),
  );
}

function renderChatThread(rootSeq: number): HTMLElement {
  const events = threadOf(rootSeq);
  const root = events[0];
  const kind = anchorKind(root);
  // Seeing it is reading it, including a reply that arrives while it is open.
  markRead(rootSeq);
  return h(
    "div",
    { class: "chat" },
    h(
      "div",
      { class: "chat-head thread" },
      h("button", { class: "btn ghost small", text: "← All", title: "Back to every conversation (Esc in the reply box)", onClick: closeChatThread }),
      h("span", { class: "chat-thread-where", text: threadAnchor(root) }),
      kind === "global" ? "" : h("button", { class: "btn ghost small", text: "Show in review", title: "Take the review pane there. Back (⌘[) returns.", onClick: () => jumpToAnchor(root) }),
    ),
    h(
      "div",
      { class: "chat-scroll", "data-scroll": scrollKey(`chat:${rootSeq}`), "data-stick": "bottom" },
      root.line_content ? h("pre", { class: "chat-code", text: root.line_content }) : "",
      ...renderThreadMessages(root, events),
    ),
    h("div", { class: "chat-foot" }, replyComposer(`chat:${rootSeq}`, root, events, "Reply. Enter to send, Esc for all threads.", closeChatThread)),
  );
}

function renderActionBar(): HTMLElement {
  if (state.decided) {
    return h("div", { class: `decided decided-${state.decided}`, text: state.decided === "approved" ? "Approved" : "Rejected" });
  }
  return h(
    "div",
    { class: "action-bar" },
    h("input", { class: "field", placeholder: "Reason — required to reject", value: state.reason, onInput: (e) => { state.reason = (e.target as HTMLInputElement).value; } }),
    h(
      "div",
      { class: "action-row" },
      h("button", { class: "btn danger", text: "Reject", disabled: state.submitting, onClick: () => { if (state.reason.trim()) void submitDecision("rejected"); else { state.error = "A rejection needs a reason."; render(); } } }),
      h("button", { class: "btn primary", text: "Approve", disabled: state.submitting, onClick: () => void submitDecision("approved") }),
    ),
  );
}

function renderHistory(): HTMLElement {
  const rows = state.decisions
    .slice()
    .reverse()
    .map((d) =>
      h(
        "div",
        { class: "history-row" },
        h("span", { class: `verdict-badge verdict-${d.verdict}`, text: d.verdict }),
        h("span", { class: "history-time muted", text: formatTime(d.decided_at_ms) }),
        h("span", { class: "history-stats muted", text: `${d.summary.files_changed} files, +${d.summary.lines_added}/-${d.summary.lines_removed}` }),
        // Absent on entries logged before revisions were supported.
        d.source ? h("span", { class: "history-source", text: d.source === "--cached" ? "staged" : d.source }) : "",
        d.reason ? h("span", { class: "history-reason", text: d.reason }) : "",
      ),
    );
  return h(
    "div",
    { class: "history" },
    h("button", { class: "btn ghost small back-btn", text: "← Back to diff", onClick: () => { state.view = "diff"; render(); } }),
    ...(rows.length ? rows : [h("p", { class: "empty muted", text: "No decisions recorded yet." })]),
  );
}

/** The size of the diff on screen, beside what it is. */
function crumbSize(): HTMLElement | string {
  const s = state.summary;
  if (!s || state.loading) return "";
  return h(
    "span",
    { class: "crumb-size", title: `${s.files_changed} file${s.files_changed === 1 ? "" : "s"} changed` },
    ...sizeParts(s.lines_added, s.lines_removed),
  );
}

/** What a new user sees: how a review gets here, and the tour. */
function renderGettingStarted(): HTMLElement {
  const install = async (replace: boolean) => {
    state.skillBusy = true;
    state.error = null;
    render();
    try {
      state.skill = await installSkill(replace);
    } catch (e) {
      state.error = String(e);
    } finally {
      state.skillBusy = false;
      render();
    }
  };
  const skill =
    state.skill === "installed"
      ? h("p", { class: "gs-done", text: "✓ Installed in ~/.claude/skills/threshold-review" })
      : state.skill === "different"
        ? h(
            "div",
            {},
            h("p", { class: "gs-warn", text: "A different copy is already in ~/.claude/skills/threshold-review." }),
            h("button", { class: "btn small", text: state.skillBusy ? "Replacing…" : "Replace it with this version", disabled: state.skillBusy, onClick: () => void install(true) }),
          )
        : h("button", { class: "btn small", text: state.skillBusy ? "Installing…" : "Install for Claude Code", disabled: state.skillBusy, onClick: () => void install(false) });
  return h(
    "div",
    { class: "empty-state" },
    h(
      "div",
      { class: "getting-started" },
      h("h2", { text: "Welcome to Threshold" }),
      h("p", { class: "gs-lede", text: "Your coding agent writes a review here when it has a change for you: a brief, the diff in chapters, and flags on the lines that matter. You read it, ask about any line, and approve or reject." }),
      h(
        "ol",
        { class: "gs-steps" },
        h(
          "li",
          {},
          h("b", { text: "Give your agent the skill" }),
          h("span", { class: "gs-m", text: "Claude Code picks it up from ~/.claude/skills." }),
          skill,
          h("span", { class: "gs-alt" }, "Another agent, or rather do it yourself? ", h("button", { class: "link", text: "Show the skill file", onClick: () => void revealSkill().catch((e) => { state.error = String(e); render(); }) })),
        ),
        h("li", {}, h("b", { text: "Ask for a review" }), h("span", { class: "gs-m", text: "Say “review this in Threshold” after a change, or ask it to walk you through a colleague's PR." })),
        h("li", {}, h("b", { text: "Leave Threshold open" }), h("span", { class: "gs-m", text: "Reviews show up in the list on the left, with a notification." })),
      ),
      h("div", { class: "gs-tour" }, h("button", { class: "btn primary", text: "Take the tour", onClick: () => void startTour() }), h("span", { class: "gs-m", text: "Two minutes, on a sample review." })),
      h("p", { class: "gs-aside" }, "Just want to read a diff yourself? ", h("button", { class: "link", text: "File ▸ Open Folder… (⌘O)", onClick: () => void openFolder() })),
    ),
  );
}

/** The tour, over a real review of the sample repo. */
async function startTour(): Promise<void> {
  if (tourRunning()) return;
  let path: string;
  try {
    path = await startSample();
  } catch (e) {
    state.error = `The sample review could not be set up: ${e}`;
    render();
    return;
  }
  await refreshReviews();
  state.view = "diff";
  const open = async () => {
    if (state.activeReview !== path) {
      const r = state.reviews.find((x) => x.path === path);
      if (r) await selectReview(r);
    }
    if (state.bundle) state.view2 = "narrative";
  };
  const toChapter = async (i: number) => {
    await open();
    state.activeChapter = i;
    render();
  };
  // The chapter holding the line flagged `flag`.
  const chapterOf = (flag: string) => {
    const note = state.events.find((e) => e.kind === "note" && e.flag === flag);
    return Math.max(0, state.bundle?.chapters.findIndex((c) => c.hunks.some((x) => x.file === note?.file)) ?? 0);
  };
  // The chapter holding the sample's answered thread.
  const threadChapter = () => {
    const note = state.events.find((e) => e.kind === "note" && e.from === "human" && e.file);
    return Math.max(0, state.bundle?.chapters.findIndex((c) => c.hunks.some((x) => x.file === note?.file)) ?? 0);
  };
  const q = (sel: string) => () => document.querySelector(sel);
  const steps: TourStep[] = [
    {
      title: "A review arrives",
      text: "This is what your agent sends you when it has a change: it shows up here, with a notification. This one is a sample, so try anything.",
      before: () => { state.showInbox = true; render(); },
      target: () => document.querySelector(`.review-open[title="${CSS.escape(path)}"]`)?.closest(".review-row") ?? null,
    },
    {
      title: "The brief",
      text: "Before any code: what the change is, what it implies, and what the agent is unsure about.",
      before: () => toChapter(0),
      target: q(".brief"),
    },
    {
      title: "Chapters, not files",
      text: "The agent cut the diff into chapters, in the order that explains it, instead of git's alphabetical order. Click one, or press ] for the next.",
      before: () => toChapter(0),
      target: q(".toc"),
    },
    {
      title: "Flags on the lines that matter",
      text: "Out of every changed line, the agent marks the few that could bite. This one can't be undone.",
      before: () => toChapter(chapterOf("caution")),
      target: q(".diff-line.flag-caution"),
    },
    {
      title: "Ask about any line",
      text: "Click any line to ask about it, and your agent answers in a thread like this one. It can revise the change while you read, too.",
      before: () => toChapter(threadChapter()),
      target: () => {
        const asked = state.events.find((e) => e.kind === "note" && e.from === "human" && e.file)?.text ?? "";
        return [...document.querySelectorAll(".inline-note")].find((n) => !!asked && n.textContent?.includes(asked)) ?? document.querySelector(".diff-line.annotatable");
      },
    },
    {
      title: "Every hunk accounted for",
      text: "Threshold checks that every hunk is in a chapter. If the agent leaves something out, this turns red and the missing hunks get a section of their own.",
      target: q(".coverage"),
    },
    {
      title: "Your verdict",
      text: "Approve, or reject with a reason. The verdict names the exact diff you read, so the agent can tell if the code has moved since.",
      target: q(".action-bar"),
    },
  ];
  runTour(steps, async () => {
    // Done or skipped, the sample leaves the inbox: it was never waiting.
    state.reviews = await setArchived(path, true).catch(() => state.reviews);
    render();
  });
}

function render(): void {
  const header = h(
    "header",
    { class: "bar" },
    h("div", { class: "brand" }, h("span", { class: "brand-mark" }), h("span", { class: "brand-name", text: "Threshold" })),
    state.activeReview
      ? h(
          "div",
          { class: "crumb" },
          h("span", { class: "crumb-title", text: currentReviewTitle() }),
          state.repoPath ? h("span", { class: "crumb-repo", text: state.repoPath.split("/").pop() ?? "" }) : "",
          crumbSize(),
        )
      : state.repoPath
        ? h("div", { class: "crumb" }, h("div", { class: "repo-path muted", text: state.repoPath }), crumbSize())
        : "",
    // Working tree, staged and the revision field are for reading a repo
    // yourself (File ▸ Open Folder…). Inside an agent's review they would
    // only swap out the diff the review is about.
    state.repoPath && !state.activeReview
      ? h(
          "div",
          { class: "toggle-group" },
          h("button", { class: `toggle-btn${!state.staged && !state.revision ? " active" : ""}`, text: "Working tree", onClick: () => void toggleStaged(false) }),
          h("button", { class: `toggle-btn${state.staged && !state.revision ? " active" : ""}`, text: "Staged", onClick: () => void toggleStaged(true) }),
        )
      : "",
    state.repoPath && !state.activeReview
      ? h("input", {
          class: `field rev-field${state.revision ? " active" : ""}`,
          placeholder: "revision, e.g. main...HEAD",
          title: "Diff a revision instead of the working tree — a branch, a range, or a commit",
          value: state.revision,
          // Applied on Enter or blur rather than on every keystroke: a
          // half-typed spec is a git error, and running one per character
          // would fill the error banner while the user is still typing.
          onKeyDown: (e) => {
            if ((e as KeyboardEvent).key === "Enter") void applyRevision((e.target as HTMLInputElement).value.trim());
          },
          onBlur: (e) => void applyRevision((e.target as HTMLInputElement).value.trim()),
        })
      : "",
    state.repoPath ? h("button", { class: "btn ghost small", text: "↻", title: "Refresh", onClick: refresh }) : "",
    // The way back. Collapsing the inbox on entering a review reclaimed
    // the width the diff needed, but left no visible exit — so this reads
    // as a back control while inside a review, not just a toggle.
    h("button", {
      class: `btn ghost small${state.showInbox ? " on" : ""}`,
      text: `${state.activeReview && !state.showInbox ? "← " : ""}Reviews${waitingCount() ? ` ${waitingCount()}` : ""}`,
      title: state.activeReview && !state.showInbox ? "Back to all reviews (Esc)" : "Show or hide the review inbox",
      onClick: () => { state.showInbox = !state.showInbox; render(); },
    }),
    // Only when there is one. Everything comes back where it was, so the
    // restart costs a second, and not taking it leaves an old build running.
    state.update
      ? h("button", {
          class: "btn small update-btn",
          text: "Update ready · Restart",
          title: `${state.update.commit} ${state.update.subject}\nRestarts Threshold into the new build. Everything comes back where it was.`,
          onClick: () => void restartForUpdate().catch((e) => { state.error = String(e); render(); }),
        })
      : "",
    h("button", { class: "btn ghost small", text: "History", onClick: openHistory }),
    // Named, when phone access is on, so the name the phone will show is
    // on the desk too.
    h(
      "button",
      { class: `btn ghost small phone-btn${state.view === "phone" ? " on" : ""}`, title: "Reach this Mac's reviews from your phone", onClick: () => void openPhone() },
      state.phone?.machine && state.phone.enabled ? h("span", { class: "phone-dot", style: `background: ${state.phone.machine.color}` }) : "",
      state.phone?.machine && state.phone.enabled ? `Phone · ${state.phone.machine.name}` : "Phone",
    ),
  );

  let body: HTMLElement;
  if (state.view === "history") {
    body = renderHistory();
  } else if (state.view === "phone") {
    body = renderPhone();
  } else if (!state.repoPath) {
    // An empty screen is a place to say what to do next. Which thing to
    // say depends on whether anything is actually waiting.
    // Until a real review has ever arrived, that is how to get one.
    const waiting = waitingCount();
    body = !waiting && !state.reviews.some((r) => !isSample(r.path))
      ? renderGettingStarted()
      : h(
          "div",
          { class: "empty-state muted" },
          waiting
            ? h("div", {}, h("p", { text: `${waiting} review${waiting === 1 ? "" : "s"} waiting on you.` }), h("p", { class: "muted", text: "Pick one on the left." }))
            : h("div", {}, h("p", { text: "Nothing waiting." }), h("p", { class: "muted", text: "Your agent puts reviews in ~/.threshold/reviews." })),
        );
  } else if (!state.summary || state.files.length === 0) {
    const what = state.revision ? `changes in ${state.revision}` : state.staged ? "staged changes" : "working-tree changes";
    body = h("div", { class: "empty-state muted", text: state.loading ? "Loading…" : `No ${what}.` });
  } else {
    body = h(
      "div",
      { class: "shell" },
      // Evidence left, argument right. The rail carries the agent's case,
      // the conversation, and — pinned at its foot — the facts and
      // the decision, so those two are read together.
      //
      // Without a session there is no argument and no conversation, so the
      // rail would be 340px of empty column. It is dropped entirely and
      // the same two things become a footer under the diff, which is the
      // shape this screen had before an agent was ever involved.
      state.session
        ? h(
            "div",
            { class: "layout" },
            h(
              "div",
              { class: "main-col" },
              // The navigator swaps with the view: files have a file
              // list, the narrative has a table of contents. Same slot,
              // same job — know where you are, jump somewhere else.
              state.view2 === "narrative" ? renderToc() : renderFileList(),
              h("div", { class: "reading" }, renderDrift(), renderModeBar(), state.view2 === "narrative" ? renderNarrative() : renderDiffView()),
              // Inside the review pane, so it covers the code and not the
              // conversation beside it.
              renderThreadModal(),
            ),
            h(
              "div",
              { class: "rail" },
              renderChat(),
              renderFacts(state.summary),
              renderActionBar(),
            ),
          )
        : h(
            "div",
            { class: "solo" },
            h("div", { class: "main-col" }, renderFileList(), h("div", { class: "reading" }, renderDrift(), renderDiffView())),
            h("div", { class: "footer" }, renderFacts(state.summary), renderActionBar()),
          ),
    );
  }

  // Replacing a textarea mid-composition cancels it and drops what was
  // being entered, so a render waits for the input method to finish.
  if (drafts.composing) {
    renderWaiting = true;
    return;
  }
  renderWaiting = false;
  // The only place the DOM is replaced, so the only place anything has to
  // be carried across it: scroll by key (scroll.ts) and unsent text with
  // its cursor (drafts.ts).
  const regions = () => document.querySelectorAll<HTMLElement>("[data-scroll]");
  const fields = () => document.querySelectorAll<HTMLTextAreaElement>("[data-draft]");
  scroll.capture(regions());
  const active = document.activeElement as HTMLTextAreaElement | null;
  drafts.capture(fields(), active?.dataset?.draft ? active : null);
  app.replaceChildren(
    header,
    state.error ? h("div", { class: "error", text: state.error }) : "",
    // The inbox sits outside the review, because it outlives any one of
    // them and must be reachable whatever is on screen.
    h("div", { class: "workspace" }, state.showInbox ? renderInbox() : "", h("div", { class: "workspace-main" }, body)),
    renderToasts(),
  );
  scroll.restore(regions());
  drafts.restore(fields());
  spy();
  persistUi();
}

async function init(): Promise<void> {
  // The title bar follows the theme's light or dark appearance, not
  // macOS's, so a Light window on a dark Mac does not wear a dark frame.
  const themeChosen = initTheme((t) => {
    try { void getCurrentWindow().setTheme(isDark(t) ? "dark" : "light").catch(() => {}); } catch { /* no native window */ }
  });
  await listen("open-settings", () => openPicker(false));
  await listen("open-folder", () => void openFolder());
  await listen("start-tour", () => void startTour());
  window.addEventListener("threshold:tour", () => void startTour());
  state.skill = await skillStatus().catch(() => null);
  render();
  // The window starts hidden so it never shows before the theme is on.
  try { void getCurrentWindow().show().catch(() => {}); } catch { /* no native window */ }
  state.phone = await phoneStatus().catch(() => null);
  // Asked now and every half minute: a build lands after a commit, and
  // the app is meant to be left open.
  const checkUpdate = async () => {
    const u = await updateReady().catch(() => null);
    if ((u?.commit ?? null) !== (state.update?.commit ?? null)) { state.update = u; render(); }
  };
  void checkUpdate();
  setInterval(() => void checkUpdate(), 30_000);
  state.place = await loadPlace();
  state.place.read ??= {};
  await initNotifications();
  // Asked after the plugin's own prompt; the answer can arrive a moment
  // later, so it is asked again the first time it matters.
  state.native = await nativeNotifications().catch(() => false);
  await refreshReviews();
  state.waitingSeen = waitingCount();
  // Per-review event counts, so an update to a review you are not
  // reading can be told apart from one you are. Only what the agent and
  // the reviewer wrote: Threshold checks reviews in the background, and
  // its pin and diagnostics are for the agent, not news for you.
  state.seenEvents = new Map(state.reviews.map((r) => [r.path, r.spoken]));

  // Both listeners are registered unconditionally, before anything else.
  // They used to be set up only when the app was launched with --session,
  // so a review opened from the inbox never received updates at all — the
  // view just sat there while the agent wrote to the file.
  await listen<number>("inbox-changed", () => {
    void (async () => {
      const seen = state.seenEvents;
      const knew = new Set(seen.keys());
      await refreshReviews();

      for (const r of state.reviews) {
        const before = seen.get(r.path);
        if (!knew.has(r.path)) {
          // Brand new, provided it is not simply the first time we have
          // looked: an empty log gets a title once its brief lands.
          if (r.spoken > 0) toast("New review", r.title, { review: r.path, thread: null });
        } else if (before != null && r.spoken > before && r.path !== state.activeReview) {
          // Updated while you were elsewhere. The open review is covered
          // by session-changed, which can say what actually arrived.
          toast("Review updated", r.title, { review: r.path, thread: null });
        }
        seen.set(r.path, r.spoken);
      }
      for (const p of knew) if (!state.reviews.some((r) => r.path === p)) seen.delete(p);

      state.waitingSeen = waitingCount();
    })();
  });

  // Only an unpinned review can drift, and only it emits this.
  await listen<string>("diff-drifted", (e) => {
    // Compared against what is actually on screen rather than trusted
    // outright: the backend reports every change it sees, including ones
    // this window has already reloaded past.
    if (e.payload !== state.digest) {
      state.drifted = true;
      render();
    }
  });

  // A notification clicked, native or in-app.
  await listen<NotificationTarget>("notification-open", (e) => void openTarget(e.payload));

  await listen<number>("session-changed", () => {
    void (async () => {
      const lastSeen = state.events.reduce((m, e) => Math.max(m, e.seq), 0);
      const review = state.activeReview;
      await refreshEvents();
      const fresh = state.events.filter((e) => e.seq > lastSeen);
      if (!fresh.length) return;
      if (!state.native) state.native = await nativeNotifications().catch(() => false);

      // One notification per thread Claude spoke in, saying what it said
      // and where, so the banner alone is enough to decide whether to look.
      // Deliberately not diagnostics: those are Threshold talking to the
      // agent, and notifying on everything makes an app worth muting.
      const spoken = fresh.filter((e) => e.from === "agent" && (e.kind === "reply" || (e.kind === "note" && !e.flag)));
      const byThread = new Map<number, SessionEvent>();
      for (const e of spoken) byThread.set(rootOf(e), e);
      const looking = document.hasFocus() ? visibleThreads() : new Set<number>();
      const threads = [...byThread].filter(([root]) => !looking.has(root));
      for (const [root, e] of threads.slice(0, 3)) {
        const r = state.events.find((x) => x.seq === root);
        toast("Claude replied", (e.text ?? "").slice(0, 240), { review, thread: root }, r ? threadAnchor(r) : null);
      }
      if (threads.length > 3) {
        toast(`Claude replied in ${threads.length - 3} more threads`, "Open the conversation to read them.", { review, thread: null });
      }

      const revised = fresh.find((e) => e.kind === "revised");
      if (revised) toast("Claude revised the change", revised.note ?? revised.to ?? "", { review, thread: null });
      if (revised?.to) {
        state.revision = revised.to;
        // Reloaded under the reader, not over them: same file, same view,
        // same chapter, and each pane's scroll kept where the page still
        // exists.
        if (state.repoPath) await loadFor(state.repoPath, true);
      } else if (fresh.some((e) => e.kind === "bundle" && e.from === "agent") && state.repoPath) {
        // A corrected narrative at the same revision. Without a reload the
        // reader keeps the old bundle's chapters and coverage, so a fixed
        // bundle still reads as incomplete.
        await loadFor(state.repoPath, true);
      }
    })();
  });

  const [argRepo, revision, session] = await Promise.all([initialRepo(), initialRevision(), hasSession()]);
  state.session = session;
  if (session) {
    state.events = await sessionEvents();
    // A review launched with --session is as open as one picked from the
    // list; without this it had no title in the header and no way back.
    state.activeReview = await activeReview();
  }

  // Revision, like repo, belongs to the review rather than to argv. The
  // backend already derives it; the frontend was still reading the
  // command line and so diffed the working tree instead of the revision
  // the review is actually about.
  const fromLog = [...state.events].reverse().find((e) => e.revision)?.revision ?? null;
  const chosen = revision ?? fromLog;
  if (chosen) state.revision = chosen;

  // A review's repo belongs to the review. Falling back to argv left a
  // directly launched review with nothing to diff, so it opened on the
  // inbox as though no review had been named at all.
  const repo = argRepo ?? (session ? await reviewRepo() : null);
  // Launched straight into a review: same reasoning as selecting one.
  if (session) state.showInbox = false;

  await reopen(session, repo);
  // Then the screen and panes as they were left. After the review, since
  // selecting one tucks the list away.
  const ui = state.place.ui ?? {};
  if (!session && ui.show_inbox != null) state.showInbox = ui.show_inbox;
  state.deletedOpen = !!ui.deleted_open;
  restored = true;
  if (ui.view === "history") await openHistory();
  else if (ui.view === "phone") await openPhone();
  else render();
  // First open: ask, over the app as it now looks.
  if (!themeChosen) openPicker(true, (tour) => { if (tour) void startTour(); });
}

/** Put back whatever was being read: the review named at launch, else a
 *  folder opened by hand if that was last, else the last review. */
async function reopen(session: boolean, repo: string | null): Promise<void> {
  const folder = state.place.ui?.folder;
  if (!session && !repo && folder?.repo) {
    state.staged = folder.staged;
    state.revision = folder.revision;
    await loadFor(folder.repo);
    return;
  }
  if (!session && state.place.review) {
    const last = state.reviews.find((r) => r.path === state.place.review);
    if (last) { await selectReview(last); return; }
  }
  if (state.activeReview) {
    scroll.load(`${state.activeReview}|`, state.place.scroll[state.activeReview]);
    state.activeChapter = state.place.chapter[state.activeReview] ?? 0;
    const m = state.place.mode[state.activeReview];
    if (m === "files" || m === "narrative") state.view2 = m;
  }
  if (repo) await loadFor(repo);
}

let spyFrame = 0;

// Scrolling is continuous; writing on every pixel is not. Persist once
// things have settled instead.
let scrollTimer: number | undefined;
document.addEventListener(
  "scroll",
  (e) => {
    const el = e.target as HTMLElement | null;
    if (!el?.dataset?.scroll) return;
    // Remembered at once, so a render a moment later restores this and
    // not the position before it; persisted once scrolling settles.
    scroll.record(el);
    clearTimeout(scrollTimer);
    scrollTimer = window.setTimeout(() => rememberPlace(), 400);
    // Once a frame at most, however many scroll events arrive in it.
    if (el.classList.contains("diff-view") && !spyFrame) {
      spyFrame = requestAnimationFrame(() => { spyFrame = 0; spy(); });
    }
  },
  true,
);

// Chapter navigation from the keyboard: a review is a sequential read,
// and reaching for the mouse between every chapter is friction.
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && state.openThread != null) {
    e.preventDefault();
    closeThread();
    return;
  }
  // Back and forward in the review pane, as in a browser or an editor.
  if ((e.metaKey || e.ctrlKey) && (e.key === "[" || e.key === "]")) {
    e.preventDefault();
    if (e.key === "[") navBack();
    else navForward();
    return;
  }
  const inField = (e.target as HTMLElement | null)?.tagName?.match(/INPUT|TEXTAREA/);
  if (inField) return;
  if (e.key === "Escape" && state.activeReview && !state.showInbox) {
    e.preventDefault();
    state.showInbox = true;
    render();
    return;
  }
  if (e.key === "]") { e.preventDefault(); stepChapter(1); }
  if (!state.bundle) return;
  if (e.key === "[") { e.preventDefault(); stepChapter(-1); }
});

// The mouse's own back and forward buttons, over the review pane only: the
// chat pane has no history for them to undo.
window.addEventListener("mouseup", (e) => {
  if (e.button !== 3 && e.button !== 4) return;
  if (!(e.target as HTMLElement | null)?.closest?.(".main-col")) return;
  e.preventDefault();
  if (e.button === 3) navBack();
  else navForward();
});

void init();
