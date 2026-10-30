// Threshold on the phone: triage, ask, mark and reject; approve at the desk.
//
// Built around the narrative, which already maps onto a phone: a brief,
// then one page per chapter, swiped between. Every screen carries the
// band naming the Mac it is driving, in that Mac's colour, because the
// same phone drives several and must never leave that in doubt.

import "./style.css";
import * as api from "./client";
import type { ApiError, ChapterRow, DiffPayload, FreshReview, Machine, ReviewRow, SessionEvent, ThreadCard } from "./client";
import type { Chapter, DiffFile, LineAnchor, Mark } from "../src/api";
import { describeHunk, pathOf, runsOf, fileRun, type FileRun } from "../src/outline";
import { spanOf } from "../src/context";
import { h } from "../src/util";
import { changesOf, isMarkdown, renderMarkdown, resolveRepoPath } from "../src/markdown";
import { foldUnchanged } from "../src/mdfold";
import { threadOf } from "../src/threads";
import { currentDiagnostics } from "../src/diagnostics";

type Loaded<T> = T | "loading" | { error: string };

/** Files and images fetched for the open review, by review, path and side.
 *  Cleared when the review reloads, since a revision changes them. */
const texts = new Map<string, Loaded<string[]>>();
const images = new Map<string, Loaded<string>>();
/** Rendered documents unfolded in full, by review and file. */
const unfolded = new Set<string>();

function load<T>(cache: Map<string, Loaded<T>>, key: string, get: () => Promise<T>): Loaded<T> {
  const have = cache.get(key);
  if (have !== undefined) return have;
  cache.set(key, "loading");
  void get()
    .then((v) => cache.set(key, v))
    .catch((e) => cache.set(key, { error: isApiError(e) ? e.message : String(e) }))
    .then(() => render());
  return "loading";
}

function isImage(path: string | null | undefined): boolean {
  return !!path && /\.(png|jpe?g|gif|webp|avif|bmp|ico|svg)$/i.test(path);
}

/** How a Markdown file is shown: rendered unless this phone asked for the
 *  diff. Remembered per review and file on this phone. */
function mdMode(review: string, path: string): "rendered" | "diff" {
  try { return localStorage.getItem(`threshold.md:${review}|${path}`) === "diff" ? "diff" : "rendered"; } catch { return "rendered"; }
}
function setMdMode(review: string, path: string, mode: "rendered" | "diff"): void {
  try {
    if (mode === "diff") localStorage.setItem(`threshold.md:${review}|${path}`, "diff");
    else localStorage.removeItem(`threshold.md:${review}|${path}`);
  } catch { /* private mode: not remembered */ }
  render();
}

/** A message as Claude wrote it: Markdown, rendered. Raw HTML is escaped
 *  and links do not navigate, as for Markdown files. */
function messageBody(text: string): HTMLElement {
  const tpl = document.createElement("template");
  tpl.innerHTML = renderMarkdown(text, { changed: new Set(), removedBefore: new Map() });
  for (const a of tpl.content.querySelectorAll("a")) a.addEventListener("click", (e) => e.preventDefault());
  const div = h("div", { class: "msg-md" });
  div.append(tpl.content);
  return div;
}

/** A message on one line: its words, without the Markdown around them. */
function plain(text: string): string {
  const tpl = document.createElement("template");
  tpl.innerHTML = renderMarkdown(text, { changed: new Set(), removedBefore: new Map() });
  return (tpl.content.textContent ?? "").replace(/\s+/g, " ").trim();
}

/** One swipeable page of a review. */
type Page =
  | { kind: "brief" }
  | { kind: "chapter"; index: number; chapter: Chapter; runs: FileRun[] }
  | { kind: "unbundled"; runs: FileRun[] }
  | { kind: "file"; run: FileRun };

interface Review {
  name: string;
  events: SessionEvent[];
  diff: DiffPayload | null;
  diffError: string | null;
  chapters: ChapterRow[];
  anchors: Map<number, LineAnchor>;
  pages: Page[];
  page: number;
  /** The revision the diff on screen was loaded for. */
  revision: string | null;
  /** Claude revised the review while it was open; cleared by reloading. */
  revised: boolean;
}

type Sheet =
  | { kind: "note"; title: string; at: api.NoteAt }
  | { kind: "reject" }
  | { kind: "approve" }
  | { kind: "thread"; root: number };

const state = {
  machine: api.savedMachine() as Machine | null,
  /** Whether the Mac is answering. Starts hopeful; the first failed call
   *  corrects it. */
  live: true,
  screen: "threads" as "threads" | "inbox" | "review" | "pair" | "unpaired" | "moved",
  /** Where "back" from a review goes: the tab it was opened from. */
  home: "threads" as "threads" | "inbox",
  threads: [] as ThreadCard[],
  fresh: [] as FreshReview[],
  threadsLoaded: false,
  /** Collapsed sections of the threads screen that are open. */
  showWaiting: false,
  showSeen: false,
  /** Threads whose earlier messages are shown, by review#root. */
  expanded: new Set<string>(),
  /** Thread cards opened from their one-line summary, by review#root. */
  open: new Set<string>(),
  /** The one project the threads screen is narrowed to, if any. */
  project: null as string | null,
  /** The Mac this page was paired with, when this address now answers as
   *  another one. */
  was: null as Machine | null,
  pairToken: null as string | null,
  reviews: [] as ReviewRow[],
  /** Whether `reviews` came from the Mac. Until it has, an empty list
   *  would claim "nothing waiting" about a Mac nobody has asked. */
  loaded: false,
  review: null as Review | null,
  tab: "read" as "read" | "threads",
  sheet: null as Sheet | null,
  /** A message that needs reading, shown until dismissed. */
  notice: null as string | null,
  busy: false,
  /** Whether this phone gets a buzz when Claude is waiting. null until
   *  asked; "unsupported" where the browser has no push. */
  push: null as null | "on" | "off" | "unsupported" | "blocked",
};

const root = document.getElementById("app")!;

// --- Identity ------------------------------------------------------------

/** Paint this Mac's colour into the browser chrome as well as the band. */
function applyMachine(m: Machine | null): void {
  const color = m?.color ?? "#555";
  document.documentElement.style.setProperty("--machine", color);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", color);
  document.title = m ? `Threshold · ${m.name}` : "Threshold";
}

/** The band. On every screen, without exception: the name of the Mac
 *  this page drives, in its colour, and whether it is answering. */
function band(): HTMLElement {
  const m = state.machine;
  return h(
    "header",
    { class: `band${state.live ? "" : " down"}` },
    h("span", { class: "band-name", text: m ? m.name : "not paired" }),
    h("span", { class: "band-app", text: "Threshold" }),
    state.live ? "" : h("span", { class: "band-state", text: "not answering" }),
  );
}

// --- Errors ----------------------------------------------------------------

function isApiError(e: unknown): e is ApiError {
  return e instanceof api.ApiError;
}

/** Route a failed call to the screen that explains it. Returns whether it
 *  was handled as something other than an ordinary message. */
function handle(e: unknown): void {
  if (isApiError(e)) {
    if (e.unreachable) { state.live = false; render(); return; }
    if (e.status === 401) { api.forget(); state.screen = "unpaired"; render(); return; }
    state.notice = e.message;
  } else {
    state.notice = String(e);
  }
  render();
}

// --- Loading ---------------------------------------------------------------

/** `live`: fetched because something changed elsewhere, so drawn only if
 *  it differs, and not under someone mid-sentence. */
async function loadInbox(live = false): Promise<void> {
  try {
    const reviews = await api.reviews();
    const same = state.loaded && JSON.stringify(reviews) === JSON.stringify(state.reviews);
    state.reviews = reviews;
    state.loaded = true;
    if (live && same && state.live) return;
    state.live = true;
  } catch (e) { handle(e); }
  if (live) renderLive(); else render();
}

function currentRevision(events: SessionEvent[]): string | null {
  let rev: string | null = null;
  for (const e of events) {
    if (e.kind === "brief" && rev === null) rev = e.revision;
    if (e.kind === "revised" && e.to) rev = e.to;
  }
  return rev ?? events.find((e) => e.revision)?.revision ?? null;
}

function pagesFor(diff: DiffPayload | null, bundle: api.BundlePayload | null): Page[] {
  const pages: Page[] = [{ kind: "brief" }];
  if (!diff) return pages;
  if (bundle && bundle.chapters.length) {
    bundle.chapters.forEach((chapter, index) => pages.push({ kind: "chapter", index, chapter, runs: runsOf(chapter.hunks, diff.files) }));
    // Never hidden: what the narrative leaves out is exactly what a
    // reviewer skimming the narrative would miss.
    if (bundle.coverage.unbundled.length) pages.push({ kind: "unbundled", runs: runsOf(bundle.coverage.unbundled, diff.files) });
  } else {
    diff.files.forEach((f) => pages.push({ kind: "file", run: fileRun(f) }));
  }
  return pages;
}

interface OpenAt {
  /** Stay on the page and tab already open: a reload of the same review. */
  keep?: boolean;
  /** The page to land on. */
  page?: number;
  /** Land on the page holding this thread, when no page is given. */
  thread?: number;
  /** Land on this place in the diff, when no page is given. */
  at?: { file: string | null; line: number | null; chapter: string | null };
}

let loading = 0;

