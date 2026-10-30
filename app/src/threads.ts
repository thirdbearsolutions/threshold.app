// Which messages make up a conversation. Shared by the phone and its
// tests; the desktop and the server group threads the same way.

import type { SessionEvent } from "./api";

/** Every message in the thread rooted at `root`, root first.
 *
 *  The whole chain, not only direct answers to the root: Claude often
 *  answers the reviewer's latest message rather than the first one, and a
 *  reply to a reply is still in the conversation. Events are append-only
 *  and a reply follows what it answers, so one forward pass closes it. */
export function threadOf(events: SessionEvent[], root: number): SessionEvent[] {
  const ids = new Set<number>([root]);
  for (const e of events) {
    if ((e.kind === "note" || e.kind === "reply") && e.in_reply_to != null && ids.has(e.in_reply_to)) ids.add(e.seq);
  }
  return events.filter((e) => ids.has(e.seq));
}
