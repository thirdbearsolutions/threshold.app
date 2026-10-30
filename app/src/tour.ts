// The point-and-click tour: a cut-out around one thing on screen and a
// card beside it, one step at a time.
//
// The engine only knows steps. What each step points at, and what has to
// happen on screen before it can (open the sample, turn to a chapter), is
// the caller's: main.ts owns that state.

import { h } from "./util";

export interface TourStep {
  title: string;
  text: string;
  /** What to point at, looked up after `before` has run. */
  target: () => Element | null;
  /** Put the screen in the state this step talks about. */
  before?: () => Promise<void> | void;
}

let active: { stop: (finished: boolean) => void } | null = null;

export function tourRunning(): boolean {
  return !!active;
}

/** Run the steps. `done` hears whether the last step was reached rather
 *  than the tour being skipped. */
export function runTour(steps: TourStep[], done: (finished: boolean) => void): void {
  active?.stop(false);
  let at = 0;
  let frame = 0;
  const hole = h("div", { class: "tour-hole", "aria-hidden": "true" });
  const card = h("div", { class: "tour-card", role: "dialog", "aria-live": "polite" });
  document.body.append(hole, card);

  const place = () => {
    const el = steps[at].target();
    const r = el?.getBoundingClientRect();
    if (!r || (r.width === 0 && r.height === 0)) {
      // Nothing to point at: a plain card in the middle, no cut-out.
      hole.style.display = "none";
      Object.assign(card.style, { left: `${(innerWidth - card.offsetWidth) / 2}px`, top: `${innerHeight / 3}px` });
      card.dataset.side = "none";
    } else {
      const pad = 4;
      const top = Math.max(r.top, 0);
      const bottom = Math.min(r.bottom, innerHeight);
      hole.style.display = "";
      Object.assign(hole.style, { left: `${r.left - pad}px`, top: `${top - pad}px`, width: `${r.width + pad * 2}px`, height: `${Math.max(bottom - top, 0) + pad * 2}px` });
      // Beside it where there's room, else below, else above.
      const gap = 14;
      const w = card.offsetWidth;
      const ch = card.offsetHeight;
      let left: number;
      let y: number;
      let side: string;
      if (r.right + gap + w < innerWidth) { left = r.right + gap; y = top; side = "right"; }
      else if (r.left - gap - w > 0) { left = r.left - gap - w; y = top; side = "left"; }
      else if (bottom + gap + ch < innerHeight) { left = r.left; y = bottom + gap; side = "below"; }
      else { left = r.left; y = top - gap - ch; side = "above"; }
      left = Math.min(Math.max(left, 8), innerWidth - w - 8);
      y = Math.min(Math.max(y, 8), innerHeight - ch - 8);
      Object.assign(card.style, { left: `${left}px`, top: `${y}px` });
      card.dataset.side = side;
    }
    frame = requestAnimationFrame(place);
  };

  const show = async (i: number) => {
    cancelAnimationFrame(frame);
    at = i;
    await steps[at].before?.();
    // One frame for the app to render what `before` changed.
    await new Promise((r) => requestAnimationFrame(r));
    steps[at].target()?.scrollIntoView({ block: "nearest", inline: "nearest" });
    const last = at === steps.length - 1;
    card.replaceChildren(
      h("div", { class: "tour-step", text: `${at + 1} of ${steps.length}` }),
      h("div", { class: "tour-title", text: steps[at].title }),
      h("p", { class: "tour-text", text: steps[at].text }),
      h(
        "div",
        { class: "tour-nav" },
        last ? h("span") : h("button", { class: "tour-skip", text: "Skip tour", onClick: () => stop(false) }),
        h(
          "span",
          { class: "tour-buttons" },
          at > 0 ? h("button", { class: "btn small", text: "Back", onClick: () => void show(at - 1) }) : "",
          h("button", { class: "btn small primary", text: last ? "Done" : "Next", onClick: () => (last ? stop(true) : void show(at + 1)) }),
        ),
      ),
    );
    (card.querySelector(".btn.primary") as HTMLElement | null)?.focus();
    place();
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      stop(false);
    }
  };

  const stop = (finished: boolean) => {
    if (!active) return;
    active = null;
    cancelAnimationFrame(frame);
    hole.remove();
    card.remove();
    document.removeEventListener("keydown", onKey, true);
    done(finished);
  };

  active = { stop };
  document.addEventListener("keydown", onKey, true);
  void show(0);
}