async function openReview(name: string, opts: OpenAt = {}): Promise<void> {
  const token = ++loading;
  /** Still wanted: not left, and not overtaken by a later load. */
  const wanted = () => token === loading && state.screen === "review" && state.review?.name === name;
  const prev = state.review?.name === name ? state.review : null;
  texts.clear(); images.clear();
  state.screen = "review";
  if (!prev) {
    state.review = { name, events: [], diff: null, diffError: null, chapters: [], anchors: new Map(), pages: [{ kind: "brief" }], page: 0, revision: null, revised: false };
    render();
  }
  try {
    const events = await api.events(name);
    const [diff, bundle, chapters, anchors] = await Promise.all([
      api.diff(name).catch((e) => (isApiError(e) && !e.unreachable ? e.message : Promise.reject(e))),
      api.bundle(name),
      api.chapters(name),
      api.anchors(name),
    ]);
    const d = typeof diff === "string" ? null : diff;
    // Left while it loaded: Back, or another review opened. Drawing it now
    // would pull the reader back into a screen they had left.
    if (!wanted()) return;
    state.review = {
      name,
      events,
      diff: d,
      diffError: typeof diff === "string" ? diff : null,
      chapters,
      anchors: new Map(anchors.map((a) => [a.seq, a])),
      pages: pagesFor(d, bundle),
      page: 0,
      revision: currentRevision(events),
      revised: false,
    };
    const r = state.review;
    const root = opts.thread !== undefined ? r.events.find((e) => e.seq === opts.thread) : undefined;
    const at = opts.at ?? (root ? { ...(whereIs(r, root) ?? { file: null, line: null }), chapter: root.chapter ?? null } : null);
    const page = opts.page ?? (opts.keep && prev ? prev.page : at ? pageAt(r, at) : 0);
    r.page = Math.max(0, Math.min(page, r.pages.length - 1));
    state.live = true;
  } catch (e) { handle(e); }
  if (!wanted()) { render(); return; }
  render();
  restorePage();
  record();
}

/** The page showing a place in the diff: the page whose hunk holds the
 *  line, since a file can be split across chapters; else the first page
 *  with the file at all; else the chapter; else the brief. */
function pageAt(r: Review, at: { file: string | null; line: number | null; chapter: string | null }): number {
  const runsIn = (p: Page) => (p.kind === "chapter" || p.kind === "unbundled" ? p.runs : p.kind === "file" ? [p.run] : []);
  let page = -1;
  if (at.file) {
    page = r.pages.findIndex((p) =>
      runsIn(p).some((run) => run.path === at.file && run.hunks.some((e) => {
        const s = spanOf(run.file.hunks[e.index]?.header ?? "");
        return at.line != null && at.line >= s.start && at.line < s.start + Math.max(1, s.count);
      })));
    if (page < 0) page = r.pages.findIndex((p) => runsIn(p).some((run) => run.path === at.file));
  } else if (at.chapter) {
    page = r.pages.findIndex((p) => p.kind === "chapter" && p.chapter.title === at.chapter);
  }
  return Math.max(0, page);
}

/** Events only: a reply or a mark does not move the diff. */
async function refreshEvents(live = false): Promise<void> {
  const r = state.review;
  if (!r) return;
  try {
    const [events, chapters, anchors] = await Promise.all([api.events(r.name), api.chapters(r.name), api.anchors(r.name)]);
    if (state.review !== r) return;
    const same =
      events.length === r.events.length &&
      events[events.length - 1]?.seq === r.events[r.events.length - 1]?.seq &&
      JSON.stringify(chapters) === JSON.stringify(r.chapters) &&
      JSON.stringify(anchors) === JSON.stringify([...r.anchors.values()]);
    r.events = events;
    r.chapters = chapters;
    r.anchors = new Map(anchors.map((a) => [a.seq, a]));
    // A revision moves the diff under the reader. Said with a banner and
    // reloaded on request, rather than yanked away mid-sentence.
    if (currentRevision(events) !== r.revision) r.revised = true;
    if (live && same && state.live) return;
    state.live = true;
  } catch (e) { handle(e); }
  if (live) renderLive(); else render();
}

// --- Threads -----------------------------------------------------------------

function roots(events: SessionEvent[]): SessionEvent[] {
  return events.filter((e) => e.kind === "note" && e.in_reply_to == null);
}

/** Waiting on the reviewer: a root or last message from Claude that the
 *  reviewer has not answered. */
function unanswered(events: SessionEvent[]): number {
  return roots(events).filter((r) => {
    const t = threadOf(events, r.seq);
    return t[t.length - 1].from === "agent";
  }).length;
}

function whereIs(r: Review, e: SessionEvent): { file: string; line: number; outdated: boolean } | null {
  const a = r.anchors.get(e.seq);
  if (a) return { file: a.file, line: a.line, outdated: a.status === "outdated" || !a.in_diff };
  return e.file && e.line ? { file: e.file, line: e.line, outdated: false } : null;
}

// --- Actions ---------------------------------------------------------------

async function submitSheet(text: string): Promise<void> {
  const r = state.review;
  const s = state.sheet;
  if (!r || !s || state.busy) return;
  state.busy = true;
  const draft = draftKey();
  /** Still on the sheet this was sent from: not closed, or replaced by
   *  another review, while it was sending. */
  const here = () => state.sheet === s && state.review === r;
  try {
    if (s.kind === "reject") {
      if (!r.diff) throw new Error("The diff has not loaded, so there is nothing to reject yet.");
      r.events = await api.reject(r.name, text, r.diff.digest);
    } else if (s.kind === "approve") {
      if (!r.diff) throw new Error("The diff has not loaded, so there is nothing to approve yet.");
      r.events = await api.approve(r.name, text || null, r.diff.digest);
    } else if (s.kind === "thread") {
      r.events = await api.note(r.name, text, { in_reply_to: s.root });
    } else {
      r.events = await api.note(r.name, text, s.at);
    }
    forgetDraft(draft);
    // A message stays open as the conversation it is now part of, so
    // Claude's answer arrives where the reader is looking. A verdict ends
    // the review, and its sheet with it.
    if (here() && s.kind === "note") {
      const mine = r.events.filter((e) => e.kind === "note" && e.from === "human" && e.in_reply_to == null && e.text === text).pop();
      state.sheet = mine ? { kind: "thread", root: mine.seq } : null;
      record();
    } else if (here() && (s.kind === "reject" || s.kind === "approve")) {
      // A verdict: the sheet's step in history is over.
      if (hs().sheet) history.back();
      state.sheet = null;
    }
    if (state.review === r) await refreshEvents();
  } catch (e) {
    // The text stays in the box: a failed send must not cost what was
    // written.
    handle(e);
    if (isApiError(e) && e.status === 409 && (s.kind === "reject" || s.kind === "approve") && here()) {
      if (hs().sheet) history.back();
      state.sheet = null;
      await openReview(r.name, { keep: true });
    }
  } finally {
    state.busy = false;
    render();
  }
}

async function setMark(chapter: string, m: Mark): Promise<void> {
  const r = state.review;
  if (!r) return;
  navigator.vibrate?.(15);
  try {
    r.events = await api.mark(r.name, chapter, m);
    r.chapters = await api.chapters(r.name);
  } catch (e) { handle(e); }
  render();
}

// --- Drafts ------------------------------------------------------------------

function draftKey(): string {
  const s = state.sheet;
  const r = state.review;
  if (!s || !r) return "";
  const where = s.kind === "note" ? JSON.stringify(s.at) : s.kind === "thread" ? String(s.root) : "reject";
  return `threshold.draft:${r.name}:${s.kind}:${where}`;
}
// Kept on the phone, not for the tab's session: Android reclaims an app
// left in the background, and a reply half written then is the reader's
// work, not scratch. Gone once sent, or once the box is emptied.
function readDraft(key: string): string {
  try { return localStorage.getItem(key) ?? sessionStorage.getItem(key) ?? ""; } catch { return ""; }
}
function writeDraft(key: string, v: string): void {
  try {
    if (v) localStorage.setItem(key, v);
    else localStorage.removeItem(key);
    sessionStorage.removeItem(key);
  } catch { /* private mode: not remembered */ }
}
function forgetDraft(key: string): void { writeDraft(key, ""); }
function loadDraft(): string { return readDraft(draftKey()); }
function saveDraft(v: string): void { writeDraft(draftKey(), v); }

// --- Rendering: diff ---------------------------------------------------------

function notesAt(r: Review, file: string, line: number): SessionEvent[] {
  return roots(r.events).filter((e) => {
    const w = whereIs(r, e);
    return w && w.file === file && w.line === line;
  });
}

function renderHunk(r: Review, file: DiffFile, index: number): HTMLElement {
  const hunk = file.hunks[index];
  const path = pathOf(file);
  const d = describeHunk(hunk.header);
  let n = spanOf(hunk.header).start;
  const rows: HTMLElement[] = [];
  for (const line of hunk.lines) {
    const at = line.kind === "removed" ? null : n++;
    const notes = at === null ? [] : notesAt(r, path, at);
    const flag = notes.find((x) => x.flag)?.flag ?? null;
    rows.push(
      h(
        "div",
        {
          class: `ln ln-${line.kind}${flag ? ` flag-${flag}` : ""}`,
          "data-line": at === null ? undefined : `${path}#${at}`,
          "data-anchor": at === null ? undefined : `${path}#${at}`,
          onClick: at === null ? undefined : () => openSheet({ kind: "note", title: `${path.split("/").pop()}:${at}`, at: { file: path, line: at, line_content: line.content } }),
        },
        h("span", { class: "ln-n", text: at === null ? "" : String(at) }),
        h("span", { class: "ln-t", text: (line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " ") + line.content }),
      ),
    );
    for (const note of notes) {
      const t = threadOf(r.events, note.seq);
      rows.push(
        h(
          "button",
          { class: `inline-note${note.flag ? ` flag-${note.flag}` : ""}`, onClick: () => openSheet({ kind: "thread", root: note.seq }) },
          // The latest word, which is what an answer looks like when it
          // arrives; the count says there is more above it.
          h("span", { class: "who", text: t[t.length - 1].from === "agent" ? "Claude" : "You" }),
          h("span", { text: plain(t[t.length - 1].text ?? "") }),
          t.length > 1 ? h("span", { class: "muted", text: ` · ${t.length} messages` }) : "",
        ),
      );
    }
  }
  return h("section", { class: "hunk" }, h("div", { class: "hunk-head" }, h("span", { text: d.lines }), h("span", { class: "muted", text: d.context })), ...rows);
}

