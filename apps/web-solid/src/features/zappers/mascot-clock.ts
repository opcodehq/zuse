// One animation loop for every mascot on the page. It only runs while a
// mascot is on screen, the tab is visible and motion is allowed, and writes
// an outline to the DOM only when it would draw differently.
import { Animator, EYES_FOLLOW_MS, HEAD_FOLLOW_MS, lineFor, outline, stillPose, type Mood, type Point } from "./mascot-motion";
import type { Mascot } from "./mascots";

export type Face = {
  svg: SVGSVGElement;
  paths: SVGPathElement[];
  mascot: Mascot;
  size: number;
  animator: Animator;
  eyes: Point;
  head: Point;
  target: Point;
  visible: boolean;
  drawn: string;
};

/** How far the pointer must be, in CSS pixels, for a full turn. */
const LOOK_REACH = 360;
const live = new Set<Face>();
const epoch = typeof performance === "undefined" ? 0 : performance.now();
let frame: number | null = null;
let last = 0;
let pointer: Point | null = null;
let pointerMoved = false;
let observer: IntersectionObserver | null = null;
let listening = false;

export const reducedMotion = () => typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
export const clock = () => (typeof performance === "undefined" ? Date.now() : performance.now()) - epoch;
/** Whether the loop is scheduled; for tests. */
export const running = () => frame !== null;
export const liveCount = () => live.size;

const px = (v: number, size: number) => Math.round(((v + 1) / 2) * size * 8) / 8;
function path(points: Point[], size: number) {
  return points.map(([x, y], i) => `${i ? "L" : "M"}${px(x, size)} ${px(y, size)}`).join("") + "Z";
}

/** Paint `face` at `ms`, touching the DOM only when the outline changed. */
export function draw(face: Face, ms = clock()) {
  const pose = reducedMotion() ? stillPose(face.animator.mood) : face.animator.pose(face.mascot, ms, face.eyes, face.head);
  const shape = outline(face.mascot, pose, lineFor(face.size));
  const data = [path(shape.body, face.size), ...face.paths.slice(1).map((_, i) => (shape.eyes[i] ? path(shape.eyes[i]!, face.size) : ""))];
  const key = data.join("|");
  if (key === face.drawn) return false;
  face.drawn = key;
  face.paths.forEach((element, i) => element.setAttribute("d", data[i]!));
  return true;
}

function tick(now: number) {
  frame = null;
  const dt = last ? Math.min(64, now - last) : 16;
  last = now;
  const ms = now - epoch;
  for (const face of live) {
    if (!face.visible) continue;
    if (pointerMoved) {
      const box = face.svg.getBoundingClientRect();
      face.target = pointer
        ? [
            Math.max(-1, Math.min(1, (pointer[0] - (box.left + box.width / 2)) / LOOK_REACH)),
            Math.max(-1, Math.min(1, (pointer[1] - (box.top + box.height / 2)) / LOOK_REACH)),
          ]
        : [0, 0];
    }
    // Eased by elapsed time, so the follow feels the same at any frame rate.
    const ease = (from: Point, span: number): Point => {
      const k = 1 - Math.exp(-dt / span);
      return [from[0] + (face.target[0] - from[0]) * k, from[1] + (face.target[1] - from[1]) * k];
    };
    face.eyes = ease(face.eyes, EYES_FOLLOW_MS);
    face.head = ease(face.head, HEAD_FOLLOW_MS);
    draw(face, ms);
  }
  pointerMoved = false;
  wake();
}

/** Start the loop if anything visible could move; otherwise let it stop. */
export function wake() {
  if (frame !== null) return;
  const hidden = typeof document !== "undefined" && document.hidden;
  if (hidden || reducedMotion() || ![...live].some(face => face.visible) || typeof requestAnimationFrame !== "function") {
    last = 0;
    return;
  }
  frame = requestAnimationFrame(tick);
}

function listen() {
  if (listening || typeof window === "undefined") return;
  listening = true;
  window.addEventListener("pointermove", event => { pointer = [event.clientX, event.clientY]; pointerMoved = true; }, { passive: true });
  document.documentElement.addEventListener("pointerleave", () => { pointer = null; pointerMoved = true; });
  window.addEventListener("blur", () => { pointer = null; pointerMoved = true; });
  document.addEventListener("visibilitychange", wake);
  if (typeof matchMedia === "function") matchMedia("(prefers-reduced-motion: reduce)").addEventListener("change", () => {
    for (const face of live) draw(face);
    wake();
  });
  if (typeof IntersectionObserver === "function") observer = new IntersectionObserver(entries => {
    for (const entry of entries) {
      for (const face of live) if (face.svg === entry.target) face.visible = entry.isIntersecting;
    }
    wake();
  });
}

/** Animate a mascot until the returned function is called. */
export function attach(svg: SVGSVGElement, paths: SVGPathElement[], mascot: Mascot, size: number, mood: Mood): Face & { detach: () => void; update: (mascot: Mascot, size: number, mood: Mood) => void } {
  listen();
  const face: Face = { svg, paths, mascot, size, animator: new Animator(mood), eyes: [0, 0], head: [0, 0], target: [0, 0], visible: !observer, drawn: "" };
  live.add(face);
  observer?.observe(svg);
  draw(face);
  wake();
  return Object.assign(face, {
    detach: () => {
      live.delete(face);
      observer?.unobserve(svg);
      if (frame !== null && ![...live].some(other => other.visible) && typeof cancelAnimationFrame === "function") {
        cancelAnimationFrame(frame);
        frame = null;
        last = 0;
      }
    },
    update: (next: Mascot, nextSize: number, nextMood: Mood) => {
      if (next !== face.mascot) face.animator = new Animator(nextMood);
      face.mascot = next;
      face.size = nextSize;
      face.animator.set(next, nextMood, clock(), face.eyes, face.head);
      draw(face);
      wake();
    },
  });
}
