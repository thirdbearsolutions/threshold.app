// The theme picker: a sheet the first time Threshold opens, and the same
// sheet from Threshold ▸ Settings… (⌘,) afterwards.
//
// Each card's preview is drawn by the real stylesheet: it carries
// `data-theme`, so the theme's tokens apply inside it whatever is on
// screen around it. Picking a card puts that theme on the app behind the
// sheet at once, so the choice is made looking at the real thing.

import { CHOICES, choose, currentChoice, type Choice } from "./theme";
import { h } from "./util";

let open: HTMLElement | null = null;

/** A few lines of a review, in whatever theme encloses it. */
function sample(theme: string): HTMLElement {
  return h(
    "div",
    { class: "picker-sample", "data-theme": theme, "aria-hidden": "true" },
    h("div", { class: "picker-sample-bar" }, h("span", { class: "brand-mark" }), h("span", { text: "Move book covers…" })),
    h("div", { class: "picker-sample-line" }, " def cover_url(book):"),
    h("div", { class: "picker-sample-line add" }, "+    key = cover_key(book)"),
    h("div", { class: "picker-sample-line add" }, "+    return storage.url(key)"),
    h("div", { class: "picker-sample-line del" }, "-    return local_path(book)"),
    h("div", { class: "picker-sample-foot" }, h("span", { class: "picker-sample-approve", text: "Approve" })),
  );
}

/** Show the picker. `first` is the first open: there is no earlier choice
 *  to go back to, so leaving the sheet any way keeps what is selected, and
 *  `then` hears whether to go on into the tour. */
export function openPicker(first: boolean, then?: (tour: boolean) => void): void {
  if (open) return;
  const before = currentChoice();
  let selected: Choice = before;

  const cards = CHOICES.map((c) =>
    h(
      "button",
      {
        class: `picker-card${c.id === selected ? " on" : ""}`,
        "aria-pressed": c.id === selected ? "true" : "false",
        "data-choice": c.id,
        onClick: () => pick(c.id),
      },
      c.id === "system"
        ? h("div", { class: "picker-frame split" }, sample("light"), sample("dark"))
        : h("div", { class: "picker-frame" }, sample(c.id)),
      h(
        "div",
        { class: "picker-card-text" },
        h("div", { class: "picker-name" }, c.name, c.id === "system" ? h("span", { class: "picker-rec", text: "Recommended" }) : ""),
        h("div", { class: "picker-note", text: c.note }),
      ),
    ),
  );

  function pick(id: Choice): void {
    selected = id;
    for (const card of cards) {
      const on = card.dataset.choice === id;
      card.classList.toggle("on", on);
      card.setAttribute("aria-pressed", on ? "true" : "false");
    }
    choose(id, false);
  }

  function close(keep: boolean, tour = false): void {
    choose(keep ? selected : before, true);
    open?.remove();
    open = null;
    document.removeEventListener("keydown", onKey, true);
    if (first) then?.(tour);
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close(first);
    }
  }

  open = h(
    "div",
    { class: "modal-backdrop picker-backdrop", onClick: (e: Event) => { if (e.target === open) close(first); } },
    h(
      "div",
      { class: "picker", role: "dialog", "aria-modal": "true", "aria-labelledby": "picker-title" },
      h("h1", { class: "picker-title", id: "picker-title", text: first ? "Choose a look" : "Settings" }),
      h("p", { class: "picker-lede", text: first ? "You can change it any time from Threshold ▸ Settings… (⌘,)." : "How Threshold looks." }),
      h("div", { class: "picker-cards" }, ...cards),
      first
        ? h(
            "div",
            { class: "picker-actions" },
            h("span", { class: "picker-next", text: "Next: a two-minute tour, on a sample review." }),
            h("button", { class: "btn", text: "Not now", onClick: () => close(true) }),
            h("button", { class: "btn primary", text: "Continue", onClick: () => close(true, true) }),
          )
        : h(
            "div",
            { class: "picker-actions" },
            // The tour's way back in that doesn't depend on the menu bar.
            h("button", { class: "link picker-next", text: "Take the tour again", onClick: () => { close(true); window.dispatchEvent(new Event("threshold:tour")); } }),
            h("button", { class: "btn primary", text: "Done", onClick: () => close(true) }),
          ),
    ),
  );
  document.body.append(open);
  document.addEventListener("keydown", onKey, true);
  cards.find((c) => c.dataset.choice === selected)?.focus();
}
