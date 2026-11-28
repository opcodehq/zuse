// The Zapper mascots' motion and outlines, ported from
// crates/protocol/src/presentation/zapper_mascots.rs so web and desktop draw the
// same faces. mascot-parity.json pins both to the same numbers.
import { blinkGaps, type Mascot } from "./mascots";

export type Mood = "awake" | "resting" | "offline" | "working" | "delegating" | "needsYou" | "blocked" | "done";
export type Point = [number, number];
export type Pose = {
  yaw: number; pitch: number; roll: number; lift: number; squash: number;
  gaze: Point; blink: number; wide: number; lid: number; worry: number; smile: number;
};
export type Outline = { body: Point[]; eyes: Point[][] };

export const moodLabel: Record<Mood, string> = {
  awake: "Idle", resting: "Resting", offline: "Offline", working: "Working",
  delegating: "Threads working", needsYou: "Needs you", blocked: "Needs attention", done: "Done",
};
export const busy = (mood: Mood) => mood === "working" || mood === "delegating";

export const LOOK_YAW = 12;
export const LOOK_PITCH = 8;
export const EYES_FOLLOW_MS = 72;
export const HEAD_FOLLOW_MS = 190;
export const TRANSITION_MS = 420;
export const GREETING_MS = 620;
const ZOOM = 1.16;
const EYE_SCALE = 1.25;
const BODY_POINTS = 72;
export const EYE_POINTS = 13;
const TAU = Math.PI * 2;

export const rest = (): Pose => ({ yaw: 0, pitch: 0, roll: 0, lift: 0, squash: 0, gaze: [0, 0], blink: 0, wide: 0, lid: 0, worry: 0, smile: 0 });
const pose = (over: Partial<Pose>): Pose => ({ ...rest(), ...over });

export function hash(text: string) {
  let value = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(text)) value = Math.imul(value ^ byte, 0x01000193) >>> 0;
  return value;
}

const easeQuick = (t: number) => 1 - (1 - t) ** 4;
const easeGentle = (t: number) => t * t * t * (t * (6 * t - 15) + 10);

export function mix(from: Pose, to: Pose, t: number): Pose {
  const m = (a: number, b: number) => a + (b - a) * t;
  return {
    yaw: m(from.yaw, to.yaw), pitch: m(from.pitch, to.pitch), roll: m(from.roll, to.roll),
    lift: m(from.lift, to.lift), squash: m(from.squash, to.squash),
    gaze: [m(from.gaze[0], to.gaze[0]), m(from.gaze[1], to.gaze[1])],
    blink: m(from.blink, to.blink), wide: m(from.wide, to.wide), lid: m(from.lid, to.lid),
    worry: m(from.worry, to.worry), smile: m(from.smile, to.smile),
  };
}

export function blinkAt(mascot: Mascot, ms: number) {
  const gaps = blinkGaps[mascot.temper];
  const length = mascot.temper === "sleepy" ? 320 : 205;
  const cycle = gaps.reduce((a, b) => a + b, 0);
  const t = (ms + (hash(mascot.id) % cycle)) % cycle;
  let end = 0;
  for (const gap of gaps) {
    end += gap;
    const span = Math.min(length, gap);
    if (t >= end - span && t < end) {
      const p = (t - (end - span)) / span;
      return p < 0.23 ? easeQuick(p / 0.23) : p < 0.39 ? 1 : 1 - easeGentle((p - 0.39) / 0.61);
    }
  }
  return 0;
}

function glance(mascot: Mascot, ms: number, period: number, travel: number, points: Point[]): Point {
  const t = ms + (hash(mascot.id) % period);
  const step = Math.floor(t / period);
  const k = easeGentle(Math.min((t % period) / travel, 1));
  const from = points[(step + points.length - 1) % points.length]!;
  const to = points[step % points.length]!;
  return [from[0] + (to[0] - from[0]) * k, from[1] + (to[1] - from[1]) * k];
}

function nowAndThen(mascot: Mascot, ms: number, period: number, length: number) {
  const t = (ms + (hash(mascot.id) % period)) % period;
  return t < length ? t / length : null;
}

