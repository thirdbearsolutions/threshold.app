// Stands in for the Tauri backend so the desktop UI can be opened in a
// browser (npm run dev:preview), against preview/fixture.ts. For design work:
// nothing is written anywhere, and every command answers from the fixture.

import { chapters, events, files, REPO, REVIEW, reviews } from "./fixture";

const path = (f: { new_path: string | null; old_path: string | null }) => f.new_path ?? f.old_path ?? "";
const allHunks = files.flatMap((f) => f.hunks.map((h) => ({ file: path(f), header: h.header })));
const summary = {
  files_changed: files.length,
  lines_added: files.reduce((n, f) => n + f.additions, 0),
  lines_removed: files.reduce((n, f) => n + f.deletions, 0),
  has_deletions: false, deleted_files: [],
  touches_tests: true, test_files: ["tests/test_covers.py"],
};
const notes = events.filter((e) => e.kind === "note" && e.file);
// ?empty answers as a fresh install would: no reviews, nothing open,
// nowhere saved. Add ?theme=first to see the picker over it.
const empty = new URLSearchParams(location.search).has("empty");
const fresh: Record<string, (a: any) => unknown> = {
  has_session: () => false,
  active_review: () => null,
  session_events: () => [],
  review_repo: () => null,
  list_reviews: () => [],
  review_sizes: () => ({}),
  load_place: () => ({ review: null, scroll: {}, chapter: {}, mode: {}, read: {}, ui: {} }),
};
let skillState = "missing";
// Once the tour has made its sample, the inbox is no longer empty.
let sampled = false;
const handlers: Record<string, (a: any) => unknown> = {
  has_session: () => true,
  active_review: () => REVIEW,
  session_events: () => events,
  open_review: () => events,
  review_repo: () => REPO,
  initial_repo: () => null,
  initial_revision: () => null,
  list_reviews: () => reviews,
  review_sizes: () => Object.fromEntries(reviews.filter((r) => !r.decided).map((r) => [r.path, { files: 5, added: summary.lines_added, removed: summary.lines_removed }])),
  load_place: () => ({ review: REVIEW, scroll: {}, chapter: {}, mode: {}, read: {}, ui: {} }),
  save_place: () => null,
  load_diff: () => ({ files, summary, source: "main...HEAD", pin: { base: "3f9e1c2b7a", head: "a71d0b4e19" }, digest: "9c1d" }),
  load_bundle: () => ({ chapters, coverage: { claimed: allHunks.length, total: allHunks.length, unbundled: [], phantom: [] } }),
  chapter_states: (a) => (a.chapters as string[]).map((_, i) => {
    const fs = new Set(chapters[i]?.hunks.map((h) => h.file));
    const on = notes.filter((n) => fs.has(n.file));
    return { mark: i === 0 ? "ok" : null, notes: on.filter((n) => !n.flag).length, unanswered: 0,
      attention: on.filter((n) => n.flag === "attention").length, caution: on.filter((n) => n.flag === "caution").length, context: on.filter((n) => n.flag === "context").length };
  }),
  line_anchors: () => notes.map((n) => ({ seq: n.seq, file: n.file, line: n.line, status: "current", in_diff: true })),
  list_decisions: () => [],
  file_text: () => "",
  update_ready: () => null,
  phone_status: () => ({ enabled: false, machine: null, url: null, devices: [] }),
  native_notifications: () => false,
  // First run: the sample is the fixture's review, and nothing is installed.
  skill_status: () => skillState,
  install_skill: () => (skillState = "installed"),
  reveal_skill: () => null,
  start_sample: () => { sampled = true; return REVIEW; },
  set_archived: () => reviews,
};

let nextId = 1;
(window as any).__TAURI_INTERNALS__ = {
  transformCallback: () => nextId++,
  unregisterCallback: () => {},
  convertFileSrc: (p: string) => p,
  metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
  invoke: async (cmd: string, args: any) => {
    if (cmd.startsWith("plugin:")) return cmd.endsWith("is_permission_granted") ? true : null;
    const h = (empty && !sampled && fresh[cmd]) || handlers[cmd];
    if (!h) { console.warn("preview: no stub for", cmd, args); return null; }
    return structuredClone(h(args ?? {}));
  },
};
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
