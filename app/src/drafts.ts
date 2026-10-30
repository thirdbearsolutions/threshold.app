// Unsent text, held as an invariant rather than patched per composer.
//
// THE RULE: a composer's text is a function of its key. Rendering never
// changes it. Only three things do — the reviewer typing, a send taking
// the text, and a failed send putting it back.
//
// Threshold renders by replacing the whole DOM, and a new textarea starts
// empty. A reply from Claude, a toast expiring, a diagnostic landing — any
// render at all — used to blank whatever the reviewer was halfway through
// writing, in a line composer, a chapter comment, a thread modal or the
// chat pane alike.
//
// So there is one mechanism, and `render` is its only caller:
//
//   - every composer carries `data-draft="<key>"`; the key names what the
//     text is *for* (a reply in thread 7, a comment on src/db.rs:142), so a
//     composer rebuilt with the same key is the same draft;
//   - `capture` runs before the DOM is replaced and reads every composer's
//     live value — the DOM is the truth, not the last input event, so text
//     from dictation, autocorrect or a paste that never fired one survives;
//   - `restore` runs after, writing each composer's text back and putting
//     the cursor exactly where it was;
//   - while an input method is composing (accents, dictation, CJK input),
//     rendering is not allowed at all: replacing the element would cancel
//     the composition and drop what was being entered. `render` asks
//     `composing` and waits for `compositionend`.
//
// Free of the DOM beyond the few members it touches, so the rule can be
// tested without a browser.

/** The part of a textarea this module touches. */
export interface Field {
  readonly dataset: { draft?: string };
  value: string;
  readonly selectionStart: number | null;
  readonly selectionEnd: number | null;
  setSelectionRange(start: number, end: number): void;
  focus(options?: { preventScroll?: boolean }): void;
}

export class DraftMemory {
  private readonly text = new Map<string, string>();
  private cursor: { key: string; start: number; end: number } | null = null;
  private wantFocus: string | null = null;
  private composingIn: string | null = null;
  /** Values still sitting in a textarea that the memory has deliberately
   *  replaced — the text just sent, or what was typed before a failed send
   *  came back — until the next render writes the new value in. Capturing
   *  them would undo the replacement: the sent message would return as a
   *  draft, and a returned message would be overwritten. */
  private readonly superseded = new Map<string, string>();

  /** The draft for `key`, or empty. */
  get(key: string): string {
    return this.text.get(key) ?? "";
  }

  /** The reviewer typed. */
  input(key: string, value: string): void {
    if (value) this.text.set(key, value);
    else this.text.delete(key);
  }

  /** Before the DOM is replaced: read every composer, and where the cursor is. */
  capture(fields: Iterable<Field>, active: Field | null): void {
    for (const f of fields) {
      const key = f.dataset.draft;
      if (!key || this.superseded.get(key) === f.value) continue;
      this.input(key, f.value);
    }
    const key = active?.dataset.draft;
    this.cursor = key && active ? { key, start: active.selectionStart ?? 0, end: active.selectionEnd ?? 0 } : null;
  }

  /** After: every composer gets its text back, and the cursor its place. */
  restore(fields: Iterable<Field>): void {
    this.superseded.clear();
    const byKey = new Map<string, Field>();
    for (const f of fields) {
      const key = f.dataset.draft;
      if (!key) continue;
      f.value = this.get(key);
      byKey.set(key, f);
    }
    const want = this.wantFocus ?? this.cursor?.key ?? null;
    this.wantFocus = null;
    const el = want ? byKey.get(want) : undefined;
    if (!el) return;
    el.focus({ preventScroll: true });
    if (this.cursor?.key === want) el.setSelectionRange(this.cursor.start, this.cursor.end);
    else el.setSelectionRange(el.value.length, el.value.length);
  }

  /** A composer that has just been opened takes the cursor on the next render. */
  focus(key: string): void {
    this.wantFocus = key;
  }

  /** Sending: the text leaves the composer. Returns it, untrimmed. */
  take(key: string): string {
    const t = this.get(key);
    this.text.delete(key);
    this.superseded.set(key, t);
    if (this.cursor?.key === key) this.cursor = { key, start: 0, end: 0 };
    return t;
  }

  /** A send failed: the text comes back. Anything typed since the send
   *  began is kept after it, never overwritten. */
  putBack(key: string, taken: string): void {
    const since = this.get(key);
    this.superseded.set(key, since);
    this.text.set(key, since ? `${taken}\n${since}` : taken);
  }

  /** An input method began or finished composing in `key`. */
  compositionStart(key: string): void {
    this.composingIn = key;
  }

  compositionEnd(): void {
    this.composingIn = null;
  }

  /** Whether a render now would destroy text still being composed. */
  get composing(): boolean {
    return this.composingIn != null;
  }
}
