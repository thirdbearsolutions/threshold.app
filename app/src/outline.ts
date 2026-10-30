// The shape of a page below the chapter: which files it walks through, and
// the hunks of each.
//
// A chapter lists hunks in the order the agent wants them read, and that
// order crosses files. The outline follows it rather than re-sorting: a
// run is a stretch of consecutive hunks from one file, and a file the
// chapter returns to later is a second run. The navigator and the page
// are then the same sequence, so a row always points at the place that
// sits in that position on the page.
//
// Free of the DOM so it is tested on its own.

import type { DiffFile, HunkRef } from "./api";

export interface HunkEntry {
  /** Index of the hunk within its file. */
  index: number;
  /** Addresses the hunk on the page: `path#index`. */
  key: string;
  /** Where it is, e.g. "L120–148", or "was L40–52" when it only removes. */
  lines: string;
  /** git's section heading after the `@@`, e.g. "fn render()". */
  context: string;
  added: number;
  removed: number;
}

export interface FileRun {
  path: string;
  file: DiffFile;
  hunks: HunkEntry[];
  added: number;
  removed: number;
}

export function pathOf(file: DiffFile): string {
  return file.new_path ?? file.old_path ?? "";
}

export function hunkKey(path: string, index: number): string {
  return `${path}#${index}`;
}

function range(start: number, count: number): string {
  return count <= 1 ? `L${start}` : `L${start}–${start + count - 1}`;
}

/** A hunk's place in its file and its section heading. */
export function describeHunk(header: string): { lines: string; context: string } {
  // A binary file's one hunk is git's "Binary files … differ" line.
  if (header.startsWith("Binary files ")) return { lines: "binary", context: "" };
  const m = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/.exec(header);
  if (!m) return { lines: "", context: header };
  // git omits a count of one.
  const count = (c: string | undefined) => (c == null ? 1 : Number(c));
  const context = m[5].trim();
  if (count(m[4]) > 0) return { lines: range(Number(m[3]), count(m[4])), context };
  // Nothing left on the new side, so say where it was.
  return { lines: `was ${range(Number(m[1]), count(m[2]))}`, context };
}

export function entryOf(file: DiffFile, index: number): HunkEntry {
  const hunk = file.hunks[index];
  let added = 0;
  let removed = 0;
  for (const l of hunk.lines) {
    if (l.kind === "added") added++;
    else if (l.kind === "removed") removed++;
  }
  return { index, key: hunkKey(pathOf(file), index), ...describeHunk(hunk.header), added, removed };
}

function run(file: DiffFile, hunks: HunkEntry[]): FileRun {
  return {
    path: pathOf(file),
    file,
    hunks,
    added: hunks.reduce((n, x) => n + x.added, 0),
    removed: hunks.reduce((n, x) => n + x.removed, 0),
  };
}

/** A chapter's hunks as runs of one file each, in the chapter's order.
 *  A reference to a hunk not in the diff is left out, as the page does. */
export function runsOf(refs: HunkRef[], files: DiffFile[]): FileRun[] {
  const out: FileRun[] = [];
  for (const ref of refs) {
    const file = files.find((f) => pathOf(f) === ref.file);
    const i = file ? file.hunks.findIndex((hk) => hk.header === ref.header) : -1;
    if (!file || i < 0) continue;
    const last = out[out.length - 1];
    if (last && last.file === file) {
      last.hunks.push(entryOf(file, i));
      last.added += last.hunks[last.hunks.length - 1].added;
      last.removed += last.hunks[last.hunks.length - 1].removed;
    } else {
      out.push(run(file, [entryOf(file, i)]));
    }
  }
  return out;
}

/** A whole file as one run: the Files view's page. */
export function fileRun(file: DiffFile): FileRun {
  return run(file, file.hunks.map((_, i) => entryOf(file, i)));
}