function renderImage(r: Review, path: string, old: boolean, label: string): HTMLElement {
  const img = load(images, `${r.name}|${old ? "old" : "new"}|${path}`, () => api.fileImage(r.name, path, old));
  return h(
    "figure",
    { class: `bin-side bin-${old ? "old" : "new"}` },
    h("figcaption", { text: label }),
    typeof img === "string" && img !== "loading" ? h("img", { src: img, alt: path }) : h("div", { class: "muted small", text: img === "loading" ? "Loading…" : img.error }),
  );
}

/** A binary file: the image, both sides when it changed, or a line
 *  saying there is nothing to read. */
function renderBinary(r: Review, file: DiffFile): HTMLElement {
  const sides: HTMLElement[] = [];
  if (isImage(file.new_path ?? file.old_path)) {
    if (file.status !== "added" && file.old_path) sides.push(renderImage(r, file.old_path, true, file.status === "deleted" ? "Deleted" : "Before"));
    if (file.status !== "deleted" && file.new_path) sides.push(renderImage(r, file.new_path, false, file.status === "added" ? "Added" : "After"));
  }
  return h("section", { class: "binary" }, ...(sides.length ? sides : [h("p", { class: "muted small", text: "Binary file, not shown." })]));
}

/** A Markdown file, rendered whole with this run's changes marked in the
 *  margin and unchanged stretches folded, as on the desktop. */
function renderMarkdownRun(r: Review, run: FileRun): HTMLElement {
  const text = load(texts, `${r.name}|${run.path}`, () => api.fileText(r.name, run.path));
  if (!Array.isArray(text)) return h("div", { class: "md-doc muted", text: text === "loading" ? "Loading…" : `Could not read this file: ${text.error}` });
  const noted = roots(r.events)
    .map((e) => whereIs(r, e))
    .filter((w): w is NonNullable<typeof w> => !!w && w.file === run.path && !w.outdated)
    .map((w) => w.line);
  const tpl = document.createElement("template");
  // markdown-it with HTML off: every tag here is one it wrote.
  tpl.innerHTML = renderMarkdown(text.join("\n"), changesOf(run.file, run.hunks.map((e) => e.index)), noted);
  for (const a of tpl.content.querySelectorAll("a")) a.addEventListener("click", (e) => e.preventDefault());
  for (const span of tpl.content.querySelectorAll<HTMLElement>("[data-local-src]")) {
    const at = resolveRepoPath(run.path, span.dataset.localSrc!);
    if (!at || !isImage(at)) continue;
    const img = load(images, `${r.name}|new|${at}`, () => api.fileImage(r.name, at, false));
    if (typeof img === "string" && img !== "loading") span.replaceWith(h("img", { class: "md-img", src: img, alt: span.textContent ?? "" }));
  }
  for (const el of tpl.content.querySelectorAll<HTMLElement>("[data-note-line]")) {
    // A note sits on a source line, which only the diff has.
    el.prepend(h("button", { class: "md-note-btn", text: "Note ›", onClick: () => setMdMode(r.name, run.path, "diff") }));
  }
  const key = `${r.name}|${run.path}|${run.hunks.map((e) => e.index).join(",")}`;
  return h("div", { class: "md-doc" }, ...foldUnchanged([...tpl.content.children] as HTMLElement[], unfolded.has(key), () => { unfolded.add(key); render(); }));
}

function renderRuns(r: Review, runs: FileRun[]): HTMLElement[] {
  return runs.map((run) => {
    const md = isMarkdown(run.path) && run.file.status !== "deleted" && !run.file.binary;
    const mode = md ? mdMode(r.name, run.path) : "diff";
    return h(
      "div",
      { class: "run" },
      h(
        "div",
        { class: "file-head" },
        h("span", { class: "file-path", text: run.path }),
        md
          ? h(
              "span",
              { class: "md-toggle" },
              h("button", { class: mode === "rendered" ? "on" : "", text: "Rendered", onClick: () => setMdMode(r.name, run.path, "rendered") }),
              h("button", { class: mode === "diff" ? "on" : "", text: "Diff", onClick: () => setMdMode(r.name, run.path, "diff") }),
            )
          : "",
      ),
      ...(run.file.binary
        ? [renderBinary(r, run.file)]
        : md && mode === "rendered"
          ? [renderMarkdownRun(r, run)]
          : run.hunks.map((e) => renderHunk(r, run.file, e.index))),
    );
  });
}

// --- Rendering: pages --------------------------------------------------------

function markOf(r: Review, title: string): Mark | null {
  return r.chapters.find((c) => c.title === title)?.mark ?? null;
}

function renderPage(r: Review, p: Page, i: number): HTMLElement {
  const body: (HTMLElement | string)[] = [];
  if (p.kind === "brief") {
    const brief = r.events.find((e) => e.kind === "brief");
    body.push(
      h("h1", { class: "brief-title", text: brief?.title ?? r.name }),
      h("p", { class: "brief-summary", text: brief?.summary ?? "" }),
      brief?.implications?.length ? h("ul", { class: "brief-impl" }, ...brief.implications.map((x) => h("li", { text: x }))) : "",
      h("div", { class: "meta muted", text: [r.revision, r.diff ? `${r.diff.summary.files_changed} files, +${r.diff.summary.lines_added} −${r.diff.summary.lines_removed}` : ""].filter(Boolean).join(" · ") }),
      r.diff && !r.diff.pin ? h("div", { class: "warn", text: "Not pinned: this diff includes files on disk and can change while you read it." }) : "",
      r.diffError ? h("div", { class: "warn", text: r.diffError }) : "",
      ...currentDiagnostics(r.events).slice(-3).map((e) => h("div", { class: "diag muted", text: e.message ?? "" })),
      r.pages.length > 1 ? h("p", { class: "hint muted", text: "Swipe for the chapters →" }) : "",
    );
  } else if (p.kind === "chapter") {
    const m = markOf(r, p.chapter.title);
    body.push(
      h("div", { class: "chapter-no muted", text: `Chapter ${p.index + 1} of ${r.pages.filter((x) => x.kind === "chapter").length}` }),
      h("h2", { class: `chapter-title${m ? ` marked-${m}` : ""}`, text: p.chapter.title }),
      p.chapter.summary ? h("p", { class: "chapter-summary", text: p.chapter.summary }) : "",
      ...renderRuns(r, p.runs),
      h(
        "div",
        { class: "marks" },
        h("button", { class: `mark ok${m === "ok" ? " on" : ""}`, text: m === "ok" ? "✓ Looks OK" : "Looks OK", onClick: () => void setMark(p.chapter.title, m === "ok" ? "clear" : "ok") }),
        h("button", { class: `mark concern${m === "concern" ? " on" : ""}`, text: m === "concern" ? "! Concern" : "Concern", onClick: () => void setMark(p.chapter.title, m === "concern" ? "clear" : "concern") }),
        h("button", { class: "mark", text: "Comment", onClick: () => openSheet({ kind: "note", title: p.chapter.title, at: { chapter: p.chapter.title } }) }),
      ),
    );
  } else if (p.kind === "unbundled") {
    body.push(
      h("h2", { class: "chapter-title", text: "Not in any chapter" }),
      h("p", { class: "warn", text: "Claude's narrative does not mention these changes. Read them anyway." }),
      ...renderRuns(r, p.runs),
    );
  } else {
    body.push(...renderRuns(r, [p.run]));
  }
  return h("article", { class: "page", "data-page": String(i), "data-keep": `page:${r.name}:${i}` }, ...body);
}

function pageLabel(p: Page): string {
  return p.kind === "brief" ? "Brief" : p.kind === "chapter" ? p.chapter.title : p.kind === "unbundled" ? "Not in any chapter" : p.run.path;
}

/** Flags Claude raised that want a look, in page order. */
function flagged(r: Review): { seq: number; page: number; key: string }[] {
  const out: { seq: number; page: number; key: string }[] = [];
  for (const e of roots(r.events)) {
    if (e.flag !== "attention" && e.flag !== "caution") continue;
    const w = whereIs(r, e);
    if (!w || w.outdated) continue;
    const page = r.pages.findIndex((p) => {
      const runs = p.kind === "chapter" || p.kind === "unbundled" ? p.runs : p.kind === "file" ? [p.run] : [];
      return runs.some((run) => run.path === w.file);
    });
    if (page >= 0) out.push({ seq: e.seq, page, key: `${w.file}#${w.line}` });
  }
  return out.sort((a, b) => a.page - b.page || a.seq - b.seq);
}

let flagCursor = -1;
function nextFlag(): void {
  const r = state.review;
  if (!r) return;
  const all = flagged(r);
  if (!all.length) return;
  flagCursor = (flagCursor + 1) % all.length;
  const f = all[flagCursor];
  goToPage(f.page);
  requestAnimationFrame(() => {
    const el = document.querySelector<HTMLElement>(`.page[data-page="${f.page}"] [data-line="${CSS.escape(f.key)}"]`);
    el?.scrollIntoView({ block: "center", behavior: "smooth" });
    el?.classList.add("pulse");
  });
}

function goToPage(i: number): void {
  const pager = document.querySelector<HTMLElement>(".pager");
  if (!pager || !state.review) return;
  const far = Math.abs(i - state.review.page) > 1;
  state.review.page = i;
  fillNear(state.review);
  // A jump across pages lands at once: smooth-scrolling past pages that
  // are not built yet is slow, and anything that redraws mid-animation
  // would stop it partway. A step to a neighbour slides, like a swipe.
  pager.scrollTo({ left: i * pager.clientWidth, behavior: far ? "instant" : "smooth" });
  renderDots();
}

/** Pages built: the one being read and its neighbours, so a swipe lands
 *  on a page that is already there. A review can run to thousands of
 *  lines, and building all of them on every tap is what made the phone
 *  slow. The rest are empty boxes of the right width until approached. */
function near(r: Review, i: number): boolean {
  return Math.abs(i - r.page) <= 1;
}

