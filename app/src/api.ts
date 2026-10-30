import { invoke } from "@tauri-apps/api/core";

export type LineKind = "context" | "added" | "removed";
export type FileStatus = "added" | "deleted" | "modified" | "renamed";
export type Verdict = "approved" | "rejected";

export interface DiffLine {
  kind: LineKind;
  content: string;
}

export interface Hunk {
  header: string;
  lines: DiffLine[];
}

export interface DiffFile {
  old_path: string | null;
  new_path: string | null;
  status: FileStatus;
  hunks: Hunk[];
  additions: number;
  deletions: number;
  /** Binary: no lines, and one hunk whose header is git's "Binary files
   *  … differ" line, so a chapter can still place it. */
  binary?: boolean;
}

export interface DiffFacts {
  files_changed: number;
  lines_added: number;
  lines_removed: number;
  has_deletions: boolean;
  deleted_files: string[];
  touches_tests: boolean;
  test_files: string[];
}

/** The two commits a review is pinned to. */
export interface Pin {
  base: string;
  head: string;
}

export interface DiffPayload {
  files: DiffFile[];
  summary: DiffFacts;
  /** null = working tree, "--cached" = index, else the revision spec. */
  source: string | null;
  /** The commits this diff was taken between. null means the working tree
   *  is one side of it, so what is on screen can change while it is read. */
  pin: Pin | null;
  /** SHA-256 of the diff text on screen; quoted by the verdict. */
  digest: string;
}

export interface Decision {
  summary: DiffFacts;
  verdict: Verdict;
  reason: string | null;
  decided_at_ms: number;
  /** What was reviewed. Absent on decisions logged before revisions existed. */
  source?: string | null;
  /** SHA-256 of the diff that was read. Absent before pinning existed. */
  digest?: string | null;
}

export type Author = "agent" | "human" | "threshold";

/** One entry in the review conversation. `kind` tags the variant. */
export interface SessionEvent {
  seq: number;
  at_ms: number;
  from: Author;
  revision: string | null;
  kind: "brief" | "note" | "reply" | "revised" | "verdict" | "bundle" | "diagnostic" | "pinned";
  // brief
  title?: string;
  summary?: string;
  implications?: string[];
  // note / reply
  text?: string;
  file?: string | null;
  line?: number | null;
  line_content?: string | null;
  in_reply_to?: number | null;
  chapter?: string | null;
  mark?: Mark;
  /** Set when Claude flagged this line while pushing the review. */
  flag?: Flag | null;
  // revised
  to?: string;
  note?: string | null;
  // pinned
  base?: string;
  head?: string;
  /** On a `pinned` event, the diff it produced; on a `verdict`, the diff
   *  the human actually read. */
  digest?: string | null;
  // diagnostic
  code?: string;
  message?: string;
  detail?: string[];
  // verdict
  verdict?: Verdict;
  reason?: string | null;
}

export interface HunkRef { file: string; header: string }
export interface Chapter { title: string; summary: string; hunks: HunkRef[] }
export interface Coverage {
  claimed: number;
  total: number;
  /** In the diff, claimed by no chapter. Must be shown, never hidden. */
  unbundled: HunkRef[];
  /** Claimed but absent — the narrative describes another revision. */
  phantom: HunkRef[];
}
export interface BundlePayload { chapters: Chapter[]; coverage: Coverage }

export const loadBundle = (path: string, staged: boolean, revision: string | null) =>
  invoke<BundlePayload | null>("load_bundle", { path, staged, revision });

export interface ReviewSummary {
  path: string;
  title: string;
  repo: string | null;
  revision: string | null;
  decided: boolean;
  last_at_ms: number;
  events: number;
  /** Events from the agent or the human; Threshold's own are left out. */
  spoken: number;
  /** Put away without a verdict; back in the inbox once anything new is said. */
  archived: boolean;
}

/** Reviews on disk. Independent of whether the agent that wrote them is
 *  still running — the list comes from the inbox directory, nothing else. */
export interface Place {
  review: string | null;
  scroll: Record<string, Record<string, number>>;
  chapter: Record<string, number>;
  mode: Record<string, string>;
  /** Per review, per thread root seq: the highest seq the reviewer has seen. */
  read: Record<string, Record<string, number>>;
  /** How the review list is arranged. */
  inbox_group?: "date" | "project" | null;
  /** The one project (repo path) the list is narrowed to, if any. */
  inbox_project?: string | null;
  /** Markdown files shown as a diff rather than rendered, by
   *  `review|path`. Absent means rendered. */
  markdown?: Record<string, "diff">;
  /** How the window was left, beyond the fields above. See UiPlace. */
  ui?: UiPlace;
}
/** Which screen, panes and toggles were showing when the app last drew.
 *  Saved whenever one changes and put back on launch. */
export interface UiPlace {
  view?: "diff" | "history" | "phone";
  /** The review list beside the review. */
  show_inbox?: boolean;
  deleted_open?: boolean;
  /** A repo opened by hand rather than a review, with how it was diffed. */
  folder?: { repo: string; staged: boolean; revision: string } | null;
  /** Per review: the file selected in the files view, by path. */
  file?: Record<string, string>;
  /** Per review: the thread open in the chat pane, by root seq. */
  thread?: Record<string, number>;
}
export const loadPlace = () => invoke<Place>("load_place");
export const savePlace = (place: Place) => invoke<void>("save_place", { place });

