// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import type { ReviewSummary } from "./api";
import { arrange, projectsIn } from "./inbox.ts";

let n = 0;
function review(o: Partial<ReviewSummary>): ReviewSummary {
  n++;
  return { path: `/inbox/${n}.jsonl`, title: `r${n}`, repo: "/code/shop", revision: null, decided: false, last_at_ms: n * 1000, events: 1, spoken: 1, archived: false, ...o };
}

const titles = (sections: ReturnType<typeof arrange>) => sections.map((s) => [s.title, s.reviews.map((r) => r.title)]);

test("by date: waiting then decided, newest first, archived left out", () => {
  const list = [
    review({ title: "old waiting", last_at_ms: 1 }),
    review({ title: "new waiting", last_at_ms: 9 }),
    review({ title: "decided", decided: true, last_at_ms: 20 }),
    review({ title: "put away", archived: true, last_at_ms: 30 }),
  ];
  assert.deepEqual(titles(arrange(list, "inbox", "date", null)), [
    ["Waiting · 2", ["new waiting", "old waiting"]],
    ["Decided", ["decided"]],
  ]);
});

test("the archived view holds only archived reviews, waiting or decided alike", () => {
  const list = [
    review({ title: "inbox" }),
    review({ title: "archived undecided", archived: true, last_at_ms: 5 }),
    review({ title: "archived decided", archived: true, decided: true, last_at_ms: 6 }),
  ];
  assert.deepEqual(titles(arrange(list, "archived", "date", null)), [["Archived · 2", ["archived decided", "archived undecided"]]]);
});

test("by project: one section per repo, the most recently active project first", () => {
  const list = [
    review({ title: "acme old", repo: "/code/acme", last_at_ms: 1 }),
    review({ title: "shop", repo: "/code/shop", last_at_ms: 5 }),
    review({ title: "acme decided new", repo: "/code/acme", decided: true, last_at_ms: 9 }),
    review({ title: "acme waiting", repo: "/code/acme", last_at_ms: 3 }),
  ];
  assert.deepEqual(titles(arrange(list, "inbox", "project", null)), [
    ["acme", ["acme waiting", "acme old", "acme decided new"]],
    ["shop", ["shop"]],
  ]);
});

test("two projects sharing a directory name are told apart by path", () => {
  const list = [review({ repo: "/work/api" }), review({ repo: "/clients/acme/api" })];
  const sections = arrange(list, "inbox", "project", null);
  assert.deepEqual(sections.map((s) => [s.title, s.detail]).sort(), [["api", "/clients/acme/api"], ["api", "/work/api"]]);
});

test("filtering to a project narrows either grouping, and a review without a repo is its own project", () => {
  const list = [review({ title: "a", repo: "/code/acme" }), review({ title: "b", repo: "/code/shop" }), review({ title: "loose", repo: null })];
  assert.deepEqual(titles(arrange(list, "inbox", "date", "/code/acme")), [["Waiting · 1", ["a"]]]);
  assert.deepEqual(titles(arrange(list, "inbox", "project", "")), [["No project", ["loose"]]]);
});

test("the project list counts what is in the view being looked at", () => {
  const list = [review({ repo: "/code/acme" }), review({ repo: "/code/acme", archived: true }), review({ repo: "/code/shop" })];
  assert.deepEqual(projectsIn(list, "inbox"), [
    { repo: "/code/acme", name: "acme", count: 1 },
    { repo: "/code/shop", name: "shop", count: 1 },
  ]);
  assert.deepEqual(projectsIn(list, "archived"), [{ repo: "/code/acme", name: "acme", count: 1 }]);
});
