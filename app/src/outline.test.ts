// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import type { DiffFile, DiffLine } from "./api.ts";
import { describeHunk, fileRun, runsOf } from "./outline.ts";

const lines = (added: number, removed: number, context = 1): DiffLine[] => [
  ...Array.from({ length: context }, () => ({ kind: "context" as const, content: "" })),
  ...Array.from({ length: removed }, () => ({ kind: "removed" as const, content: "" })),
  ...Array.from({ length: added }, () => ({ kind: "added" as const, content: "" })),
];

function file(path: string, ...hunks: [string, number, number][]): DiffFile {
  return {
    old_path: path,
    new_path: path,
    status: "modified",
    hunks: hunks.map(([header, a, r]) => ({ header, lines: lines(a, r) })),
    additions: hunks.reduce((n, [, a]) => n + a, 0),
    deletions: hunks.reduce((n, [, , r]) => n + r, 0),
  };
}

const a = file("src/a.rs", ["@@ -1,3 +1,4 @@", 1, 0], ["@@ -40,5 +41,9 @@ fn render()", 5, 1], ["@@ -90,2 +95,2 @@", 1, 1]);
const b = file("src/b.ts", ["@@ -10,4 +10,6 @@ export function go", 2, 0]);

test("a hunk is labelled by its new-side lines and git's section heading", () => {
  assert.deepEqual(describeHunk("@@ -40,5 +41,9 @@ fn render()"), { lines: "L41–49", context: "fn render()" });
  assert.deepEqual(describeHunk("@@ -4 +4 @@"), { lines: "L4", context: "" });
});

test("a hunk that only removes says where it was", () => {
  assert.deepEqual(describeHunk("@@ -12,4 +11,0 @@ impl Foo"), { lines: "was L12–15", context: "impl Foo" });
});

test("consecutive hunks of one file are one run, in the chapter's order", () => {
  const runs = runsOf(
    [
      { file: "src/a.rs", header: "@@ -40,5 +41,9 @@ fn render()" },
      { file: "src/a.rs", header: "@@ -1,3 +1,4 @@" },
      { file: "src/b.ts", header: "@@ -10,4 +10,6 @@ export function go" },
    ],
    [a, b],
  );
  assert.deepEqual(runs.map((r) => [r.path, r.hunks.map((x) => x.index)]), [["src/a.rs", [1, 0]], ["src/b.ts", [0]]]);
  assert.deepEqual(runs.map((r) => [r.added, r.removed]), [[6, 1], [2, 0]]);
  assert.equal(runs[0].hunks[0].key, "src/a.rs#1");
});

test("a file the chapter returns to is a second run, not merged into the first", () => {
  const runs = runsOf(
    [
      { file: "src/a.rs", header: "@@ -1,3 +1,4 @@" },
      { file: "src/b.ts", header: "@@ -10,4 +10,6 @@ export function go" },
      { file: "src/a.rs", header: "@@ -90,2 +95,2 @@" },
    ],
    [a, b],
  );
  assert.deepEqual(runs.map((r) => r.path), ["src/a.rs", "src/b.ts", "src/a.rs"]);
});

test("a reference to a hunk that is not in the diff is left out", () => {
  const runs = runsOf([{ file: "src/a.rs", header: "@@ -7,7 +7,7 @@ gone" }, { file: "nope.rs", header: "@@ -1 +1 @@" }], [a, b]);
  assert.deepEqual(runs, []);
});

test("a whole file is one run of every hunk", () => {
  const r = fileRun(a);
  assert.deepEqual(r.hunks.map((x) => x.lines), ["L1–4", "L41–49", "L95–96"]);
  assert.deepEqual([r.added, r.removed], [7, 2]);
});
