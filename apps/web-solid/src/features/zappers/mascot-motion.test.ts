import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Animator, gridFor, GREETING_MS, idlePose, raster, stillPose, type Cell, type Mood, type Pose } from "./mascot-motion";
import { mascots } from "./mascots";

type Sample = { mascot: string; mood: Mood; ms: number; look: [number, number]; grid: number; pose: number[]; cells: string[] };
const samples: Sample[] = JSON.parse(readFileSync(new URL("./mascot-parity.json", import.meta.url), "utf8"));
const numbers = (p: Pose) => [p.yaw, p.pitch, p.lift, p.gaze[0], p.gaze[1], p.blink, p.wide, p.lid, p.worry, p.smile];
const rows = (cells: Cell[], grid: number) =>
  Array.from({ length: grid }, (_, y) => cells.slice(y * grid, y * grid + grid).map(c => (c === "body" ? "b" : c === "eye" ? "e" : ".")).join(""));
const moods: Mood[] = ["awake", "resting", "offline", "working", "delegating", "needsYou", "blocked", "done"];
const key = (mood: Mood, ms: number) => rows(raster(mascots[0]!, 26, idlePose(mascots[0]!, mood, ms)), 26).join("");

describe("mascot motion", () => {
  test("draws exactly the pixels the desktop draws", () => {
    assert.ok(samples.length >= mascots.length + moods.length - 1);
    for (const sample of samples) {
      const mascot = mascots.find(m => m.id === sample.mascot)!;
      const pose = idlePose(mascot, sample.mood, sample.ms, sample.look, sample.look);
      numbers(pose).forEach((v, i) => assert.ok(Math.abs(v - sample.pose[i]!) < 2e-3, `${sample.mascot} ${sample.mood} pose[${i}]`));
      assert.deepEqual(rows(raster(mascot, sample.grid, pose), sample.grid), sample.cells, `${sample.mascot} ${sample.mood} at ${sample.grid}`);
    }
  });

  test("uses the desktop's grid sizes", () => {
    assert.deepEqual([16, 17, 24, 40, 64].map(gridFor), [16, 16, 16, 26, 42]);
  });

  test("quiet moods hold still between moments, busy ones step", () => {
    const changes = (mood: Mood) => Array.from({ length: 299 }, (_, i) => key(mood, (i + 1) * 33) !== key(mood, i * 33)).filter(Boolean).length;
    for (const mood of ["resting", "offline", "done"] as Mood[]) assert.equal(changes(mood), 0, mood);
    for (const mood of ["awake", "needsYou", "blocked", "working", "delegating"] as Mood[]) assert.ok(changes(mood) < 150, mood);
    assert.ok(changes("working") > 10);
  });

  test("still poses keep moods apart for reduced motion", () => {
    const seen = new Set(moods.filter(m => m !== "offline").map(mood => rows(raster(mascots[0]!, 26, stillPose(mood)), 26).join("")));
    assert.equal(seen.size, moods.length - 1);
  });

  test("changes of mood blend without jumps and never restart", () => {
    const mascot = mascots[0]!;
    const animator = new Animator("awake");
    assert.deepEqual(animator.pose(mascot, 5000), idlePose(mascot, "awake", 5000));
    const before = animator.pose(mascot, 10_000);
    animator.set(mascot, "working", 10_000);
    const start = animator.pose(mascot, 10_000);
    assert.ok(Math.abs(start.lid - before.lid) < 1e-6 && Math.abs(start.lift - before.lift) < 1e-6);
    animator.set(mascot, "working", 10_200);
    assert.ok(animator.changing(10_300));
    assert.ok(!animator.changing(10_000 + GREETING_MS));
    assert.deepEqual(animator.pose(mascot, 12_000), idlePose(mascot, "working", 12_000));
  });
});
