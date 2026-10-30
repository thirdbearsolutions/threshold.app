// The draft invariant, tested without a browser: a composer is a plain
// object, and "rendering" is what render() does to one — throw it away
// and build an empty one in its place.
//
// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { DraftMemory, type Field } from "./drafts.ts";

let focused: Field | null = null;

function field(key: string): Field {
  const f: Field & { start: number; end: number } = {
    dataset: { draft: key },
    value: "",
    start: 0,
    end: 0,
    get selectionStart() { return f.start; },
    get selectionEnd() { return f.end; },
    setSelectionRange(s: number, e: number) { f.start = s; f.end = e; },
    focus() { focused = f; },
  };
  return f;
}

/** One render: capture the live composers, replace them, restore the new ones. */
function render(mem: DraftMemory, live: Field[], keys: string[]): Field[] {
  mem.capture(live, focused && live.includes(focused) ? focused : null);
  focused = null;
  const next = keys.map(field);
  mem.restore(next);
  return next;
}

function type(mem: DraftMemory, f: Field, text: string, fireInput = true): void {
  f.value = text;
  f.setSelectionRange(text.length, text.length);
  f.focus();
  if (fireInput) mem.input(f.dataset.draft!, text);
}

test("a render keeps what is being typed, and the cursor where it was", () => {
  const mem = new DraftMemory();
  let [modal] = render(mem, [], ["modal:7"]);
  type(mem, modal, "I think this should be");
  modal.setSelectionRange(8, 8);
  [modal] = render(mem, [modal], ["modal:7"]);
  assert.equal(modal.value, "I think this should be");
  assert.equal(focused, modal);
  assert.deepEqual([modal.selectionStart, modal.selectionEnd], [8, 8]);
});

test("a reply on another thread, twice in ten seconds, blanks nothing", () => {
  const mem = new DraftMemory();
  let fields = render(mem, [], ["modal:7", "chat:3", "global"]);
  type(mem, fields[1], "chat draft");
  type(mem, fields[0], "modal draft");
  for (let i = 0; i < 2; i++) fields = render(mem, fields, ["modal:7", "chat:3", "global"]);
  assert.deepEqual(fields.map((f) => f.value), ["modal draft", "chat draft", ""]);
  assert.equal(focused, fields[0], "the cursor stays in the modal");
});

test("text that never fired an input event still survives", () => {
  // Dictation, autocorrect and some pastes change the value without the
  // event the old code relied on. The DOM is the truth at capture time.
  const mem = new DraftMemory();
  let [f] = render(mem, [], ["line:a.rs#4"]);
  type(mem, f, "dictated text", false);
  [f] = render(mem, [f], ["line:a.rs#4"]);
  assert.equal(f.value, "dictated text");
});

test("a composer closed and reopened comes back with its draft", () => {
  const mem = new DraftMemory();
  let [f] = render(mem, [], ["modal:7"]);
  type(mem, f, "half a thought");
  const none = render(mem, [f], []); // Esc: the modal is gone
  [f] = render(mem, none, ["modal:7"]);
  assert.equal(f.value, "half a thought");
});

test("drafts do not leak between composers", () => {
  const mem = new DraftMemory();
  let fields = render(mem, [], ["modal:7", "modal:8"]);
  type(mem, fields[0], "for seven");
  fields = render(mem, fields, ["modal:8", "modal:7"]);
  assert.equal(fields[0].value, "");
  assert.equal(fields[1].value, "for seven");
});

test("sending takes the text; the next render shows an empty composer", () => {
  const mem = new DraftMemory();
  let [f] = render(mem, [], ["chat:3"]);
  type(mem, f, "send me");
  // Enter: send() takes the text and renders, while the old textarea still
  // holds it. That capture must not turn the sent message back into a draft.
  assert.equal(mem.take("chat:3"), "send me");
  [f] = render(mem, [f], ["chat:3"]);
  assert.equal(f.value, "");
  [f] = render(mem, [f], ["chat:3"]);
  assert.equal(f.value, "", "and it stays sent");
});

test("a failed send puts the text back, keeping anything typed meanwhile", () => {
  const mem = new DraftMemory();
  let [f] = render(mem, [], ["chat:3"]);
  type(mem, f, "first message");
  const taken = mem.take("chat:3");
  [f] = render(mem, [f], ["chat:3"]);
  type(mem, f, "typed while it sent");
  mem.putBack("chat:3", taken);
  [f] = render(mem, [f], ["chat:3"]);
  assert.equal(f.value, "first message\ntyped while it sent");
});

test("a newly opened composer takes the cursor once", () => {
  const mem = new DraftMemory();
  let fields = render(mem, [], ["global"]);
  type(mem, fields[0], "elsewhere");
  mem.focus("modal:9");
  fields = render(mem, fields, ["global", "modal:9"]);
  assert.equal(focused, fields[1]);
  // It has the cursor now; the next render keeps it there, not back in global.
  fields = render(mem, fields, ["global", "modal:9"]);
  assert.equal(focused, fields[1]);
  assert.equal(fields[0].value, "elsewhere");
});

test("while an input method is composing, rendering is not allowed", () => {
  const mem = new DraftMemory();
  assert.equal(mem.composing, false);
  mem.compositionStart("modal:7");
  assert.equal(mem.composing, true);
  mem.compositionEnd();
  assert.equal(mem.composing, false);
});

test("a failed send with nothing typed meanwhile comes back whole", () => {
  const mem = new DraftMemory();
  let [f] = render(mem, [], ["chat:3"]);
  type(mem, f, "only message");
  const taken = mem.take("chat:3");
  [f] = render(mem, [f], ["chat:3"]);
  mem.putBack("chat:3", taken);
  [f] = render(mem, [f], ["chat:3"]);
  assert.equal(f.value, "only message");
});
