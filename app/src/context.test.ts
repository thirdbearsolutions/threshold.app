// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { gapsAround, layout, open, spanOf, STEP } from "./context.ts";

test("a header gives its new-file span, with git's omitted count of one", () => {
  assert.deepEqual(spanOf("@@ -10,7 +12,9 @@ fn main()"), { start: 12, count: 9 });
  assert.deepEqual(spanOf("@@ -4 +4 @@"), { start: 4, count: 1 });
  assert.deepEqual(spanOf("@@ -4,2 +3,0 @@"), { start: 3, count: 0 });
});

test("the gaps are before, between and after the hunks", () => {
  const gaps = gapsAround([spanOf("@@ -1,1 +10,5 @@"), spanOf("@@ -1,1 +40,3 @@")], 100);
  assert.deepEqual(gaps, [
    { first: 1, last: 9 },
    { first: 15, last: 39 },
    { first: 43, last: 100 },
  ]);
});

test("adjacent hunks leave an empty gap, and a pure deletion sits after its line", () => {
  const gaps = gapsAround([spanOf("@@ -1 +1,3 @@"), spanOf("@@ -9,2 +4,0 @@"), spanOf("@@ -20 +5,2 @@")], null);
  assert.deepEqual(gaps[0], { first: 1, last: 0 }, "nothing above a hunk starting at line 1");
  assert.deepEqual(gaps[1], { first: 4, last: 4 }, "line 4 is before the deletion point");
  assert.deepEqual(gaps[2], { first: 5, last: 4 }, "and nothing between the deletion and the next hunk");
  assert.deepEqual(gaps[3], { first: 7, last: null }, "the end of the file is not known yet");
});

test("opening from either edge shows those lines and hides the rest", () => {
  const gap = { first: 15, last: 64 }; // 50 lines
  let o = open(gap, { top: 0, bottom: 0 }, "top");
  assert.deepEqual(layout(gap, o), { top: [15, 34], hidden: 30, bottom: null });
  o = open(gap, o, "bottom");
  assert.deepEqual(layout(gap, o), { top: [15, 34], hidden: 10, bottom: [45, 64] });
});

test("opening more never moves what is already shown, and the gap closes when the edges meet", () => {
  const gap = { first: 15, last: 64 };
  let o = open(gap, { top: 0, bottom: 0 }, "top");
  o = open(gap, o, "bottom");
  const before = layout(gap, o);
  o = open(gap, o, "top");
  const after = layout(gap, o);
  assert.deepEqual(after.bottom, before.bottom, "the bottom edge's lines are untouched");
  assert.equal(after.hidden, 0);
  assert.equal((after.top![1] - after.top![0] + 1) + (after.bottom![1] - after.bottom![0] + 1), 50, "every line shown once");
  assert.deepEqual(open(gap, o, "top"), o, "a closed gap does not grow");
});

test("expand all opens the whole gap", () => {
  const gap = { first: 1, last: 9 };
  assert.deepEqual(layout(gap, open(gap, { top: 0, bottom: 0 }, "all")), { top: [1, 9], hidden: 0, bottom: null });
});

test("below the last hunk, only the top edge opens until the file's end is known", () => {
  const gap = { first: 43, last: null };
  const o = open(gap, { top: 0, bottom: 0 }, "top");
  assert.deepEqual(layout(gap, o), { top: [43, 42 + STEP], hidden: null, bottom: null });
  // The file turns out shorter than a step: nothing past its end is shown.
  assert.deepEqual(layout({ first: 43, last: 50 }, o), { top: [43, 50], hidden: 0, bottom: null });
});
