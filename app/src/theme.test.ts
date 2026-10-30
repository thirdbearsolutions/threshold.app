// Run with: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { isDark, resolve, savedChoice } from "./theme.ts";

test("System follows macOS, and the others ignore it", () => {
  assert.equal(resolve("system", false), "light");
  assert.equal(resolve("system", true), "dark");
  assert.equal(resolve("threshold", false), "threshold");
  assert.equal(resolve("light", true), "light");
});

test("Threshold draws on a dark ground", () => {
  assert.equal(isDark("threshold"), true);
  assert.equal(isDark("light"), false);
});

test("an unknown or missing value counts as never chosen", () => {
  const store = (v: string | null) => ({ getItem: () => v });
  assert.equal(savedChoice(store(null)), null);
  assert.equal(savedChoice(store("solarized")), null);
  assert.equal(savedChoice(store("threshold")), "threshold");
  assert.equal(savedChoice({ getItem: () => { throw new Error("denied"); } }), null);
});
