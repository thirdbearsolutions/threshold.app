// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import type { SessionEvent } from "./api";
import { currentDiagnostics, isRetired } from "./diagnostics.ts";

function ev(seq: number, o: Partial<SessionEvent>): SessionEvent {
  return { seq, at_ms: seq, from: "agent", revision: "main...HEAD", kind: "note", ...o };
}
const diag = (seq: number, code: string) => ev(seq, { from: "threshold", kind: "diagnostic", code, message: code });

test("a corrected bundle at the same revision retires the bundle finding", () => {
  const events = [ev(1, { kind: "brief" }), ev(2, { kind: "bundle" }), diag(3, "bundle_incomplete"), diag(4, "bundle_phantom_hunks"), ev(5, { kind: "bundle" })];
  assert.equal(isRetired(events, events[2]), true);
  assert.equal(isRetired(events, events[3]), true);
  assert.deepEqual(currentDiagnostics(events), []);
});

test("a finding about the corrected bundle stands", () => {
  const events = [ev(1, { kind: "bundle" }), diag(2, "bundle_incomplete"), ev(3, { kind: "bundle" }), diag(4, "bundle_incomplete")];
  assert.deepEqual(currentDiagnostics(events).map((e) => e.seq), [4]);
});

test("a new bundle does not retire a finding about the revision", () => {
  const events = [ev(1, { kind: "brief" }), diag(2, "review_unpinned"), ev(3, { kind: "bundle" })];
  assert.deepEqual(currentDiagnostics(events).map((e) => e.seq), [2]);
});

test("a revision retires every finding before it", () => {
  const events = [diag(1, "review_unpinned"), diag(2, "bundle_incomplete"), ev(3, { kind: "revised", to: "main...HEAD" }), diag(4, "diff_empty")];
  assert.deepEqual(currentDiagnostics(events).map((e) => e.seq), [4]);
});

test("only the agent replaces things", () => {
  // A reviewer's notes and Threshold's own lines never retire a finding.
  const events = [diag(1, "bundle_incomplete"), ev(2, { from: "human", kind: "note", text: "hm" }), ev(3, { from: "threshold", kind: "pinned" })];
  assert.deepEqual(currentDiagnostics(events).map((e) => e.seq), [1]);
});
