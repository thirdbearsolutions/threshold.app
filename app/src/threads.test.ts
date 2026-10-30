// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import type { SessionEvent } from "./api";
import { threadOf } from "./threads.ts";

function ev(seq: number, o: Partial<SessionEvent>): SessionEvent {
  return { seq, at_ms: seq, from: "agent", revision: null, kind: "note", ...o };
}

test("a reply to a reply is in the thread", () => {
  const events = [
    ev(1, { kind: "brief" }),
    ev(2, { from: "human", text: "why?" }),
    ev(3, { kind: "reply", in_reply_to: 2, text: "because" }),
    ev(4, { kind: "note", from: "human", in_reply_to: 3, text: "and then?" }),
    // Claude answering the reviewer's latest message, not the root.
    ev(5, { kind: "reply", in_reply_to: 4, text: "then this" }),
    ev(6, { from: "human", text: "another thread" }),
    ev(7, { kind: "reply", in_reply_to: 6, text: "elsewhere" }),
  ];
  assert.deepEqual(threadOf(events, 2).map((e) => e.seq), [2, 3, 4, 5]);
  assert.deepEqual(threadOf(events, 6).map((e) => e.seq), [6, 7]);
});

test("a mark or verdict pointing nowhere joins nothing", () => {
  const events = [ev(2, { from: "human", text: "q" }), ev(3, { kind: "verdict", in_reply_to: 2 })];
  assert.deepEqual(threadOf(events, 2).map((e) => e.seq), [2]);
});