function emptyPage(i: number): HTMLElement {
  return h("article", { class: "page", "data-page": String(i), "data-empty": "1" });
}

/** Build the pages that have just come near, in place, without
 *  rebuilding the screen. */
function fillNear(r: Review): void {
  for (const el of document.querySelectorAll<HTMLElement>(".page[data-empty]")) {
    const i = Number(el.dataset.page);
    if (near(r, i) && r.pages[i]) el.replaceWith(renderPage(r, r.pages[i], i));
  }
}

function restorePage(): void {
  const pager = document.querySelector<HTMLElement>(".pager");
  if (pager && state.review) pager.scrollLeft = state.review.page * pager.clientWidth;
}

function renderDots(): void {
  const r = state.review;
  const el = document.querySelector(".dots");
  if (!r || !el) return;
  el.replaceChildren(...r.pages.map((p, i) => h("button", { class: `dot${i === r.page ? " on" : ""}${p.kind === "chapter" && markOf(r, p.chapter.title) ? ` marked-${markOf(r, p.chapter.title)}` : ""}`, "aria-label": pageLabel(p), onClick: () => goToPage(i) })));
  const label = document.querySelector(".page-label");
  if (label) label.textContent = pageLabel(r.pages[r.page]);
}

// --- Rendering: screens -----------------------------------------------------

function ago(ms: number): string {
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}

/** Full commit ids cut to git's short form, so a pinned range fits a
 *  phone row. */
function shortRevision(rev: string): string {
  return rev.replace(/\b[0-9a-f]{12,40}\b/g, (oid) => oid.slice(0, 8));
}

function urlBase64ToBytes(s: string): Uint8Array<ArrayBuffer> {
  const pad = "=".repeat((4 - (s.length % 4)) % 4);
  const raw = atob((s + pad).replace(/-/g, "+").replace(/_/g, "/"));
  const out = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

async function loadPushState(): Promise<void> {
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) {
    state.push = "unsupported";
  } else if (Notification.permission === "denied") {
    state.push = "blocked";
  } else {
    try {
      const k = await api.pushKey();
      const reg = await navigator.serviceWorker.ready;
      const sub = await reg.pushManager.getSubscription();
      state.push = k.subscribed && sub ? "on" : "off";
      // Sent again each time, with its keys: a subscription made before
      // notifications carried text was saved without them, and would
      // otherwise stay silent about what Claude said.
      if (sub && k.subscribed) void api.pushSubscribe(sub.toJSON()).catch(() => {});
    } catch { state.push = "off"; }
  }
  render();
}

/** Ask for notifications and register this phone with the Mac. Runs from
 *  a tap, which the permission prompt requires. */
async function turnOnPush(): Promise<void> {
  try {
    const permission = await Notification.requestPermission();
    if (permission !== "granted") { state.push = permission === "denied" ? "blocked" : "off"; render(); return; }
    const { key } = await api.pushKey();
    const reg = await navigator.serviceWorker.ready;
    const existing = await reg.pushManager.getSubscription();
    // A subscription made with another machine's key cannot be reused.
    if (existing) await existing.unsubscribe();
    const sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: urlBase64ToBytes(key) });
    await api.pushSubscribe(sub.toJSON());
    state.push = "on";
  } catch (e) { handle(e); }
  render();
}

async function turnOffPush(): Promise<void> {
  try {
    const reg = await navigator.serviceWorker.ready;
    await (await reg.pushManager.getSubscription())?.unsubscribe();
    await api.pushUnsubscribe();
    state.push = "off";
  } catch (e) { handle(e); }
  render();
}

function renderPushRow(): HTMLElement | string {
  const name = state.machine?.name ?? "this Mac";
  switch (state.push) {
    case "off":
      return h("button", { class: "push-row", text: `🔔 Buzz me when Claude is waiting on ${name}`, onClick: () => void turnOnPush() });
    case "on":
      return h("div", { class: "push-row on" }, h("span", { text: `🔔 Notifications on for ${name}` }), h("button", { class: "push-off", text: "Turn off", onClick: () => void turnOffPush() }));
    case "blocked":
      return h("div", { class: "push-row muted", text: "Notifications are blocked for this site. Allow them in Chrome's site settings to get a buzz." });
    default:
      return "";
  }
}

// --- Threads: the home screen -------------------------------------------------
//
// On a phone the job is to keep Claude moving: answer what it asked, reply
// to what it answered, acknowledge what it said. So home is every
// conversation across every open review, with what is waiting on you
// first, and the reviews themselves one tab away.

function threadKey(t: { review: string; root: number }): string {
  return `${t.review}#${t.root}`;
}

/** Highest seq seen per thread, on this phone. */
function seenMap(): Record<string, number> {
  try { return JSON.parse(localStorage.getItem("threshold.seen") ?? "{}"); } catch { return {}; }
}
function markSeen(t: ThreadCard, undoable = false): void {
  const seen = seenMap();
  const key = threadKey(t);
  const before = seen[key];
  seen[key] = Math.max(...t.messages.map((m) => m.seq));
  saveSeen(seen);
  if (undoable) offerUndo(t, before);
  render();
}

function saveSeen(seen: Record<string, number>): void {
  try { localStorage.setItem("threshold.seen", JSON.stringify(seen)); } catch { /* not remembered */ }
}

/** The last swipe, for a few seconds: a swipe is easy to do by accident. */
let undo: { title: string; key: string; before: number | undefined; timer: number } | null = null;

function offerUndo(t: ThreadCard, before: number | undefined): void {
  if (undo) clearTimeout(undo.timer);
  undo = { title: t.title, key: threadKey(t), before, timer: window.setTimeout(() => { undo = null; render(); }, 5000) };
}

function undoSeen(): void {
  if (!undo) return;
  const seen = seenMap();
  if (undo.before === undefined) delete seen[undo.key];
  else seen[undo.key] = undo.before;
  saveSeen(seen);
  clearTimeout(undo.timer);
  undo = null;
  render();
}

function renderUndo(): HTMLElement | string {
  if (!undo) return "";
  return h(
    "div",
    { class: "undo-bar" },
    h("span", { class: "undo-text", text: `Seen: ${undo.title}` }),
    h("button", { class: "undo-btn", text: "Undo", onClick: undoSeen }),
  );
}

/** Let `row` be swiped sideways, calling `done` once it has gone far
 *  enough. The drag commits to horizontal or vertical after a few pixels;
 *  vertical is left to the page, so scrolling a list of rows is unchanged.
 *  A drag that moved never also counts as a tap. */
function swipeable(row: HTMLElement, done: () => void): void {
  let x0 = 0, y0 = 0, dx = 0, id = -1;
  let phase: "idle" | "deciding" | "dragging" = "idle";
  let moved = false;
  const reset = (animate: boolean) => {
    row.style.transition = animate ? "transform .18s ease-out" : "";
    row.style.transform = "";
    row.parentElement?.classList.remove("armed");
  };
  row.addEventListener("pointerdown", (e) => {
    if (e.pointerType === "mouse" && e.button !== 0) return;
    x0 = e.clientX; y0 = e.clientY; dx = 0; id = e.pointerId; phase = "deciding"; moved = false;
    row.style.transition = "";
  });
  row.addEventListener("pointermove", (e) => {
    if (e.pointerId !== id || phase === "idle") return;
    const mx = e.clientX - x0, my = e.clientY - y0;
    if (phase === "deciding") {
      if (Math.abs(mx) < 10 && Math.abs(my) < 10) return;
      if (Math.abs(my) >= Math.abs(mx)) { phase = "idle"; return; }
      phase = "dragging";
      moved = true;
      row.setPointerCapture(e.pointerId);
    }
    dx = mx;
    row.style.transform = `translateX(${dx}px)`;
    row.parentElement?.classList.toggle("armed", Math.abs(dx) > row.offsetWidth * 0.33);
  });
  const end = (e: PointerEvent) => {
    if (e.pointerId !== id) return;
    const was = phase;
    phase = "idle";
    if (was !== "dragging") return;
    if (Math.abs(dx) > row.offsetWidth * 0.33) {
      navigator.vibrate?.(12);
      row.style.transition = "transform .15s ease-in";
      row.style.transform = `translateX(${dx > 0 ? "" : "-"}110%)`;
      setTimeout(done, 150);
    } else {
      reset(true);
    }
  };
  row.addEventListener("pointerup", end);
  row.addEventListener("pointercancel", (e) => { if (e.pointerId === id) { phase = "idle"; reset(true); } });
  // Swallow the click that ends a drag, so a swipe that sprang back does
  // not also open the thread.
  row.addEventListener("click", (e) => { if (moved) { e.preventDefault(); e.stopImmediatePropagation(); moved = false; } }, true);
}

function lastOf(t: ThreadCard) { return t.messages[t.messages.length - 1]; }

type Whose = "yours" | "claude" | "seen";
function whose(t: ThreadCard, seen: Record<string, number>): Whose {
  const last = lastOf(t);
  if (!last || last.from === "human") return "claude";
  return last.seq > (seen[threadKey(t)] ?? 0) ? "yours" : "seen";
}

async function loadThreads(live = false): Promise<void> {
  try {
    const r = await api.threads();
    const same = state.threadsLoaded && JSON.stringify(r) === JSON.stringify({ threads: state.threads, fresh: state.fresh });
    if (live && same && state.live) return;
    state.threads = r.threads;
    state.fresh = r.fresh;
    state.threadsLoaded = true;
    state.live = true;
  } catch (e) { handle(e); }
  if (live) renderLive(); else render();
}

function goHome(tab: "threads" | "inbox"): void {
  moved();
  if (state.screen === "threads" || state.screen === "inbox") remember();
  state.home = tab;
  state.screen = tab;
  state.review = null;
  state.sheet = null;
  // Not seen since a reload: where this entry last showed it.
  awaitScrolls((hs().scrolls ?? []).filter(([k]) => !scrolls.has(k)));
  if (tab === "threads") void loadThreads();
  else void loadInbox();
  render();
  record();
}