const AROUND: Point[] = [[0, 0], [0.8, -0.2], [0, 0], [-0.7, 0.3], [0, 0], [0.3, 0.6]];
const WORK: Point[] = [[0, 0.3], [0.6, 0.35], [-0.5, 0.3], [0.1, 0]];

/** The pose at `ms` in `mood`; `eyes` and `head` are smoothed look targets in -1..1. */
export function idlePose(mascot: Mascot, mood: Mood, ms: number, eyes: Point = [0, 0], head: Point = [0, 0]): Pose {
  const s = ms / 1000 + (hash(mascot.id) % 1000) / 37;
  const sway = (rate: number, phase: number) => Math.sin(s * rate + phase);
  const looking = pose({
    yaw: head[0] * LOOK_YAW, pitch: head[1] * LOOK_PITCH, roll: head[0] * head[1] * 2.2,
    gaze: [eyes[0] * 0.08, eyes[1] * 0.06], blink: blinkAt(mascot, ms),
  });
  switch (mood) {
    case "awake": {
      const g = glance(mascot, ms, 3100, 260, AROUND);
      return { ...looking, yaw: looking.yaw + g[0] * 4, pitch: looking.pitch + g[1] * 3, gaze: [looking.gaze[0] + g[0] * 0.08, looking.gaze[1] + g[1] * 0.05] };
    }
    case "resting": return pose({ pitch: 5, blink: 1 });
    case "offline": return pose({ pitch: 2, blink: 1 });
    case "working": {
      const g = glance(mascot, ms, 1500, 170, WORK);
      const bounce = Math.abs(sway(3.8, 0));
      return {
        ...looking, yaw: g[0] * 5 + head[0] * LOOK_YAW * 0.5, pitch: g[1] * 3 + head[1] * LOOK_PITCH * 0.5,
        lift: -bounce * 0.07, squash: (1 - bounce) * 0.05 - 0.015,
        gaze: [g[0] * 0.08 + eyes[0] * 0.04, g[1] * 0.05 + eyes[1] * 0.03], wide: 0.04, lid: 0.38,
      };
    }
    case "delegating": {
      const sweep = sway(1.9, 0);
      return { ...looking, yaw: sway(1.9, -0.6) * 5 + head[0] * LOOK_YAW * 0.4, gaze: [sweep * 0.11 + eyes[0] * 0.03, looking.gaze[1]], lid: 0.2 };
    }
    case "needsYou": {
      const out = { ...looking, pitch: looking.pitch - 3, gaze: [eyes[0] * 0.11, eyes[1] * 0.08] as Point, wide: 0.3 };
      const p = nowAndThen(mascot, ms, 2400, 380);
      if (p !== null) {
        const hop = Math.sin(p * Math.PI);
        out.lift = -hop * 0.12; out.squash = -hop * 0.05; out.blink = 0;
      }
      return out;
    }
    case "blocked": {
      const out = { ...looking, pitch: looking.pitch + 4, gaze: [looking.gaze[0], looking.gaze[1] + 0.04] as Point, lid: 0.1, worry: 1 };
      const p = nowAndThen(mascot, ms, 3400, 520);
      if (p !== null) out.yaw += Math.sin(p * TAU * 2) * 6 * (1 - p);
      return out;
    }
    case "done": return { ...looking, pitch: looking.pitch - 2, smile: 1, blink: 0 };
  }
}

/** A still pose that still says what the Zapper is doing, for reduced motion. */
export function stillPose(mood: Mood): Pose {
  switch (mood) {
    case "awake": return rest();
    case "resting": return pose({ pitch: 5, blink: 1 });
    case "offline": return pose({ pitch: 2, blink: 1 });
    case "working": return pose({ wide: 0.04, lid: 0.38, gaze: [0, 0.04] });
    case "delegating": return pose({ yaw: 4, gaze: [0.1, 0], lid: 0.2 });
    case "needsYou": return pose({ wide: 0.3, pitch: -5 });
    case "blocked": return pose({ pitch: 4, gaze: [0, 0.04], lid: 0.1, worry: 1 });
    case "done": return pose({ pitch: -2, smile: 1 });
  }
}

