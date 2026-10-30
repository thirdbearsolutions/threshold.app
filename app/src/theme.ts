// Which theme is on screen, and keeping it there. public/theme-boot.js
// applies the saved choice before the first paint; this module owns it
// afterwards: choosing, following macOS when the choice is System, and
// telling the native window which appearance to draw its title bar in.

export type Theme = "light" | "dark" | "threshold";
export type Choice = Theme | "system";

export const CHOICES: { id: Choice; name: string; note: string }[] = [
  { id: "system", name: "System", note: "Light or Dark, following macOS" },
  { id: "light", name: "Light", note: "" },
  { id: "dark", name: "Dark", note: "" },
  { id: "threshold", name: "Threshold", note: "Amber on warm dark" },
];

const KEY = "threshold.theme";

/** The theme a choice puts on screen. */
export function resolve(choice: Choice, systemDark: boolean): Theme {
  return choice === "system" ? (systemDark ? "dark" : "light") : choice;
}

/** Whether a theme is drawn on a dark ground. */
export function isDark(theme: Theme): boolean {
  return theme !== "light";
}

/** The saved choice, or null the first time Threshold opens. */
export function savedChoice(store: Pick<Storage, "getItem"> = localStorage): Choice | null {
  let v: string | null = null;
  try { v = store.getItem(KEY); } catch { /* storage unavailable: treat as never chosen */ }
  return v === "system" || v === "light" || v === "dark" || v === "threshold" ? v : null;
}

const media = () => window.matchMedia("(prefers-color-scheme: dark)");
let current: Choice = "system";
let onApply: ((t: Theme) => void) | null = null;

function apply(): void {
  const theme = resolve(current, media().matches);
  document.documentElement.dataset.theme = theme;
  onApply?.(theme);
}

/** Put a choice on screen. Saved only when `save` is set, so a picker can
 *  preview each card before the reader settles on one. */
export function choose(choice: Choice, save = true): void {
  current = choice;
  if (save) try { localStorage.setItem(KEY, choice); } catch { /* not saved: the picker shows again next time */ }
  apply();
}

/** Start following the saved choice; `applied` hears every change of
 *  theme, including macOS switching appearance under System. */
export function initTheme(applied: (t: Theme) => void): Choice | null {
  const saved = savedChoice();
  current = saved ?? "system";
  onApply = applied;
  media().addEventListener("change", () => { if (current === "system") apply(); });
  apply();
  return saved;
}

export function currentChoice(): Choice {
  return current;
}
