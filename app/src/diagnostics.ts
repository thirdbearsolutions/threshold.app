// Which of Threshold's findings still stand. Shared by the desktop and
// the phone; the core's `diagnose` keys its de-duplication the same way.

import type { SessionEvent } from "./api";

/** Whether a diagnostic is about something the agent has since replaced.
 *
 *  A later `revised` replaces the code, so every finding before it is
 *  about commits nobody is reviewing any more. A later `bundle` replaces
 *  the narrative, so a `bundle_*` finding before it is about a bundle
 *  nobody is reading. Either way Threshold checks the replacement, and
 *  writes a new finding if the problem is still there: a retired one is
 *  history, and showing it as current tells the reviewer a fixed bundle
 *  is still broken. */
export function isRetired(events: SessionEvent[], d: SessionEvent): boolean {
  const bundleFinding = (d.code ?? "").startsWith("bundle_");
  return events.some(
    (e) => e.seq > d.seq && e.from === "agent" && (e.kind === "revised" || (bundleFinding && e.kind === "bundle")),
  );
}

/** The diagnostics that still stand, in log order. */
export function currentDiagnostics(events: SessionEvent[]): SessionEvent[] {
  return events.filter((e) => e.kind === "diagnostic" && !isRetired(events, e));
}