function greet(from: Mood, to: Mood, p: number, out: Pose) {
  const arc = Math.sin(p * Math.PI);
  if ((to === "working" || to === "delegating") && !busy(from)) {
    out.lift -= arc * 0.08; out.squash -= arc * 0.05; out.wide += arc * 0.25 * (1 - p);
    out.lid *= p; out.blink *= p;
  } else if (to === "done") {
    out.lift -= arc * 0.06; out.squash -= arc * 0.03;
  } else if (to === "needsYou") {
    out.lift -= arc * 0.07; out.wide += arc * 0.1;
  } else if (to === "blocked") {
    out.yaw += Math.sin(p * TAU * 2) * 7 * (1 - p);
  }
}

/** One mascot's mood over time: blends each change from wherever it was and greets the new mood. */
export class Animator {
  mood: Mood;
  private previous: Mood;
  private from: Pose | null = null;
  private changed = 0;
  constructor(mood: Mood) { this.mood = mood; this.previous = mood; }
  set(mascot: Mascot, mood: Mood, ms: number, eyes: Point = [0, 0], head: Point = [0, 0]) {
    if (mood === this.mood) return;
    this.from = this.pose(mascot, ms, eyes, head);
    this.previous = this.mood;
    this.mood = mood;
    this.changed = ms;
  }
  changing(ms: number) { return this.from !== null && ms - this.changed < Math.max(TRANSITION_MS, GREETING_MS); }
  pose(mascot: Mascot, ms: number, eyes: Point = [0, 0], head: Point = [0, 0]): Pose {
    let out = idlePose(mascot, this.mood, ms, eyes, head);
    if (!this.from) return out;
    const since = Math.max(0, ms - this.changed);
    if (since < TRANSITION_MS) out = mix(this.from, out, easeGentle(since / TRANSITION_MS));
    if (since < GREETING_MS) greet(this.previous, this.mood, since / GREETING_MS, out);
    return out;
  }
}

/** The thinnest an eye may get, in tile units, for an avatar `size` CSS pixels across. */
export const lineFor = (size: number) => Math.min(0.2, Math.max(0.03, 1.3 / (size / 2) / ZOOM));

const power = (v: number, n: number) => Math.sign(v) * Math.abs(v) ** (2 / n);

/** `mascot` in `pose` as filled outlines; the tile spans -1..1, y down. */
export function outline(mascot: Mascot, p: Pose, line: number): Outline {
  const rad = Math.PI / 180;
  const [yaw, pitch, roll] = [p.yaw * rad, p.pitch * rad, p.roll * rad];
  const centreY = mascot.dy + p.lift;
  const [sin, cos] = [Math.sin(roll), Math.cos(roll)];
  const place = (x0: number, y0: number): Point => {
    const [x, y] = [x0 * (1 + p.squash * 0.5), y0 * (1 - p.squash)];
    return [(x * cos - y * sin) * ZOOM, (x * sin + y * cos + centreY) * ZOOM];
  };
  const a = mascot.a * (1 - 0.08 * Math.sin(yaw) ** 2);
  const halfAt = (y: number) => a * (1 - mascot.taper * (0.5 - 0.5 * (y / mascot.b)));
  const body: Point[] = [];
  for (let i = 0; i < BODY_POINTS; i++) {
    const t = (i / BODY_POINTS) * TAU;
    const y = mascot.b * power(Math.sin(t), mascot.n);
    body.push(place(halfAt(y) * power(Math.cos(t), mascot.n), y));
  }
  const depth = 0.9 * Math.min(mascot.a, mascot.b);
  const eyes: Point[][] = [];
  for (const eye of mascot.eyes) {
    const [u, v] = [eye.x / mascot.a, eye.y / mascot.b];
    const z0 = depth * Math.sqrt(Math.max(1 - u * u - v * v, 0.05));
    const [x1, z1] = [eye.x * Math.cos(yaw) + z0 * Math.sin(yaw), -eye.x * Math.sin(yaw) + z0 * Math.cos(yaw)];
    const [y2, z2] = [eye.y * Math.cos(pitch) + z1 * Math.sin(pitch), -eye.y * Math.sin(pitch) + z1 * Math.cos(pitch)];
    if (z2 < 0.12 * depth) continue;
    const squeeze = (turned: number, still: number) => Math.min(1.2, Math.max(0.6, Math.cos(turned) / Math.cos(still)));
    const grow = EYE_SCALE * (1 + p.wide);
    const w = eye.w * grow * squeeze(Math.atan2(x1, z1), Math.atan2(eye.x, z0));
    const h = eye.h * grow * squeeze(Math.atan2(y2, z2), Math.atan2(eye.y, z0));
    const fits = (cx: number, cy: number) => [[-1, -1], [1, -1], [-1, 1], [1, 1], [0, -1], [0, 1], [-1, 0], [1, 0]].every(([sx, sy]) => {
      const [x, y] = [cx + sx! * w * 0.42, cy + sy! * h * 0.42];
      const half = a * (1 - mascot.taper * (0.5 - 0.5 * Math.min(1, Math.max(-1, y / mascot.b))));
      return Math.abs(x / half) ** mascot.n + Math.abs(y / mascot.b) ** mascot.n <= 0.9;
    });
    let [cx, cy] = [x1 + p.gaze[0], y2 + p.gaze[1]];
    if (!fits(cx, cy)) {
      let [lo, hi] = [0, 1];
      for (let i = 0; i < 10; i++) {
        const k = (lo + hi) / 2;
        if (fits(cx * k, cy * k)) lo = k; else hi = k;
      }
      [cx, cy] = [cx * lo, cy * lo];
    }
    const [top, bottom] = eyeEdges(eye.shape, w, h, p, line, eye.x < 0 ? -1 : 1);
    eyes.push([...top, ...bottom.reverse()].map(([x, y]) => place(cx + x, cy + y)));
  }
  return { body, eyes };
}

