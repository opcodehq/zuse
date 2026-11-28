import { describe, test } from "node:test";
import assert from "node:assert/strict";

// A minimal page: animation frames run only when the test steps them.
const frames = new Map<number, (now: number) => void>();
let nextFrame = 1;
let reduce = false;
const motionListeners: (() => void)[] = [];
const observed = new Map<unknown, (visible: boolean) => void>();
const page = {
  hidden: false,
  listeners: new Map<string, () => void>(),
  addEventListener(type: string, fn: () => void) { this.listeners.set(type, fn); },
  documentElement: { addEventListener() {} },
};
Object.assign(globalThis, {
  window: { addEventListener() {} },
  document: page,
  requestAnimationFrame: (fn: (now: number) => void) => { frames.set(nextFrame, fn); return nextFrame++; },
  cancelAnimationFrame: (id: number) => { frames.delete(id); },
  matchMedia: () => ({ get matches() { return reduce; }, addEventListener: (_: string, fn: () => void) => motionListeners.push(fn) }),
  IntersectionObserver: class {
    constructor(private report: (entries: { target: unknown; isIntersecting: boolean }[]) => void) {}
    observe(target: unknown) { observed.set(target, visible => this.report([{ target, isIntersecting: visible }])); }
    unobserve(target: unknown) { observed.delete(target); }
  },
});
const { attach, running, liveCount } = await import("./mascot-clock");
const { mascots } = await import("./mascots");

const element = () => ({ d: "", setAttribute(_: string, value: string) { this.d = value; }, getBoundingClientRect: () => ({ left: 0, top: 0, width: 17, height: 17 }) });
const step = (now: number) => { const pending = [...frames.values()]; frames.clear(); pending.forEach(fn => fn(now)); };
const mount = (mood: "awake" | "working" = "working") => {
  const svg = element();
  const paths = [element(), element()];
  const face = attach(svg as unknown as SVGSVGElement, paths as unknown as SVGPathElement[], mascots[0]!, 17, mood);
  return { svg, paths, face };
};

describe("mascot clock", () => {
  test("draws at once, animates only while visible, and stops when unmounted", () => {
    const { svg, paths, face } = mount();
    assert.ok(paths[0]!.d.startsWith("M") && paths[1]!.d.startsWith("M"), "first paint is synchronous");
    assert.equal(running(), false, "nothing runs before it is on screen");
    observed.get(svg)!(true);
    assert.equal(running(), true);
    const before = paths[0]!.d;
    step(1000); step(1250);
    assert.notEqual(paths[0]!.d, before, "a working mascot moves");
    observed.get(svg)!(false);
    step(1300);
    assert.equal(running(), false, "scrolled away: the loop stops");
    observed.get(svg)!(true);
    face.detach();
    assert.equal(running(), false, "unmounted: the loop stops");
    assert.equal(liveCount(), 0);
    assert.equal(observed.has(svg), false);
  });

  test("a hidden tab stops the loop and a visible one resumes it", () => {
    const { svg, face } = mount();
    observed.get(svg)!(true);
    step(2000);
    page.hidden = true;
    step(2033);
    assert.equal(running(), false);
    page.hidden = false;
    page.listeners.get("visibilitychange")!();
    assert.equal(running(), true);
    face.detach();
  });

  test("reduced motion shows a still face for the mood and never animates", () => {
    const { svg, paths, face } = mount();
    observed.get(svg)!(true);
    reduce = true;
    motionListeners.forEach(fn => fn());
    step(3000);
    assert.equal(running(), false);
    const still = paths.map(p => p.d).join("|");
    face.update(mascots[0]!, 17, "awake");
    assert.notEqual(paths.map(p => p.d).join("|"), still, "a new mood still shows");
    assert.equal(running(), false);
    reduce = false;
    face.detach();
  });

  test("re-rendering with the same mood never restarts a transition", () => {
    const { svg, face } = mount("awake");
    observed.get(svg)!(true);
    face.update(mascots[0]!, 17, "working");
    const animator = face.animator;
    const pose = JSON.stringify(animator.pose(mascots[0]!, 10_000));
    face.update(mascots[0]!, 17, "working");
    assert.equal(face.animator, animator);
    assert.equal(JSON.stringify(animator.pose(mascots[0]!, 10_000)), pose);
    face.detach();
  });
});
