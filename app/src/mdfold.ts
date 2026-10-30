// Folding a rendered Markdown document down to what changed, shared by the
// desktop and the phone.

import { h } from "./util";

/** How many unchanged blocks either side of a change stay open, as the
 *  diff keeps lines of context around a hunk. */
const MD_CONTEXT = 1;

/** Top-level blocks of a rendered document, with long unchanged stretches
 *  folded away, as the diff folds unchanged code.
 *
 *  A one-line edit in a long document is otherwise a bar somewhere below
 *  the fold that the reader has to go looking for. Each change keeps a
 *  block of context either side and the nearest heading above it, so a
 *  change is never shown without saying which section it is in. */
export function foldUnchanged(blocks: HTMLElement[], unfolded: boolean, unfold: () => void): (HTMLElement | string)[] {
  const touched = (el: HTMLElement) => el.matches(".md-changed, .md-removed, .md-noted") || !!el.querySelector(".md-changed, .md-removed, .md-noted");
  const marked = blocks.map(touched);
  if (unfolded || !marked.includes(false) || !marked.includes(true)) return blocks;
  const keep = new Set<number>();
  marked.forEach((m, i) => {
    if (!m) return;
    for (let k = Math.max(0, i - MD_CONTEXT); k <= Math.min(blocks.length - 1, i + MD_CONTEXT); k++) keep.add(k);
    for (let k = i - 1; k >= 0; k--) if (/^H[1-6]$/.test(blocks[k].tagName)) { keep.add(k); break; }
  });
  const out: (HTMLElement | string)[] = [];
  let hidden: HTMLElement[] = [];
  const flush = () => {
    // One block is shorter to read than a button standing in for it.
    if (hidden.length === 1) out.push(hidden[0]);
    else if (hidden.length) out.push(h("button", { class: "md-fold", text: `⋯ ${hidden.length} unchanged blocks`, title: "Show the whole document", onClick: unfold }));
    hidden = [];
  };
  blocks.forEach((b, i) => {
    if (keep.has(i)) { flush(); out.push(b); } else hidden.push(b);
  });
  flush();
  return out;
}