function tabs(): HTMLElement {
  const seen = seenMap();
  const yours = state.threads.filter((t) => whose(t, seen) === "yours").length + state.fresh.length;
  const waiting = state.reviews.filter((r) => !r.decided && !r.archived).length;
  return h(
    "nav",
    { class: "home-tabs" },
    h("button", { class: `tab${state.screen === "threads" ? " on" : ""}`, text: yours ? `Threads · ${yours}` : "Threads", onClick: () => goHome("threads") }),
    h("button", { class: `tab${state.screen === "inbox" ? " on" : ""}`, text: waiting ? `Reviews · ${waiting}` : "Reviews", onClick: () => goHome("inbox") }),
  );
}

/** Open a review at a thread's place: its line, its chapter, or the brief. */
async function openAt(t: ThreadCard): Promise<void> {
  // A Markdown file shows rendered by default, but a thread sits on a
  // source line, which only the diff has.
  if (t.file && isMarkdown(t.file)) {
    try { localStorage.setItem(`threshold.md:${t.review}|${t.file}`, "diff"); } catch { /* fine */ }
  }
  await go("threads", t.review, { at: { file: t.file, line: t.line, chapter: t.chapter } });
  const r = state.review;
  if (!r || r.name !== t.review || !t.file || !t.line) return;
  const el = document.querySelector<HTMLElement>(`.page[data-page="${r.page}"] [data-line="${CSS.escape(`${t.file}#${t.line}`)}"]`);
  el?.scrollIntoView({ block: "center" });
  el?.classList.add("pulse");
}

function threadDraftKey(t: ThreadCard): string {
  return `threshold.draft:${threadKey(t)}`;
}

/** Threads with a reply on its way, so only that card says so. */
const sending = new Set<string>();

async function replyTo(t: ThreadCard, box: HTMLTextAreaElement): Promise<void> {
  const text = box.value.trim();
  const key = threadKey(t);
  if (!text || sending.has(key)) return;
  sending.add(key);
  render();
  try {
    await api.note(t.review, text, { in_reply_to: t.root });
    forgetDraft(threadDraftKey(t));
    await loadThreads();
  } catch (e) { handle(e); }
  sending.delete(key);
  render();
}

function where(t: ThreadCard): string {
  if (t.file) return `${t.file.split("/").pop()}${t.line ? `:${t.line}` : ""}`;
  return t.chapter ?? "The whole change";
}

/** A thread as one tappable row: where it is, the last thing said, and
 *  how long ago. Opened, it is the whole card with its reply box. */
function renderThreadRow(t: ThreadCard, kind: Whose): HTMLElement {
  const key = threadKey(t);
  if (state.open.has(key)) return renderThreadCard(t, kind);
  const last = lastOf(t);
  const row = h(
    "button",
    { class: `thread-line thread-${kind}`, "data-anchor": key, onClick: () => { state.open.add(key); render(); } },
    h(
      "span",
      { class: "thread-line-top" },
      h("span", { class: "thread-line-title", text: t.title }),
      h("span", { class: "thread-line-age muted", text: ago(last.at_ms) }),
    ),
    h("span", { class: "thread-line-where muted", text: [t.project, where(t)].filter(Boolean).join(" · ") }),
    h("span", { class: "thread-line-text" }, h("b", { text: last.from === "agent" ? "Claude: " : "You: " }), plain(last.text)),
  );
  // Only what is waiting on you can be swiped away: that is what "seen"
  // means.
  if (kind !== "yours") return row;
  swipeable(row, () => markSeen(t, true));
  return h("div", { class: "swipe-wrap", "data-anchor": key }, h("span", { class: "swipe-under left", text: "✓ Seen" }), h("span", { class: "swipe-under right", text: "✓ Seen" }), row);
}

function renderThreadCard(t: ThreadCard, kind: Whose): HTMLElement {
  const key = threadKey(t);
  const all = t.messages;
  const shown = state.expanded.has(key) ? all : all.slice(-3);
  const box = h("textarea", {
    rows: "2",
    "data-keep": `reply:${key}`,
    placeholder: kind === "yours" ? "Reply to Claude…" : "Add to this thread…",
    onInput: (e) => writeDraft(threadDraftKey(t), (e.target as HTMLTextAreaElement).value),
  });
  box.value = readDraft(threadDraftKey(t));
  return h(
    "article",
    { class: `card card-${kind}`, "data-anchor": key },
    h(
      "button",
      { class: "card-head", title: "Collapse", onClick: () => { state.open.delete(key); render(); } },
      h("span", { class: "card-title", text: `▾ ${t.title}` }),
      h("span", { class: "card-meta muted", text: [t.project, where(t)].filter(Boolean).join(" · ") }),
    ),
    t.code ? h("pre", { class: "card-code", text: t.code }) : "",
    all.length > shown.length
      ? h("button", { class: "card-earlier muted", text: `${all.length - shown.length} earlier`, onClick: () => { state.expanded.add(key); render(); } })
      : "",
    h("div", { class: "chat", "data-chat": `card:${key}`, "data-last": String(lastOf(t)?.seq ?? 0) }, ...shown.map((m) => h("div", { class: `msg ${m.from}` }, h("span", { class: "who", text: `${m.from === "agent" ? "Claude" : "You"} · ${ago(m.at_ms)}` }), messageBody(m.text)))),
    box,
    h(
      "div",
      { class: "card-actions" },
      h("button", { class: "send", text: sending.has(key) ? "Sending…" : "Reply", disabled: sending.has(key), onClick: () => void replyTo(t, box) }),
      kind === "yours" ? h("button", { class: "ghost", text: "Seen", title: "Nothing more to say for now", onClick: () => markSeen(t) }) : "",
      h("button", { class: "ghost", text: "In review ›", onClick: () => void openAt(t) }),
    ),
  );
}

function renderFresh(f: FreshReview): HTMLElement {
  return h(
    "article",
    { class: "card card-fresh", "data-anchor": `fresh:${f.review}` },
    h(
      "button",
      { class: "card-head", onClick: () => void go("threads", f.review) },
      h("span", { class: "card-title", text: f.title }),
      h("span", { class: "card-meta muted", text: [f.project, "new review", f.flags ? `${f.flags} flagged` : "", ago(f.at_ms)].filter(Boolean).join(" · ") }),
    ),
    f.summary ? h("p", { class: "card-summary", text: f.summary }) : "",
    h("div", { class: "card-actions" }, h("button", { class: "send", text: "Read ›", onClick: () => void go("threads", f.review) })),
  );
}

/** The projects with something on the threads screen, as chips that
 *  narrow the list to one. Counts are what is waiting on you there. */
function renderProjectChips(seen: Record<string, number>): HTMLElement | string {
  const count = new Map<string, number>();
  for (const t of state.threads) if (t.project && whose(t, seen) === "yours") count.set(t.project, (count.get(t.project) ?? 0) + 1);
  for (const f of state.fresh) if (f.project) count.set(f.project, (count.get(f.project) ?? 0) + 1);
  const all = [...new Set([...state.threads, ...state.fresh].map((x) => x.project).filter((p): p is string => !!p))];
  if (all.length < 2) return "";
  all.sort((a, b) => (count.get(b) ?? 0) - (count.get(a) ?? 0) || a.localeCompare(b));
  const chip = (p: string | null, label: string) =>
    h("button", { class: `chip${state.project === p ? " on" : ""}`, text: label, onClick: () => { state.project = p; render(); } });
  return h("nav", { class: "chips" }, chip(null, "All"), ...all.map((p) => chip(p, count.get(p) ? `${p} · ${count.get(p)}` : p)));
}

function renderHome(): HTMLElement {
  const seen = seenMap();
  const inProject = <T extends { project: string | null }>(x: T) => !state.project || x.project === state.project;
  const recent = (a: ThreadCard, b: ThreadCard) => lastOf(b).at_ms - lastOf(a).at_ms;
  const by = (k: Whose) => state.threads.filter((t) => whose(t, seen) === k && inProject(t)).sort(recent);
  // Most recent first, in every section.
  const yours = by("yours");
  const claude = by("claude");
  const done = by("seen");
  const fresh = state.fresh.filter(inProject).sort((a, b) => b.at_ms - a.at_ms);
  const nothing = state.threadsLoaded && !yours.length && !fresh.length;
  return h(
    "main",
    { class: "threads-home", "data-keep": "home" },
    renderUndo(),
    tabs(),
    renderPushRow(),
    renderProjectChips(seen),
    !state.threadsLoaded ? h("p", { class: "pad muted", text: "Loading…" }) : "",
    nothing ? h("p", { class: "caught-up", text: `Nothing waiting on you${state.project ? ` in ${state.project}` : ""} on ${state.machine?.name ?? "this Mac"}.` }) : "",
    yours.length ? h("h2", { class: "section", text: `Your move · ${yours.length}` }) : "",
    ...yours.map((t) => renderThreadRow(t, "yours")),
    fresh.length ? h("h2", { class: "section", text: `New reviews · ${fresh.length}` }) : "",
    ...fresh.map(renderFresh),
    claude.length
      ? h("button", { class: "section section-toggle", text: `${state.showWaiting ? "▾" : "▸"} Waiting on Claude · ${claude.length}`, onClick: () => { state.showWaiting = !state.showWaiting; render(); } })
      : "",
    // A card left open stays in view when its section is folded: replying
    // moves a thread here, and it must not vanish from under the reply.
    ...(state.showWaiting ? claude : claude.filter((t) => state.open.has(threadKey(t)))).map((t) => renderThreadRow(t, "claude")),
    done.length
      ? h("button", { class: "section section-toggle", text: `${state.showSeen ? "▾" : "▸"} Seen · ${done.length}`, onClick: () => { state.showSeen = !state.showSeen; render(); } })
      : "",
    ...(state.showSeen ? done : done.filter((t) => state.open.has(threadKey(t)))).map((t) => renderThreadRow(t, "seen")),
  );
}

function renderInbox(): HTMLElement {
  const waiting = state.reviews.filter((r) => !r.decided && !r.archived);
  const decided = state.reviews.filter((r) => r.decided).slice(0, 20);
  const row = (r: ReviewRow) =>
    h(
      "button",
      { class: `row${r.decided ? " decided" : ""}`, "data-anchor": r.name, onClick: () => void go("inbox", r.name) },
      h("span", { class: "row-title", text: r.title }),
      h("span", { class: "row-meta muted", text: [r.project, r.revision && shortRevision(r.revision), ago(r.last_at_ms)].filter(Boolean).join(" · ") }),
    );
  return h(
    "main",
    { class: "inbox", "data-keep": "inbox" },
    tabs(),
    h("h2", { class: "section", text: !state.loaded ? "Waiting on you · unknown" : waiting.length ? `Waiting on you · ${waiting.length}` : "Nothing waiting" }),
    ...waiting.map(row),
    decided.length ? h("h2", { class: "section", text: "Decided" }) : "",
    ...decided.map(row),
  );
}

function renderThreads(r: Review): HTMLElement {
  const rs = roots(r.events).slice().reverse();
  return h(
    "main",
    { class: "threads", "data-keep": `threads:${r.name}` },
    h("button", { class: "new-note", text: "+ Ask Claude something", onClick: () => openSheet({ kind: "note", title: "To Claude", at: {} }) }),
    ...(rs.length
      ? rs.map((e) => {
          const t = threadOf(r.events, e.seq);
          const w = whereIs(r, e);
          const last = t[t.length - 1];
          return h(
            "button",
            { class: `thread-row${last.from === "agent" ? " theirs" : ""}`, "data-anchor": String(e.seq), onClick: () => openSheet({ kind: "thread", root: e.seq }) },
            h("span", { class: "thread-where muted", text: w ? `${w.file.split("/").pop()}:${w.line}${w.outdated ? " (outdated)" : ""}` : e.chapter ?? "General" }),
            h("span", { class: "thread-text", text: plain(last.text ?? "") }),
            h("span", { class: "muted", text: `${last.from === "agent" ? "Claude" : "You"} · ${ago(last.at_ms)}` }),
          );
        })
      : [h("p", { class: "muted pad", text: "No threads yet." })]),
  );
}

function renderReview(r: Review): HTMLElement {
  const verdict = r.events.find((e) => e.kind === "verdict");
  const flags = flagged(r).length;
  const waiting = unanswered(r.events);
  return h(
    "div",
    { class: "review" },
    h(
      "nav",
      { class: "review-bar" },
      h("button", { class: "back", text: state.home === "threads" ? "‹ Threads" : "‹ Reviews", onClick: back }),
      h(
        "div",
        { class: "tabs" },
        h("button", { class: `tab${state.tab === "read" ? " on" : ""}`, text: "Read", onClick: () => { state.tab = "read"; render(); restorePage(); record(); } }),
        h("button", { class: `tab${state.tab === "threads" ? " on" : ""}`, text: waiting ? `Threads · ${waiting}` : "Threads", onClick: () => { state.tab = "threads"; render(); record(); } }),
      ),
    ),
    r.revised
      ? h("button", { class: "revised", text: "Claude revised this review. Tap to load the new version.", onClick: () => void openReview(r.name, { keep: true }) })
      : "",
    state.tab === "threads"
      ? renderThreads(r)
      : h(
          "div",
          { class: "reader" },
          h("div", { class: "page-label muted", text: pageLabel(r.pages[r.page] ?? r.pages[0]) }),
          h(
            "div",
            {
              class: "pager",
              onScroll: (e) => {
                const el = e.target as HTMLElement;
                const i = Math.round(el.scrollLeft / Math.max(1, el.clientWidth));
                if (i !== r.page) { r.page = i; renderDots(); fillNear(r); record(); }
              },
            },
            ...r.pages.map((p, i) => (near(r, i) ? renderPage(r, p, i) : emptyPage(i))),
          ),
          h("div", { class: "dots" }),
          flags ? h("button", { class: "fab", text: `⚑ ${flags}`, "aria-label": "Next flagged line", onClick: nextFlag }) : "",
        ),
    h(
      "footer",
      { class: "verdict-bar" },
      verdict
        ? h("div", { class: `decided-${verdict.verdict}`, text: `${verdict.verdict === "approved" ? "Approved" : "Rejected"}${verdict.reason ? `: ${verdict.reason}` : ""}` })
        : [
            h("button", { class: "reject", text: "Reject…", disabled: !r.diff, onClick: () => openSheet({ kind: "reject" }) }),
            h("button", { class: "approve", text: "Approve…", disabled: !r.diff, onClick: () => openSheet({ kind: "approve" }) }),
          ],
    ),
  );
}

function openSheet(s: Sheet): void {
  moved();
  remember();
  state.sheet = s;
  render();
  history.pushState({ ...fresh(), sheet: true }, "", urlOf(hereNow()));
  // A conversation opens on its latest message, beside the reply box.
  const sheet = document.querySelector<HTMLElement>(".sheet");
  if (sheet && s.kind === "thread") sheet.scrollTop = sheet.scrollHeight;
  document.querySelector<HTMLTextAreaElement>(".sheet textarea")?.focus({ preventScroll: s.kind === "thread" });
}

/** Close the sheet as Back would: it is a step in history. */
function closeSheet(): void {
  if (hs().sheet) { history.back(); return; }
  state.sheet = null;
  render();
  restorePage();
  record();
}

function renderSheet(): HTMLElement | string {
  const s = state.sheet;
  const r = state.review;
  if (!s || !r) return "";
  const m = state.machine;
  const box = h("textarea", {
    rows: "4",
    "data-keep": `sheet-box:${draftKey()}`,
    placeholder: s.kind === "reject" ? "What should Claude fix? Required." : s.kind === "approve" ? "Anything to add? Optional." : "Write to Claude…",
    onInput: (e) => saveDraft((e.target as HTMLTextAreaElement).value),
  });
  box.value = loadDraft();
  const close = closeSheet;
  let head: (HTMLElement | string)[];
  if (s.kind === "thread") {
    const t = threadOf(r.events, s.root);
    head = [
      h("div", { class: "sheet-title", text: "Thread" }),
      h("div", { class: "chat", "data-chat": `sheet:${r.name}#${s.root}`, "data-last": String(t[t.length - 1]?.seq ?? 0) }, ...t.map((e) => h("div", { class: `msg ${e.from}` }, h("span", { class: "who", text: e.from === "agent" ? "Claude" : "You" }), messageBody(e.text ?? "")))),
    ];
  } else if (s.kind === "reject" || s.kind === "approve") {
    // The machine is named on the button itself: these end a review, and
    // must not land on the wrong Mac.
    const verb = s.kind === "reject" ? "Reject" : "Approve";
    const size = r.diff ? `${r.diff.summary.files_changed} files, +${r.diff.summary.lines_added} −${r.diff.summary.lines_removed}` : "";
    head = [
      h("div", { class: "sheet-title", text: `${verb} on ${m?.name ?? "this Mac"}` }),
      h("p", { class: "muted", text: [r.events.find((e) => e.kind === "brief")?.title ?? r.name, size].filter(Boolean).join(" · ") }),
    ];
  } else {
    head = [h("div", { class: "sheet-title", text: s.title })];
  }
  return h(
    "div",
    { class: "scrim", onClick: (e) => { if (e.target === e.currentTarget) close(); } },
    h(
      "div",
      { class: "sheet", "data-keep": `sheet:${draftKey()}` },
      band(),
      ...head,
      box,
      h(
        "div",
        { class: "sheet-actions" },
        h("button", { class: "ghost", text: "Cancel", onClick: close }),
        h("button", {
          class: s.kind === "reject" ? "reject" : s.kind === "approve" ? "approve" : "send",
          text: state.busy ? "Sending…" : s.kind === "reject" ? `Reject on ${m?.name ?? "this Mac"}` : s.kind === "approve" ? `Approve on ${m?.name ?? "this Mac"}` : "Send",
          disabled: state.busy,
          // An approval may say nothing; everything else needs words.
          onClick: () => { if (box.value.trim() || s.kind === "approve") void submitSheet(box.value.trim()); },
        }),
      ),
    ),
  );
}

function renderPairing(): HTMLElement {
  const m = state.machine;
  const nameBox = h("input", { class: "field", placeholder: "Name this phone, e.g. Pixel", value: "" });
  return h(
    "main",
    { class: "pairing" },
    h("h1", { text: m ? `Pair with ${m.name}?` : "Pairing…" }),
    h("p", { class: "muted", text: "This phone will be able to read reviews on this Mac, write to Claude, mark chapters, and approve or reject." }),
    nameBox,
    h("button", {
      class: "send big",
      text: m ? `Pair with ${m.name}` : "Pair",
      disabled: !state.pairToken || state.busy,
      onClick: async () => {
        state.busy = true;
        render();
        try {
          state.machine = await api.pair(state.pairToken!, nameBox.value.trim() || "phone");
          applyMachine(state.machine);
          state.pairToken = null;
          history.replaceState(null, "", "/");
          state.screen = "threads";
          startListening();
          await loadInbox();
          await loadThreads();
          void loadPushState();
        } catch (e) {
          state.notice = isApiError(e) ? e.message : String(e);
        } finally {
          state.busy = false;
          render();
        }
      },
    }),
    h("p", { class: "muted small", text: "Then add it to your home screen so it opens like an app: in Chrome, ⋮ → Add to Home screen. (iPhone: Safari → Share → Add to Home Screen.)" }),
  );
}

function renderUnpaired(): HTMLElement {
  const m = state.machine;
  return h(
    "main",
    { class: "pairing" },
    h("h1", { text: "This phone is not paired" }),
    h("p", { class: "muted", text: `On ${m ? `the Mac called “${m.name}”` : "the Mac"}, open Threshold → Phone and scan the code with your camera.` }),
  );
}

function renderMoved(): HTMLElement {
  return h(
    "main",
    { class: "pairing" },
    h("h1", { text: "This address is a different Mac now" }),
    h("p", { text: `This page was paired with “${state.was?.name ?? "another Mac"}”. This address now answers as “${state.machine?.name}”. Nothing has been sent to it.` }),
    h("p", { class: "muted", text: "To use this Mac, pair again from its Threshold → Phone." }),
  );
}

function render(): void {
  applyMachine(state.machine);
  let body: HTMLElement;
  if (state.screen === "pair") body = renderPairing();
  else if (state.screen === "unpaired") body = renderUnpaired();
  else if (state.screen === "moved") body = renderMoved();
  else if (state.screen === "review" && state.review) body = renderReview(state.review);
  else if (state.screen === "inbox") body = renderInbox();
  else body = renderHome();

  const pager = document.querySelector<HTMLElement>(".pager");
  const left = pager?.scrollLeft ?? 0;
  const focus = capture();

  root.replaceChildren(
    band(),
    state.notice ? h("button", { class: "notice", text: `${state.notice}  ✕`, onClick: () => { state.notice = null; render(); } }) : "",
    body,
    state.live ? "" : h("div", { class: "down-line", text: `Threshold is not answering on ${state.machine?.name ?? "the Mac"}. It has to be running there, and this phone on Tailscale.` }),
    renderSheet(),
  );

  const newPager = document.querySelector<HTMLElement>(".pager");
  if (newPager) {
    // The page being read, not the pixel offset: a redraw during a
    // sliding scroll would otherwise freeze it between pages.
    newPager.scrollLeft = state.review ? state.review.page * newPager.clientWidth : left;
  }
  restore(focus);
  renderDots();
  rememberSoon();
}

// --- Staying put across a redraw -------------------------------------------
//
// A redraw builds the screen afresh, and a fresh scrolling list starts at
// the top. Live updates arrive every few seconds while Claude is working,
// so without this the list jumped to the top under the reader, and the box
// they were typing in lost the keyboard.
//
// Every scrolling element carries data-keep, naming it across redraws, and
// its rows carry data-anchor. What is kept is the first row in view and how
// far down it sat, not the pixel offset, so a thread arriving above the
// one being read does not push it out of view. Kept per name for the life
// of the page, so coming back from a review lands where the list was left.

const scrolls = new Map<string, { top: number; anchor: string | null; offset: number }>();

function capture(): { key: string; start: number | null; end: number | null } | null {
  for (const el of document.querySelectorAll<HTMLElement>("[data-keep]")) {
    if (el.tagName === "TEXTAREA" || el.tagName === "INPUT" || awaiting.has(el.dataset.keep!)) continue;
    const top = el.getBoundingClientRect().top;
    let anchor: string | null = null;
    let offset = 0;
    // Rows run down the page in order, so the first in view is found by
    // halving: a diff page can hold thousands of lines.
    const rows = el.querySelectorAll<HTMLElement>("[data-anchor]");
    let lo = 0, hi = rows.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (rows[mid].getBoundingClientRect().bottom > top + 1) hi = mid;
      else lo = mid + 1;
    }
    if (lo < rows.length) { anchor = rows[lo].dataset.anchor!; offset = rows[lo].getBoundingClientRect().top - top; }
    scrolls.set(el.dataset.keep!, { top: el.scrollTop, anchor, offset });
  }
  const f = document.activeElement;
  if ((f instanceof HTMLTextAreaElement || f instanceof HTMLInputElement) && f.dataset.keep) {
    return { key: f.dataset.keep, start: f.selectionStart, end: f.selectionEnd };
  }
  return null;
}

