// The scroll invariant, tested without a browser: a region is a plain
// object with the four properties ScrollMemory reads, and "rendering" is
// what render() does to one — throw it away and build a new one at the top.
//
// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { ScrollMemory, type Scrollable } from "./scroll.ts";

function region(key: string, height = 5000, client = 800, stick?: "bottom"): Scrollable {
  let top = 0;
  return {
    dataset: { scroll: key, stick },
    get scrollHeight() { return height; },
    get clientHeight() { return client; },
    // What a browser does: clamp to the scrollable range.
    get scrollTop() { return top; },
    set scrollTop(v: number) { top = Math.max(0, Math.min(v, Math.max(0, height - client))); },
  };
}

/** One render: capture the live regions, replace them, restore the new ones. */
function render(mem: ScrollMemory, live: Scrollable[], next: Scrollable[]): Scrollable[] {
  mem.capture(live);
  mem.restore(next);
  return next;
}

test("a rebuild keeps a region where the reader left it", () => {
  const mem = new ScrollMemory();
  const pane = region("r1|narrative:2");
  pane.scrollTop = 1830;
  mem.record(pane);
  const [after] = render(mem, [pane], [region("r1|narrative:2")]);
  assert.equal(after.scrollTop, 1830);
});

test("renders the reader did not cause — a reply, a toast — never move the page", () => {
  const mem = new ScrollMemory();
  let pane = region("r1|narrative:3");
  pane.scrollTop = 900;
  mem.record(pane);
  for (let i = 0; i < 20; i++) [pane] = render(mem, [pane], [region("r1|narrative:3")]);
  assert.equal(pane.scrollTop, 900);
});

test("opening a modal and closing it with Esc leaves the page exactly where it was", () => {
  // The modal adds a region of its own and then takes it away. Neither
  // render may touch the page under it.
  const mem = new ScrollMemory();
  let pane = region("r1|files:src/db.rs");
  pane.scrollTop = 2412;
  mem.record(pane);
  let [p, modal] = render(mem, [pane], [region("r1|files:src/db.rs"), region("r1|modal:7", 600, 400, "bottom")]);
  assert.equal(p.scrollTop, 2412);
  [pane] = render(mem, [p, modal], [region("r1|files:src/db.rs")]);
  assert.equal(pane.scrollTop, 2412);
});

test("scrolling after a render is remembered, not the position before it", () => {
  const mem = new ScrollMemory();
  let pane = region("r1|narrative:1");
  [pane] = render(mem, [], [pane]);
  pane.scrollTop = 640;
  mem.record(pane); // the scroll event
  [pane] = render(mem, [pane], [region("r1|narrative:1")]);
  assert.equal(pane.scrollTop, 640);
});

test("each page has its own position, and returning to one restores it", () => {
  const mem = new ScrollMemory();
  let pane = region("r1|narrative:0");
  pane.scrollTop = 1200;
  mem.record(pane);
  [pane] = render(mem, [pane], [region("r1|narrative:1")]);
  assert.equal(pane.scrollTop, 0, "a page never visited starts at the top, not at the last page's offset");
  pane.scrollTop = 300;
  mem.record(pane);
  [pane] = render(mem, [pane], [region("r1|narrative:0")]);
  assert.equal(pane.scrollTop, 1200);
});

test("the file view and the narrative do not share offsets", () => {
  // The bug this replaced: the files view saved its scroll under the
  // chapter index, so switching views landed mid-chapter.
  const mem = new ScrollMemory();
  let pane = region("r1|files:a.rs");
  pane.scrollTop = 3000;
  mem.record(pane);
  [pane] = render(mem, [pane], [region("r1|narrative:0")]);
  assert.equal(pane.scrollTop, 0);
});

test("an explicit landing wins exactly once", () => {
  const mem = new ScrollMemory();
  let pane = region("r1|narrative:4");
  pane.scrollTop = 100;
  mem.record(pane);
  mem.land("r1|narrative:4", 2200);
  assert.equal(mem.get("r1|narrative:4"), 2200);
  [pane] = render(mem, [], [region("r1|narrative:4")]);
  assert.equal(pane.scrollTop, 2200);
  pane.scrollTop = 2250;
  mem.record(pane);
  [pane] = render(mem, [pane], [region("r1|narrative:4")]);
  assert.equal(pane.scrollTop, 2250, "the landing is spent; the reader's own scroll governs again");
});

test("content briefly shorter does not cost the reader their place", () => {
  const mem = new ScrollMemory();
  let pane = region("r1|narrative:2");
  pane.scrollTop = 2600;
  mem.record(pane);
  // Reloading: the chapter renders short, and the browser clamps.
  [pane] = render(mem, [pane], [region("r1|narrative:2", 900, 800)]);
  assert.equal(pane.scrollTop, 100);
  mem.record(pane); // the clamp fires a scroll event too
  [pane] = render(mem, [pane], [region("r1|narrative:2")]);
  assert.equal(pane.scrollTop, 2600);
});

test("a conversation read at its latest message follows new ones in", () => {
  const mem = new ScrollMemory();
  let chat = region("r1|chat:4", 1000, 400, "bottom");
  [chat] = render(mem, [], [chat]);
  assert.equal(chat.scrollTop, 600, "a thread opens at its latest message");
  [chat] = render(mem, [chat], [region("r1|chat:4", 1300, 400, "bottom")]);
  assert.equal(chat.scrollTop, 900);
});

test("a conversation scrolled back stays where it was when a message arrives", () => {
  const mem = new ScrollMemory();
  let chat = region("r1|chat:4", 1000, 400, "bottom");
  [chat] = render(mem, [], [chat]);
  chat.scrollTop = 120;
  mem.record(chat);
  [chat] = render(mem, [chat], [region("r1|chat:4", 1300, 400, "bottom")]);
  assert.equal(chat.scrollTop, 120);
});

test("positions persist per review and come back under the same keys", () => {
  const mem = new ScrollMemory();
  for (const [k, v] of [["r1|narrative:0", 10], ["r1|files:a.rs", 20], ["r2|narrative:0", 30]] as const) {
    const el = region(k);
    el.scrollTop = v;
    mem.record(el);
  }
  const saved = mem.snapshot("r1|");
  assert.deepEqual(saved, { "narrative:0": 10, "files:a.rs": 20 });

  const fresh = new ScrollMemory();
  fresh.load("r1|", saved);
  const [pane] = render(fresh, [], [region("r1|files:a.rs")]);
  assert.equal(pane.scrollTop, 20);
});
