import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Animator, GREETING_MS, idlePose, lineFor, outline, stillPose, type Mood, type Pose } from "./mascot-motion";
import { mascots } from "./mascots";

type Sample = { mascot: string; mood: Mood; ms: number; look: [number, number]; line: number; pose: number[]; body: number[][]; eyes: number[][][] };
const samples: Sample[] = JSON.parse(readFileSync(new URL("./mascot-parity.json", import.meta.url), "utf8"));
const numbers = (p: Pose) => [p.yaw, p.pitch, p.roll, p.lift, p.squash, p.gaze[0], p.gaze[1], p.blink, p.wide, p.lid, p.worry, p.smile];
const close = (a: number[], b: number[], what: string) => {
  assert.equal(a.length, b.length, what);
  a.forEach((v, i) => assert.ok(Math.abs(v - b[i]!) < 2e-3, `${what}[${i}]: ${v} vs ${b[i]}`));
};
const moods: Mood[] = ["awake", "resting", "offline", "working", "delegating", "needsYou", "blocked", "done"];
const key = (mascot = mascots[0]!, mood: Mood, ms: number) =>
  JSON.stringify(outline(mascot, idlePose(mascot, mood, ms), 0.08).body.concat(...outline(mascot, idlePose(mascot, mood, ms), 0.08).eyes).map(([x, y]) => [Math.round(x * 64), Math.round(y * 64)]));

describe("mascot motion", () => {
  test("draws exactly what the desktop draws", () => {
    assert.ok(samples.length >= mascots.length + moods.length - 1);
    for (const sample of samples) {
      const mascot = mascots.find(m => m.id === sample.mascot)!;
      const pose = idlePose(mascot, sample.mood, sample.ms, sample.look, sample.look);
      const what = `${sample.mascot} ${sample.mood}`;
      close(numbers(pose), sample.pose, `${what} pose`);
      const shape = outline(mascot, pose, sample.line);
      close(shape.body.filter((_, i) => i % 4 === 0).flat(), sample.body.flat(), `${what} body`);
      close(shape.eyes.flatMap(eye => eye.filter((_, i) => i % 2 === 0).flat()), sample.eyes.flat(2), `${what} eyes`);
    }
  });

  test("quiet moods hold still between moments, busy ones move", () => {
    const changes = (mood: Mood) => Array.from({ length: 299 }, (_, i) => key(undefined, mood, (i + 1) * 33) !== key(undefined, mood, i * 33)).filter(Boolean).length;
    for (const mood of ["resting", "offline", "done"] as Mood[]) assert.equal(changes(mood), 0, mood);
    for (const mood of ["awake", "needsYou", "blocked"] as Mood[]) assert.ok(changes(mood) < 100, mood);
    assert.ok(changes("working") > 200);
  });

  test("still poses keep every mood distinct for reduced motion", () => {
    const seen = new Set(moods.map(mood => JSON.stringify(outline(mascots[0]!, stillPose(mood), lineFor(17)))));
    assert.equal(seen.size, moods.length);
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