/** Scroll positions from before a reload, waiting for the rows they name
 *  to load: drawn first with nothing in them, a list cannot be scrolled. */
const awaiting = new Map<string, { top: number; anchor: string | null; offset: number }>();
let awaitingUntil = 0;

function awaitScrolls(kept: NonNullable<Kept["scrolls"]>): void {
  for (const [k, v] of kept) awaiting.set(k, v);
  awaitingUntil = Date.now() + 10_000;
}

/** Whether what a kept element shows has arrived from the Mac. */
function loadedFor(key: string): boolean {
  if (key === "home") return state.threadsLoaded;
  if (key === "inbox") return state.loaded;
  return !!state.review && state.review.revision !== null;
}

/** Put `el` back where `was` says. Without `wait`, as near as it can;
 *  with it, only once the row it names, or enough of the list, is there. */
function scrollBack(el: HTMLElement, was: { top: number; anchor: string | null; offset: number }, wait: boolean): boolean {
  const a = was.anchor ? el.querySelector<HTMLElement>(`[data-anchor="${CSS.escape(was.anchor)}"]`) : null;
  if (a) { el.scrollTop += a.getBoundingClientRect().top - el.getBoundingClientRect().top - was.offset; return true; }
  if (wait && (was.anchor || el.scrollHeight - el.clientHeight < was.top)) return false;
  el.scrollTop = was.top;
  return true;
}