/** Where a line thread sits in the diff on screen, carried from the
 *  commits it was written against. */
export interface LineAnchor {
  seq: number;
  file: string;
  line: number;
  status: "current" | "moved" | "outdated";
  in_diff: boolean;
}
export const lineAnchors = () => invoke<LineAnchor[]>("line_anchors");

/** Where a click on a notification leads. */
export interface NotificationTarget { review: string | null; thread: number | null }
export const nativeNotifications = () => invoke<boolean>("native_notifications");
/** False when native notifications are unavailable (an unbundled dev
 *  build, or permission denied), so the caller can say it another way. */
export const notify = (title: string, subtitle: string | null, body: string, review: string | null, thread: number | null) =>
  invoke<boolean>("notify", { title, subtitle, body, review, thread });

export const listReviews = () => invoke<ReviewSummary[]>("list_reviews");
/** How big a diff is: what a reviewer budgets time by. */
export interface DiffSize { files: number; added: number; removed: number }
/** Sizes of the reviews still waiting, by path. Slower than the list, so
 *  fetched after it. */
export const reviewSizes = () => invoke<Record<string, DiffSize>>("review_sizes");
/** Archive a review without deciding it, or bring it back. Returns the list. */
export const setArchived = (path: string, archived: boolean) => invoke<ReviewSummary[]>("set_archived", { path, archived });
export const openReview = (path: string) => invoke<SessionEvent[]>("open_review", { path });
export const reviewRepo = () => invoke<string | null>("review_repo");

export type Mark = "ok" | "concern" | "clear";
export type Flag = "attention" | "caution" | "context";
export interface ChapterState {
  mark: Mark | null;
  notes: number;
  unanswered: number;
  attention: number;
  caution: number;
  context: number;
}

export const markChapter = (chapter: string, mark: Mark) =>
  invoke<SessionEvent[]>("mark_chapter", { chapter, mark });
export const chapterStates = (chapters: string[], files: string[][]) =>
  invoke<ChapterState[]>("chapter_states", { chapters, files });

export const sessionEvents = () => invoke<SessionEvent[]>("session_events");
export const hasSession = () => invoke<boolean>("has_session");
export const activeReview = () => invoke<string | null>("active_review");
export const postNote = (
  text: string,
  file: string | null,
  line: number | null,
  lineContent: string | null,
  chapter: string | null = null,
  inReplyTo: number | null = null,
) => invoke<SessionEvent[]>("post_note", { text, file, line, lineContent, chapter, inReplyTo });

export const initialRepo = () => invoke<string | null>("initial_repo");
export const initialRevision = () => invoke<string | null>("initial_revision");
export const pickRepoFolder = () => invoke<string | null>("pick_repo_folder");
export const loadDiff = (path: string, staged: boolean, revision: string | null) => invoke<DiffPayload>("load_diff", { path, staged, revision });
export const listDecisions = () => invoke<Decision[]>("list_decisions");
/** The new side of a file as the diff on screen sees it, one entry per line. */
export const fileText = (path: string, staged: boolean, revision: string | null, file: string) =>
  invoke<string[]>("file_text", { path, staged, revision, file });
export const decide = (summary: DiffFacts, verdict: Verdict, reason: string | null, source: string | null, digest: string | null) =>
  invoke<Decision[]>("decide", { summary, verdict, reason, source, digest });

/** Which Mac this is, as every phone screen shows it. */
export interface Machine { id: string; name: string; color: string }
export interface PairedDevice { id: string; name: string; paired_at_ms: number; last_seen_ms: number }
export interface PhoneStatus { enabled: boolean; machine: Machine | null; url: string | null; devices: PairedDevice[] }
export interface Pairing { url: string; svg: string; expires_in_ms: number }
export const phoneStatus = () => invoke<PhoneStatus>("phone_status");
export const phonePairing = () => invoke<Pairing>("phone_pairing");
export const phoneRevoke = (id: string) => invoke<PairedDevice[]>("phone_revoke", { id });

/** An image from one side of the diff on screen, as a data: URL. */
export const fileImage = (path: string, staged: boolean, revision: string | null, file: string, old: boolean) =>
  invoke<string>("file_image", { path, staged, revision, file, old });

/** A newer build waiting beside this one (scripts/auto-install.sh). */
export interface Update { commit: string; subject: string }
export const updateReady = () => invoke<Update | null>("update_ready");
export const restartForUpdate = () => invoke<void>("restart_for_update");

/** Whether Claude Code has the review skill this build ships. */
export type SkillStatus = "missing" | "installed" | "different";
export const skillStatus = () => invoke<SkillStatus>("skill_status");
/** Copy the skill into ~/.claude/skills. A different copy already there
 *  is only replaced when `replace` is set. */
export const installSkill = (replace: boolean) => invoke<SkillStatus>("install_skill", { replace });
/** Show the skill file in Finder. */
export const revealSkill = () => invoke<void>("reveal_skill");
/** Build the tour's sample review; returns its path in the inbox. */
export const startSample = () => invoke<string>("start_sample");
/** The tour's sample review, by its file name. */
export const isSample = (path: string | null | undefined) => !!path && path.endsWith("-sample-tour.jsonl");
