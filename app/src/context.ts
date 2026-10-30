// Expanding the unchanged code around a hunk, as GitHub does.
//
// A diff shows each change with three lines either side, and the gaps
// between are exactly where "wait, what calls this?" gets answered. Each
// gap can be opened from its top edge, its bottom edge, or all at once;
// what has been opened is recorded as a count from each edge, so opening
// more never moves what is already shown and the gap closes up when the
// two meet.
//
// Numbers here are new-file line numbers, the same ones the gutter and
// every line note use, so an expanded line is annotatable like any other.

/** A hunk's new-file span, from its `@@ -a,b +c,d @@` header. */
export interface Span {
  start: number;
  count: number;
}

export function spanOf(header: string): Span {
  const m = /\+(\d+)(?:,(\d+))?/.exec(header);
  return { start: m ? Number(m[1]) : 1, count: m?.[2] != null ? Number(m[2]) : 1 };
}

/** Unchanged new-file lines between two hunks, inclusive. `last` is null
 *  for the gap after the final hunk until the file's length is known. */
export interface Gap {
  first: number;
  last: number | null;
}

/** The gaps around `spans`, one more than there are hunks: before the
 *  first, between each pair, after the last. */
export function gapsAround(spans: Span[], fileLines: number | null): Gap[] {
  const out: Gap[] = [];
  let next = 1;
  for (const s of spans) {
    // An empty new side (`+c,0`) sits after line c rather than at it.
    const begin = s.count === 0 ? s.start + 1 : s.start;
    out.push({ first: next, last: begin - 1 });
    next = begin + s.count;
  }
  out.push({ first: next, last: fileLines });
  return out;
}

/** How much of a gap is open, counted from each edge. */
export interface Opened {
  top: number;
  bottom: number;
}

export const STEP = 20;

/** A gap's size, or null when it runs to an end of file not yet known. */
export function sizeOf(gap: Gap): number | null {
  return gap.last == null ? null : Math.max(0, gap.last - gap.first + 1);
}

/** What to draw for a gap: the lines open from its top, how many stay
 *  hidden (null: unknown), and the lines open from its bottom. */
export function layout(gap: Gap, opened: Opened): { top: [number, number] | null; hidden: number | null; bottom: [number, number] | null } {
  const size = sizeOf(gap);
  if (size == null) {
    // Only the top edge can be opened before the file's end is known.
    return { top: opened.top > 0 ? [gap.first, gap.first + opened.top - 1] : null, hidden: null, bottom: null };
  }
  const top = Math.min(opened.top, size);
  const bottom = Math.min(opened.bottom, size - top);
  return {
    top: top > 0 ? [gap.first, gap.first + top - 1] : null,
    hidden: size - top - bottom,
    bottom: bottom > 0 ? [gap.first + size - bottom, gap.first + size - 1] : null,
  };
}

/** Open `n` more lines from one edge, never more than the gap holds. */
export function open(gap: Gap, opened: Opened, edge: "top" | "bottom" | "all", n = STEP): Opened {
  const size = sizeOf(gap);
  if (edge === "all") return size == null ? opened : { top: size, bottom: 0 };
  const room = size == null ? Infinity : Math.max(0, size - opened.top - opened.bottom);
  const add = Math.min(n, room);
  return edge === "top" ? { ...opened, top: opened.top + add } : { ...opened, bottom: opened.bottom + add };
}
