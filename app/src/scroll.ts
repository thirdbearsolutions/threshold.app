// Scroll position, held as an invariant rather than patched per feature.
//
// THE RULE: a scrolling region's position is a function of its key.
// Rendering never changes it. Only two things do — the reader scrolling,
// and an explicit navigation that names where to land (`land`).
//
// Threshold renders by replacing the whole DOM, and a new scroll container
// starts at the top. Before this module, each feature that noticed that
// patched it locally: a remembered offset per chapter index (shared, by
// accident, with the file view), a requestAnimationFrame restore after a
// jump, a flag to stop the restore fighting the jump. Anything that
// rendered without knowing about those — a reply arriving, a modal opening
// or closing, a toast expiring — sent the reader back to the top.
//
// So there is one mechanism, and `render` is its only caller:
//
//   - every scrolling region carries `data-scroll="<key>"`; the key names
//     *what is shown* (this review's chapter 3, this thread), so a region
//     rebuilt with the same key is the same place and one with a new key
//     is somewhere else, with its own remembered position;
//   - `capture` runs before the DOM is replaced, `restore` after;
//   - regions with `data-stick="bottom"` are conversations, and follow new
//     messages in while the reader is at the latest one.
//
// Deliberately free of any DOM dependency beyond the four properties it
// reads, so the rule can be tested without a browser.

/** The part of an element this module touches. */
export interface Scrollable {
  readonly dataset: { scroll?: string; stick?: string };
  scrollTop: number;
  readonly scrollHeight: number;
  readonly clientHeight: number;
}

/** Within this many pixels of the end counts as reading the latest message. */
const BOTTOM_SLACK = 24;

function maxTop(el: Scrollable): number {
  return Math.max(0, el.scrollHeight - el.clientHeight);
}

export class ScrollMemory {
  private readonly top = new Map<string, number>();
  private readonly atBottom = new Map<string, boolean>();
  private readonly landing = new Map<string, number>();

  /** Where a region with this key is, or would be restored to. */
  get(key: string): number {
    return this.landing.get(key) ?? this.top.get(key) ?? 0;
  }

  /** Note where a region is. Called for every scroll event and for every
   *  live region just before a render.
   *
   *  A position the browser clamped is not the reader's position. Content
   *  that is briefly shorter — a chapter still loading, a reload in
   *  flight — pins scrollTop to its end; remembering that would lose the
   *  real place the moment the content returned. */
  record(el: Scrollable): void {
    const key = el.dataset.scroll;
    if (!key) return;
    const known = this.top.get(key);
    const clamped = known != null && el.scrollTop < known && el.scrollTop >= maxTop(el) - 1;
    if (!clamped) this.top.set(key, el.scrollTop);
    this.atBottom.set(key, maxTop(el) - el.scrollTop < BOTTOM_SLACK);
  }

  /** Before the DOM is replaced: remember every region as it stands. */
  capture(regions: Iterable<Scrollable>): void {
    for (const el of regions) this.record(el);
  }

  /** After the DOM is replaced: put every region where its key says. */
  restore(regions: Iterable<Scrollable>): void {
    for (const el of regions) {
      const key = el.dataset.scroll;
      if (!key) continue;
      const landing = this.landing.get(key);
      if (landing != null) {
        this.landing.delete(key);
        this.top.set(key, landing);
        el.scrollTop = landing;
      } else if (el.dataset.stick === "bottom" && this.atBottom.get(key) !== false) {
        // A conversation opened for the first time, or read at its latest
        // message, follows new ones in. Scrolled back, it stays put.
        el.scrollTop = el.scrollHeight;
      } else {
        el.scrollTop = this.top.get(key) ?? 0;
      }
    }
  }

  /** An explicit navigation: the next restore of `key` lands here, once.
   *  Back and forward use this to return to an exact offset. */
  land(key: string, top: number): void {
    this.landing.set(key, top);
    this.atBottom.set(key, false);
  }

  /** Positions under `prefix`, with the prefix removed, for persisting. */
  snapshot(prefix: string): Record<string, number> {
    const out: Record<string, number> = {};
    for (const [k, v] of this.top) if (k.startsWith(prefix)) out[k.slice(prefix.length)] = v;
    return out;
  }

  /** Positions persisted by `snapshot`, put back under `prefix`. A key
   *  already known in this session wins: it is newer. */
  load(prefix: string, saved: Record<string, number> | undefined): void {
    for (const [k, v] of Object.entries(saved ?? {})) {
      if (typeof v === "number" && !this.top.has(prefix + k)) this.top.set(prefix + k, v);
    }
  }
}