function eyeEdges(shape: "capsule" | "oval", w: number, h: number, p: Pose, line: number, outward: number): [Point[], Point[]] {
  const [rx, ry] = [w / 2, h / 2];
  const half = (x0: number) => {
    const x = Math.min(Math.abs(x0), rx);
    if (shape === "oval") return ry * Math.sqrt(Math.max(1 - (x / rx) ** 2, 0));
    if (ry >= rx) return ry - rx + Math.sqrt(Math.max(rx * rx - x * x, 0));
    const straight = rx - ry;
    return x <= straight ? ry : Math.sqrt(Math.max(ry * ry - (x - straight) ** 2, 0));
  };
  const blink = Math.min(1, Math.max(0, p.blink));
  const open = 1 - blink;
  const smile = Math.min(1, Math.max(0, p.smile));
  const wide = Math.max(rx, ry * 0.8);
  const arch = wide * 0.85;
  const band = Math.max(h * 0.24, line * 1.5);
  const top: Point[] = [];
  const bottom: Point[] = [];
  for (let i = 0; i < EYE_POINTS; i++) {
    const u = -Math.cos((Math.PI * i) / (EYE_POINTS - 1));
    let x = u * rx;
    const across = Math.sqrt(Math.max(1 - u * u, 0));
    const thin = line * 0.5 * Math.sqrt(across);
    const drop = ry * 0.25 * blink;
    const reach = Math.max(half(x) * open, thin);
    let t = drop - reach;
    let b = drop + reach;
    const lid = ry * open * (-1 + p.lid + p.worry * (0.1 + 0.4 * outward * u));
    t = Math.min(Math.max(t, lid), b - 2 * thin);
    if (smile > 0) {
      const arcTop = arch * (0.4 - across);
      const arcBottom = arcTop + band * (0.5 + 0.5 * Math.sqrt(across));
      x += (u * wide - x) * smile;
      t += (arcTop - t) * smile;
      b += (arcBottom - b) * smile;
    }
    top.push([x, t]);
    bottom.push([x, b]);
  }
  return [top, bottom];
}

/** SVG path data for an outline at `size` pixels, rounded to a hundredth of a pixel. */
export function pathData(points: Point[], size: number) {
  const f = (v: number) => Math.round(((v + 1) / 2) * size * 100) / 100;
  return points.map(([x, y], i) => `${i ? "L" : "M"}${f(x)} ${f(y)}`).join("") + "Z";
}