function restore(focus: ReturnType<typeof capture>): void {
  if (awaiting.size && Date.now() > awaitingUntil) awaiting.clear();
  for (const el of document.querySelectorAll<HTMLElement>("[data-keep]")) {
    const key = el.dataset.keep!;
    const later = awaiting.get(key);
    if (later) {
      // Waits for the row only while the list is still loading; once it
      // is there and the row is not (answered, folded away), as near as
      // it can, so the list is not held at the top.
      if (scrollBack(el, later, !loadedFor(key))) awaiting.delete(key);
      continue;
    }
    const was = scrolls.get(key);
    if (was) scrollBack(el, was, false);
  }
  if (focus) {
    const box = document.querySelector<HTMLTextAreaElement | HTMLInputElement>(`[data-keep="${CSS.escape(focus.key)}"]`);
    if (box) {
      // Within the same task as the redraw, so the keyboard stays up.
      box.focus({ preventScroll: true });
      if (focus.start !== null && focus.end !== null) box.setSelectionRange(focus.start, focus.end);
    }
  }
  followChat();
}

/** The latest message in each conversation on screen, as last drawn. */
const chatSeen = new Map<string, number>();

/** When a message lands in the conversation on screen, bring it into
 *  view; otherwise leave the scroll where the reader put it. */
function followChat(): void {
  for (const chat of document.querySelectorAll<HTMLElement>("[data-chat]")) {
    const key = chat.dataset.chat!;
    const n = Number(chat.dataset.last ?? 0);
    const before = chatSeen.get(key);
    chatSeen.set(key, n);
    if (before !== undefined && n > before) chat.lastElementChild?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }
}

/** Typing, as of the last keystroke. A live update that would redraw
 *  under someone mid-sentence waits until they pause. */
let lastKey = 0;
let composing = false;
document.addEventListener("input", () => { lastKey = Date.now(); }, true);
// A keyboard mid-word (Gboard's underlined suggestion) is typing however
// long it sits: a redraw then would break the word.
document.addEventListener("compositionstart", () => { composing = true; }, true);
document.addEventListener("compositionend", () => { composing = false; lastKey = Date.now(); }, true);
let deferred: number | null = null;

/** Redraw for something that changed elsewhere, not for something the
 *  reader did: held while they type, then drawn once. */
function renderLive(): void {
  const typing = document.activeElement instanceof HTMLTextAreaElement && (composing || Date.now() - lastKey < 1500);
  if (!typing) { render(); return; }
  if (deferred === null) deferred = window.setTimeout(() => { deferred = null; renderLive(); }, 1500);
}

// --- Where the reader is -------------------------------------------------------
//
// In the address, so a reload, a cold start after Android reclaimed the
// app, or a tap on a notification lands where the reader was rather than
// on the home screen. In history, so Back steps back through what was
// opened rather than leaving the app. Opening a review or a sheet is a
// step; turning a page or switching a tab rewrites the current one.

interface Place { screen: "threads" | "inbox" | "review"; review: string | null; page: number; tab: "read" | "threads"; thread: number | null }

/** What history keeps per entry, beyond the address. */
interface Kept {
  depth?: number;
  /** The tab a review was opened from, which "back" names. */
  home?: "threads" | "inbox";
  /** This entry is an open sheet, so closing it is a step back. */
  sheet?: boolean;
  /** The home screen as left: which cards and sections were open. */
  ui?: { open: string[]; expanded: string[]; showWaiting: boolean; showSeen: boolean; project: string | null };
  scrolls?: [string, { top: number; anchor: string | null; offset: number }][];
}

function hs(): Kept { return (history.state ?? {}) as Kept; }

function placeOf(url: Location): Place {
  const q = new URLSearchParams(url.search);
  const review = q.get("review");
  const n = (k: string) => { const v = Number(q.get(k)); return Number.isInteger(v) && v > 0 ? v : null; };
  return { screen: review ? "review" : q.has("reviews") ? "inbox" : "threads", review, page: n("page") ?? 0, tab: q.get("tab") === "threads" ? "threads" : "read", thread: n("thread") };
}

function hereNow(): Place {
  const r = state.screen === "review" ? state.review : null;
  return { screen: r ? "review" : state.screen === "inbox" ? "inbox" : "threads", review: r?.name ?? null, page: r?.page ?? 0, tab: state.tab, thread: state.sheet?.kind === "thread" ? state.sheet.root : null };
}

function urlOf(p: Place): string {
  if (p.screen === "review" && p.review) {
    const q = new URLSearchParams({ review: p.review });
    if (p.page) q.set("page", String(p.page));
    if (p.tab === "threads") q.set("tab", "threads");
    if (p.thread) q.set("thread", String(p.thread));
    return `/?${q}`;
  }
  return p.screen === "inbox" ? "/?reviews" : "/";
}

