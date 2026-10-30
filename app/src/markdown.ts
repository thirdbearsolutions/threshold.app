// A Markdown file in a review, rendered as it will read, with what the
// diff changed marked in the margin.
//
// Prose reviewed as source lines is slow to read and says nothing about
// how it renders. So a `.md` file is shown whole, as it stands at the
// review's new side, and the diff is carried as marks rather than as
// inline insertions and deletions: a bar beside every rendered block that
// contains a changed line, and a marker where lines were only removed.
// Rendering deletions inside Markdown would mean rendering two documents
// at once, and the raw diff is one click away for that.
//
// Raw HTML in the source is escaped, never rendered: the file comes from
// the agent's branch. Images are shown as their alt text; links do not
// navigate, since a click would take the app itself off the review.
//
// Free of the DOM so it is tested on its own.

import MarkdownIt from "markdown-it";
import type Token from "markdown-it/lib/token.mjs";
import type { DiffFile } from "./api";

/** Where `src`, written in the Markdown file at `doc`, points in the repo:
 *  resolved against the file's directory, or null if it climbs out of the
 *  repo. A leading `/` means the repo root, as GitHub reads it. */
export function resolveRepoPath(doc: string, src: string): string | null {
  const clean = decodeURI(src.split(/[?#]/)[0]);
  const parts = clean.startsWith("/") ? [] : doc.split("/").slice(0, -1);
  for (const seg of clean.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (!parts.length) return null;
      parts.pop();
    } else parts.push(seg);
  }
  return parts.length ? parts.join("/") : null;
}

export function isMarkdown(path: string | null | undefined): boolean {
  return !!path && /\.(md|markdown|mdx?)$/i.test(path);
}

/** What a diff did to a file, in new-side line numbers. */
export interface Changes {
  /** Lines added or rewritten. */
  changed: Set<number>;
  /** Removed lines, by the new-side line they sat just before. A removal
   *  at the end of the file is keyed one past its last line. */
  removedBefore: Map<number, number>;
}

/** The changes made by `hunks` of `file`, or all of them. */
export function changesOf(file: DiffFile, hunks?: number[]): Changes {
  const changed = new Set<number>();
  const removedBefore = new Map<number, number>();
  const which = hunks ?? file.hunks.map((_, i) => i);
  for (const i of which) {
    const hunk = file.hunks[i];
    if (!hunk) continue;
    const m = /\+(\d+)/.exec(hunk.header);
    let n = m ? Number(m[1]) : 1;
    let removed = 0;
    for (const line of hunk.lines) {
      if (line.kind === "removed") {
        removed++;
        continue;
      }
      // Removed lines followed by added ones are those lines rewritten,
      // which the added lines already mark. Only removed lines followed
      // by unchanged ones are gone without replacement.
      if (removed && line.kind === "context") removedBefore.set(n, (removedBefore.get(n) ?? 0) + removed);
      removed = 0;
      if (line.kind === "added") changed.add(n);
      n++;
    }
    if (removed) removedBefore.set(n, (removedBefore.get(n) ?? 0) + removed);
  }
  return { changed, removedBefore };
}

/** Block tokens that stand for something a reader sees as one unit. */
const BLOCKS = new Set(["heading_open", "paragraph_open", "fence", "code_block", "hr", "tr_open", "html_block", "list_item_open", "blockquote_open"]);

/** Blocks that contain others: marked only when nothing inside them is,
 *  so a changed sentence in a list item is one bar, not two. */
const CONTAINERS = new Set(["list_item_open", "blockquote_open"]);

interface Block {
  token: Token;
  /** 1-based, inclusive. */
  from: number;
  to: number;
  container: boolean;
}

function blocksOf(tokens: Token[]): Block[] {
  const out: Block[] = [];
  for (const t of tokens) {
    // A tight list hides its paragraphs; they render no element to mark,
    // so the list item stands in for them.
    if (!BLOCKS.has(t.type) || !t.map || (t.type === "paragraph_open" && t.hidden)) continue;
    out.push({ token: t, from: t.map[0] + 1, to: t.map[1], container: CONTAINERS.has(t.type) });
  }
  return out;
}

/** The innermost blocks covering `line`: leaves if any cover it, else
 *  the containers that do. */
function covering(blocks: Block[], line: number): Block[] {
  const hit = blocks.filter((b) => b.from <= line && line <= b.to);
  const leaves = hit.filter((b) => !b.container);
  if (leaves.length) return leaves;
  // The innermost container: the one that starts latest.
  const inner = hit.sort((a, b) => b.from - a.from)[0];
  return inner ? [inner] : [];
}

function addClass(t: Token, cls: string): void {
  const now = t.attrGet("class");
  t.attrSet("class", now ? `${now} ${cls}` : cls);
}

function createRenderer(): MarkdownIt {
  const md = new MarkdownIt({ html: false, linkify: true, typographer: false });
  md.renderer.rules.image = (tokens, idx) => {
    const t = tokens[idx];
    const src = t.attrGet("src") ?? "";
    const alt = md.utils.escapeHtml(t.content || src || "image");
    // A path into the repo is shown from the repo, by the view, at the
    // review's commit. Anything with a scheme stays alt text: a review
    // should not fetch from the network because the document says so.
    const local = src && !/^[a-z][a-z0-9+.-]*:/i.test(src) && !src.startsWith("//") && !src.startsWith("#");
    const data = local ? ` data-local-src="${md.utils.escapeHtml(src)}"` : "";
    return `<span class="md-image" title="${md.utils.escapeHtml(src)}"${data}>[image: ${alt}]</span>`;
  };
  return md;
}

const md = createRenderer();

/** Render `source`, marking the blocks `changes` touched and those with a
 *  note thread on one of their lines.
 *
 *  Marks are classes and data attributes on the rendered blocks:
 *  `md-changed`; `md-removed` with `data-removed` (and `md-removed-after`
 *  when the removal follows the block); `md-noted` with `data-note-line`,
 *  the first noted line, for jumping to it in the diff. */
export function renderMarkdown(source: string, changes: Changes, notedLines: number[] = []): string {
  const env = {};
  const tokens = md.parse(source, env);
  const blocks = blocksOf(tokens);

  for (const n of changes.changed) for (const b of covering(blocks, n)) addClass(b.token, "md-changed");

  for (const [n, count] of changes.removedBefore) {
    // Removed from the middle of a block: that block changed.
    const inside = blocks.filter((b) => b.from < n && n <= b.to && !b.container);
    if (inside.length) {
      for (const b of inside) addClass(b.token, "md-changed");
      continue;
    }
    // The first block from there on; a leaf over a container starting on
    // the same line, but a container when it has no leaf of its own (an
    // item in a tight list).
    const byStart = (a: Block, b: Block) => a.from - b.from || Number(a.container) - Number(b.container);
    const next = blocks.filter((b) => b.from >= n).sort(byStart)[0];
    const target = next ?? blocks.filter((b) => !b.container).sort((a, b) => b.to - a.to)[0];
    if (!target) continue;
    addClass(target.token, next ? "md-removed" : "md-removed md-removed-after");
    const before = Number(target.token.attrGet("data-removed") ?? 0);
    target.token.attrSet("data-removed", String(before + count));
  }

  for (const n of [...notedLines].sort((a, b) => a - b)) {
    for (const b of covering(blocks, n)) {
      if (b.token.attrGet("data-note-line")) continue;
      addClass(b.token, "md-noted");
      b.token.attrSet("data-note-line", String(n));
    }
  }

  return md.renderer.render(tokens, md.options, env);
}

// Code blocks carry their attributes onto the <pre>, which the default
// rules do not do for `code_block` and do only partly for `fence`.
for (const kind of ["fence", "code_block"] as const) {
  const base = md.renderer.rules[kind]!;
  md.renderer.rules[kind] = (tokens, idx, options, env, self) => {
    const t = tokens[idx];
    const cls = t.attrGet("class");
    const attrs = [
      cls ? ` class="${md.utils.escapeHtml(cls)}"` : "",
      ...["data-removed", "data-note-line"].map((a) => (t.attrGet(a) ? ` ${a}="${md.utils.escapeHtml(t.attrGet(a)!)}"` : "")),
    ].join("");
    if (!attrs) return base(tokens, idx, options, env, self);
    // Rendered without the marks, then wrapped, so the language class on
    // the fence's <code> is left as the base rule wrote it.
    const saved = t.attrs;
    t.attrs = null;
    const inner = base(tokens, idx, options, env, self);
    t.attrs = saved;
    return `<div${attrs}>${inner}</div>`;
  };
}
