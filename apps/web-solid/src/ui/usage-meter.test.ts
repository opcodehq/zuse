import assert from "node:assert/strict";
import { test } from "node:test";
import { percentLeft, usageResetLabel } from "./usage-meter";

test("allowance left is rounded, bounded, and distinguishes missing usage", () => {
  assert.equal(percentLeft(0), 100);
  assert.equal(percentLeft(0.382), 62);
  assert.equal(percentLeft(0.95), 5);
  assert.equal(percentLeft(1), 0);
  assert.equal(percentLeft(-0.2), 100);
  assert.equal(percentLeft(1.2), 0);
  assert.equal(percentLeft(undefined), null);
  assert.equal(percentLeft(NaN), null);
  assert.equal(percentLeft(Infinity), null);
});

test("reset copy covers upcoming, elapsed, and unavailable reset times", () => {
  const now = Date.UTC(2026, 10, 25, 12);
  const afterMinutes = (minutes: number) => usageResetLabel(now + minutes * 60_000, now);
  assert.equal(afterMinutes(2 * 1440 + 3 * 60 + 1), "Resets in 2d 3h");
  assert.equal(afterMinutes(62), "Resets in 1h 02m");
  assert.equal(afterMinutes(17), "Resets in 17m");
  assert.equal(afterMinutes(0.5), "Resets in <1m");
  assert.equal(afterMinutes(0), "Reset time passed");
  assert.equal(afterMinutes(-120), "Reset time passed");
  assert.equal(usageResetLabel(NaN, now), "Reset unavailable");
});