/** The home screen's open cards and sections, as history keeps them. */
function uiNow(): NonNullable<Kept["ui"]> {
  return { open: [...state.open], expanded: [...state.expanded], showWaiting: state.showWaiting, showSeen: state.showSeen, project: state.project };
}

function adoptUi(ui: Kept["ui"]): void {
  if (!ui) return;
  state.open = new Set(ui.open);
  state.expanded = new Set(ui.expanded);
  state.showWaiting = ui.showWaiting;
  state.showSeen = ui.showSeen;
  state.project = ui.project;
}

/** A new entry's state: one step deeper, opened from the current tab,
 *  carrying the home screen as it is, so a reload there still knows it. */
function fresh(): Kept { return { depth: (hs().depth ?? 0) + 1, home: state.home, ui: uiNow() }; }

/** Rewrite the current entry to where the reader now is. */
function record(): void {
  if (state.screen !== "threads" && state.screen !== "inbox" && state.screen !== "review") return;
  const url = urlOf(hereNow());
  try {
    history.replaceState({ ...hs(), home: state.home, sheet: hs().sheet && !!state.sheet }, "", url);
  } catch { /* throttled: the next step records it */ }
}

/** Keep, in the entry being left, what it looked like: scroll, and the
 *  home screen's open cards. What a reload or Back restores. */
function remember(): void {
  capture();
  // What is on screen now, over what this entry showed before (a tab
  // switch rewrites an entry, and Back may land on either tab's list).
  const live = new Set([...document.querySelectorAll<HTMLElement>("[data-keep]")].map((el) => el.dataset.keep!));
  const kept = new Map(hs().scrolls ?? []);
  for (const [k, v] of scrolls) if (live.has(k)) kept.set(k, v);
  try {
    history.replaceState({ ...hs(), ui: uiNow(), scrolls: [...kept] }, "");
  } catch { /* throttled */ }
}

/** Open a review as a step: Back returns to the list it was opened from. */
async function go(home: "threads" | "inbox", review: string, opts: OpenAt = {}): Promise<void> {
  moved();
  remember();
  state.home = home;
  state.tab = "read";
  state.sheet = null;
  history.pushState(fresh(), "", urlOf({ screen: "review", review, page: 0, tab: "read", thread: null }));
  await openReview(review, opts);
}

/** The back arrow: a step back when there is one; else, as when a
 *  notification opened the app straight into a review, home. */
function back(): void {
  if ((hs().depth ?? 0) > 0) history.back();
  else goHome(state.home);
}

/** Show a place: on Back and Forward, and on arriving at an address. */
/** Bumped by every move the reader makes, so an arrival still loading
 *  when they move again does not land on top of where they went. */
let moves = 0;
function moved(): number { return ++moves; }

async function arrive(p: Place): Promise<void> {
  const move = moved();
  state.home = hs().home ?? state.home;
  // Where this entry was left and how it looked, which may be from
  // before a reload.
  if (hs().scrolls) awaitScrolls(hs().scrolls!);
  adoptUi(hs().ui);
  if (p.screen !== "review" || !p.review) {
    state.sheet = null;
    state.screen = p.screen;
    state.review = null;
    render();
    void (p.screen === "inbox" ? loadInbox() : loadThreads());
    return;
  }
  state.tab = p.tab;
  // A sheet is open only on its own entry: Back from one closes it.
  const sheet: Sheet | null = p.thread ? { kind: "thread", root: p.thread } : hs().sheet ? state.sheet : null;
  if (state.review?.name !== p.review || state.screen !== "review") {
    state.sheet = null;
    state.screen = "review";
    await openReview(p.review, p.page || !p.thread ? { page: p.page } : { thread: p.thread });
  }
  // Moved on, or turned away (an unpaired phone): leave the screen as is.
  const r = state.review;
  if (move !== moves || state.screen !== "review" || !r || r.name !== p.review) return;
  state.sheet = sheet;
  render();
  if (!p.thread && p.page !== r.page && p.page < r.pages.length) goToPage(p.page);
  else restorePage();
  const el = document.querySelector<HTMLElement>(".sheet");
  if (el && sheet?.kind === "thread") el.scrollTop = el.scrollHeight;
  record();
}

window.addEventListener("popstate", () => void arrive(placeOf(location)));

// "2m ago" stays true while nothing else redraws the lists.
setInterval(() => {
  if (document.visibilityState === "visible" && !state.sheet && (state.screen === "threads" || state.screen === "inbox")) renderLive();
}, 60_000);

// A notification tapped while the app is open: go there as a step, so
// Back returns to whatever was open before.
navigator.serviceWorker?.addEventListener("message", (e) => {
  if (e.data?.type !== "open" || typeof e.data.url !== "string") return;
  e.ports[0]?.postMessage("heard");
  if (!api.credential() || state.screen === "pair" || state.screen === "moved" || state.screen === "unpaired") return;
  const u = new URL(e.data.url, location.origin);
  if (u.origin !== location.origin) return;
  if (u.pathname + u.search !== location.pathname + location.search) {
    remember();
    // A thread is a sheet over its review, as when opened by hand: two
    // steps, so closing it leaves the review and Back then leaves that.
    const q = new URLSearchParams(u.search);
    const review = q.get("review");
    if (review && q.get("thread")) {
      if (!(state.screen === "review" && state.review?.name === review)) {
        q.delete("thread");
        history.pushState(fresh(), "", `/?${q}`);
      }
      history.pushState({ ...fresh(), sheet: true }, "", u.pathname + u.search);
    } else {
      history.pushState(fresh(), "", u.pathname + u.search);
    }
  }
  void arrive(placeOf(location));
});

/** Kept as the reader goes, shortly after they stop: a page being
 *  reloaded or reclaimed is not reliably told so in time to save it. */
let rememberTimer: number | null = null;
function rememberSoon(): void {
  if (rememberTimer !== null) clearTimeout(rememberTimer);
  rememberTimer = window.setTimeout(() => { rememberTimer = null; remember(); }, 400);
}
document.addEventListener("scroll", rememberSoon, true);
document.addEventListener("visibilitychange", () => { if (document.visibilityState === "hidden") remember(); });
// Where it is told in time, as on a reload, also then.
window.addEventListener("pagehide", remember);

// --- Live updates -----------------------------------------------------------

let listening: AbortController | null = null;

const pending = new Set<"inbox" | "threads" | "review">();
let pendingTimer: number | null = null;

/** Refetch what is on screen shortly, once however many events ask. */
function soon(what: "inbox" | "threads" | "review"): void {
  pending.add(what);
  if (pendingTimer !== null) return;
  pendingTimer = window.setTimeout(() => {
    pendingTimer = null;
    const due = new Set(pending);
    pending.clear();
    if (due.has("review") && state.screen === "review") void refreshEvents(true);
    if (due.has("threads") && state.screen === "threads") void loadThreads(true);
    if (due.has("inbox") && state.screen === "inbox") void loadInbox(true);
  }, 300);
}

function startListening(): void {
  listening?.abort();
  listening = new AbortController();
  void api
    .listen(
      (p) => {
        // One write sends two events, and Claude writes in bursts: gather
        // them and fetch once.
        if (p.type === "inbox-changed") { soon("inbox"); soon("threads"); soon("review"); }
        else if (state.review && p.review === state.review.name) soon("review");
        else soon("threads");
      },
      (live) => { if (live !== state.live) { state.live = live; render(); } },
      listening.signal,
    )
    .catch(handle);
}

// --- Start --------------------------------------------------------------------

async function start(): Promise<void> {
  const token = new URLSearchParams(location.hash.slice(1)).get("pair");
  // Who is answering at this address, before anything else. A page whose
  // saved machine differs is about to talk to the wrong Mac.
  let here: Machine | null = null;
  try {
    here = await api.machine();
  } catch (e) {
    if (isApiError(e) && e.unreachable) state.live = false;
  }

  if (token) {
    state.live = here !== null;
    state.pairToken = token;
    state.machine = here ?? state.machine;
    state.screen = "pair";
    render();
    return;
  }
  if (here && state.machine && here.id !== state.machine.id) {
    api.forget();
    state.was = state.machine;
    state.machine = here;
    api.remember(here);
    state.screen = "moved";
    render();
    return;
  }
  if (here) { state.machine = here; api.remember(here); }
  if (!api.credential()) {
    state.screen = "unpaired";
    render();
    return;
  }
  // A reload, or Android bringing the app back after reclaiming it: the
  // address says where the reader was, and the entry what it looked like.
  const kept = hs();
  if (kept.depth === undefined) history.replaceState({ ...kept, depth: 0 }, "");
  adoptUi(kept.ui);
  if (kept.scrolls) awaitScrolls(kept.scrolls);
  const place = placeOf(location);
  if (place.screen === "inbox") state.screen = "inbox";
  render();
  startListening();
  const lists = Promise.all([loadInbox(), loadThreads()]);
  if (place.screen === "review") await arrive(place);
  else { state.screen = place.screen; await lists; }
  void loadPushState();
}

if ("serviceWorker" in navigator) void navigator.serviceWorker.register("/sw.js").catch(() => {});
window.addEventListener("resize", restorePage);

/** Back from sleep, or the network came back: reconnect now rather than
 *  when a backed-off retry gets round to it, and refresh what is on
 *  screen, which may have changed while the phone was away. */
function wake(): void {
  if (document.visibilityState !== "visible" || !api.credential() || state.screen === "pair" || state.screen === "moved") return;
  startListening();
  if (state.screen === "review" && state.review) void refreshEvents();
  else if (state.screen === "threads") void loadThreads();
  else void loadInbox();
}
document.addEventListener("visibilitychange", wake);
window.addEventListener("online", wake);
window.addEventListener("pageshow", (e) => { if ((e as PageTransitionEvent).persisted) wake(); });
// Scanning a pairing code while this page is already open only changes
// the fragment, which does not reload it.
window.addEventListener("hashchange", () => {
  if (location.hash.includes("pair=")) { listening?.abort(); void start(); }
});
void start();
