// Arranging the review list: which reviews, in what sections.
//
// Three independent choices, each narrowing or regrouping the same list
// rather than hiding anything silently:
//
//   - the view: the inbox, or what has been archived;
//   - a project filter: one repository, or all of them;
//   - the grouping: by date (waiting, then decided, newest first — the
//     default), or by project, each project ordered by its most recent
//     activity so the one being worked on is at the top.
//
// Free of the DOM so it is tested on its own.

import type { ReviewSummary } from "./api";

export type InboxView = "inbox" | "archived";
export type Grouping = "date" | "project";

export interface Section {
  /** Stable key for the section, e.g. the repo path. */
  key: string;
  title: string;
  /** A second line of the title, to tell two same-named projects apart. */
  detail?: string;
  reviews: ReviewSummary[];
}

/** What a review with no repo on its brief is filed under. */
export const NO_PROJECT = "";

export function projectOf(r: ReviewSummary): string {
  return r.repo ?? NO_PROJECT;
}

/** A project's short name: the repo directory's own name. */
export function projectName(repo: string): string {
  if (repo === NO_PROJECT) return "No project";
  const parts = repo.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? repo;
}

/** Newest activity first; ties broken by path so the order is stable. */
function newestFirst(a: ReviewSummary, b: ReviewSummary): number {
  return b.last_at_ms - a.last_at_ms || a.path.localeCompare(b.path);
}

function latest(reviews: ReviewSummary[]): number {
  return Math.max(...reviews.map((r) => r.last_at_ms));
}

/** Waiting before decided, then newest first. */
function waitingFirst(a: ReviewSummary, b: ReviewSummary): number {
  return Number(a.decided) - Number(b.decided) || newestFirst(a, b);
}

/** The projects in a view, with how many reviews each has there, by name. */
export function projectsIn(reviews: ReviewSummary[], view: InboxView): { repo: string; name: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const r of reviews) {
    if (r.archived !== (view === "archived")) continue;
    counts.set(projectOf(r), (counts.get(projectOf(r)) ?? 0) + 1);
  }
  return [...counts]
    .map(([repo, count]) => ({ repo, name: projectName(repo), count }))
    .sort((a, b) => a.name.localeCompare(b.name) || a.repo.localeCompare(b.repo));
}

export function arrange(reviews: ReviewSummary[], view: InboxView, grouping: Grouping, project: string | null): Section[] {
  const shown = reviews.filter((r) => r.archived === (view === "archived") && (project == null || projectOf(r) === project));

  if (grouping === "project") {
    const byRepo = new Map<string, ReviewSummary[]>();
    for (const r of shown) {
      const list = byRepo.get(projectOf(r)) ?? [];
      list.push(r);
      byRepo.set(projectOf(r), list);
    }
    // Two repos with the same directory name are told apart by their path.
    const nameCounts = new Map<string, number>();
    for (const repo of byRepo.keys()) nameCounts.set(projectName(repo), (nameCounts.get(projectName(repo)) ?? 0) + 1);
    return [...byRepo]
      .map(([repo, list]) => ({
        key: repo,
        title: projectName(repo),
        detail: (nameCounts.get(projectName(repo)) ?? 0) > 1 ? repo : undefined,
        reviews: list.sort(waitingFirst),
      }))
      // By the project's newest activity, not its first listed review: a
      // waiting review listed first can be older than a decision just made.
      .sort((a, b) => latest(b.reviews) - latest(a.reviews) || a.key.localeCompare(b.key));
  }

  if (view === "archived") {
    return shown.length ? [{ key: "archived", title: `Archived · ${shown.length}`, reviews: [...shown].sort(newestFirst) }] : [];
  }
  const waiting = shown.filter((r) => !r.decided).sort(newestFirst);
  const decided = shown.filter((r) => r.decided).sort(newestFirst);
  const out: Section[] = [];
  if (waiting.length) out.push({ key: "waiting", title: `Waiting · ${waiting.length}`, reviews: waiting });
  if (decided.length) out.push({ key: "decided", title: "Decided", reviews: decided });
  return out;
}
