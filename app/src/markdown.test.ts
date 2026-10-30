// The Markdown view's marks, tested on rendered HTML without a browser.
//
// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { changesOf, isMarkdown, renderMarkdown, resolveRepoPath } from "./markdown.ts";
import type { DiffFile } from "./api.ts";

function file(header: string, lines: [" " | "+" | "-", string][]): DiffFile {
  const kind = { " ": "context", "+": "added", "-": "removed" } as const;
  return {
    old_path: "doc.md",
    new_path: "doc.md",
    status: "modified",
    hunks: [{ header, lines: lines.map(([k, content]) => ({ kind: kind[k], content })) }],
    additions: lines.filter(([k]) => k === "+").length,
    deletions: lines.filter(([k]) => k === "-").length,
  };
}

const none = { changed: new Set<number>(), removedBefore: new Map<number, number>() };

test("markdown files are recognised by extension", () => {
  assert.ok(isMarkdown("docs/README.md"));
  assert.ok(isMarkdown("NOTES.markdown"));
  assert.ok(!isMarkdown("src/main.ts"));
  assert.ok(!isMarkdown(null));
});

test("changes are counted in new-side lines, and removals keyed to the line after", () => {
  const f = file("@@ -1,5 +1,4 @@", [[" ", "a"], ["+", "A2"], [" ", "b"], ["-", "c"], ["-", "d"], [" ", "e"]]);
  const c = changesOf(f);
  assert.deepEqual([...c.changed], [2]);
  assert.deepEqual([...c.removedBefore], [[4, 2]]);
});

test("a changed paragraph is marked and an untouched one is not", () => {
  const src = "# Title\n\nFirst para.\n\nSecond para.\n";
  const html = renderMarkdown(src, { changed: new Set([5]), removedBefore: new Map() });
  assert.match(html, /<p class="md-changed">Second para\.<\/p>/);
  assert.match(html, /<p>First para\.<\/p>/);
  assert.match(html, /<h1>Title<\/h1>/);
});

test("a changed item in a tight list marks the item, once", () => {
  const src = "- one\n- two\n- three\n";
  const html = renderMarkdown(src, { changed: new Set([2]), removedBefore: new Map() });
  assert.equal((html.match(/md-changed/g) ?? []).length, 1);
  assert.match(html, /<li class="md-changed">two<\/li>/);
});

test("a changed item in a loose list marks its paragraph, not the item too", () => {
  const src = "- one\n\n- two\n";
  const html = renderMarkdown(src, { changed: new Set([3]), removedBefore: new Map() });
  assert.equal((html.match(/md-changed/g) ?? []).length, 1);
  assert.match(html, /<p class="md-changed">two<\/p>/);
});

test("lines removed between blocks mark the block after, with a count", () => {
  const src = "Kept.\n\nAfter.\n";
  const html = renderMarkdown(src, { changed: new Set(), removedBefore: new Map([[3, 4]]) });
  assert.match(html, /<p class="md-removed" data-removed="4">After\.<\/p>/);
});

test("lines removed from the end of the file mark the last block as followed by a removal", () => {
  const html = renderMarkdown("Only.\n", { changed: new Set(), removedBefore: new Map([[2, 3]]) });
  assert.match(html, /class="md-removed md-removed-after" data-removed="3"/);
});

test("a line removed from inside a paragraph marks the paragraph changed", () => {
  const html = renderMarkdown("one\ntwo\nthree\n", { changed: new Set(), removedBefore: new Map([[2, 1]]) });
  assert.match(html, /<p class="md-changed">/);
});

test("a changed code fence keeps its language class and carries the mark", () => {
  const src = "```rust\nfn main() {}\n```\n";
  const html = renderMarkdown(src, { changed: new Set([2]), removedBefore: new Map() });
  assert.match(html, /<div class="md-changed"><pre><code class="language-rust">/);
});

test("a noted line marks its block with the line to jump to", () => {
  const html = renderMarkdown("a\n\nb\n", none, [3]);
  assert.match(html, /<p class="md-noted" data-note-line="3">b<\/p>/);
});

test("raw HTML is escaped, and images become their alt text", () => {
  const html = renderMarkdown('<script>alert(1)</script>\n\n![a diagram](https://example.com/x.png)\n', none);
  assert.ok(!html.includes("<script>"));
  assert.ok(!html.includes("<img"));
  assert.match(html, /\[image: a diagram\]/);
});

test("a line rewritten in place is a change, not a change plus a removal", () => {
  const f = file("@@ -1,3 +1,3 @@", [[" ", "a"], ["-", "old"], ["+", "new"], [" ", "c"]]);
  const c = changesOf(f);
  assert.deepEqual([...c.changed], [2]);
  assert.equal(c.removedBefore.size, 0);
});

test("a pure removal just before a tight list item marks that item", () => {
  const html = renderMarkdown("Intro.\n\n- one\n- two\n", { changed: new Set(), removedBefore: new Map([[4, 1]]) });
  assert.match(html, /<li class="md-removed" data-removed="1">two<\/li>/);
});

test("an image in the repo is marked for the view to show; one on the web is not", () => {
  const html = renderMarkdown("![overview](world-overview.png)\n\n![remote](https://example.com/x.png)\n", none);
  assert.match(html, /data-local-src="world-overview.png"/);
  assert.equal((html.match(/data-local-src/g) ?? []).length, 1);
});

test("an image path resolves against the document's directory and cannot leave the repo", () => {
  assert.equal(resolveRepoPath("docs/design/README.md", "world-overview.png"), "docs/design/world-overview.png");
  assert.equal(resolveRepoPath("docs/design/README.md", "./example/rooms/cove.png"), "docs/design/example/rooms/cove.png");
  assert.equal(resolveRepoPath("docs/design/README.md", "../img/a%20b.png"), "docs/img/a b.png");
  assert.equal(resolveRepoPath("docs/README.md", "/assets/logo.png"), "assets/logo.png");
  assert.equal(resolveRepoPath("README.md", "../../etc/passwd"), null);
});
