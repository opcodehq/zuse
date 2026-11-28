// The Zapper mascots' motion and pixel raster, ported from
// crates/protocol/src/presentation/zapper_mascots.rs so web and desktop draw the
// same faces. mascot-parity.json pins both to the same numbers.
import { blinkGaps, type Mascot } from "./mascots";

export type Mood = "awake" | "resting" | "offline" | "working" | "delegating" | "needsYou" | "blocked" | "done";
export type Point = [number, number];
export type Pose = {
  yaw: number; pitch: number; lift: number;
  gaze: Point; blink: number; wide: number; lid: number; worry: number; smile: number;
};

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
const TAU = Math.PI * 2;

export const rest = (): Pose => ({ yaw: 0, pitch: 0, lift: 0, gaze: [0, 0], blink: 0, wide: 0, lid: 0, worry: 0, smile: 0 });
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
    yaw: m(from.yaw, to.yaw), pitch: m(from.pitch, to.pitch), lift: m(from.lift, to.lift),
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
    yaw: head[0] * LOOK_YAW, pitch: head[1] * LOOK_PITCH,
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
        lift: -bounce * 0.07,
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
        out.lift = -hop * 0.12; out.blink = 0;
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
    out.lift -= arc * 0.08; out.wide += arc * 0.25 * (1 - p);
    out.lid *= p; out.blink *= p;
  } else if (to === "done") {
    out.lift -= arc * 0.06;
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

export type Cell = "body" | "eye" | null;

/** Cells across for an avatar `size` pixels wide; matches grid_for on desktop. */
export const gridFor = (size: number) => Math.floor(Math.min(44, Math.max(16, roundAway(size / 1.5))) / 2) * 2;

/** Rust's rounding: halves go away from zero. */
function roundAway(v: number) {
  return Math.sign(v) * Math.round(Math.abs(v));
}

/** `mascot` in `p` on a `grid` x `grid` raster, row by row, in whole cells. */
export function raster(mascot: Mascot, grid: number, p: Pose): Cell[] {
  const f = Math.fround;
  const rad = Math.PI / 180;
  const [yaw, pitch] = [p.yaw * rad, p.pitch * rad];
  const g = grid;
  const cell = 2 / g / ZOOM;
  const depth = 0.9 * Math.min(mascot.a, mascot.b);
  const a = mascot.a * (1 - 0.08 * Math.sin(yaw) ** 2);
  const centreY = mascot.dy + roundAway(p.lift / cell) * cell;
  const inside = (qx: number, qy: number) => {
    const along = Math.min(1, Math.max(-1, qy / mascot.b));
    const half = a * (1 - mascot.taper * (0.5 - 0.5 * along));
    return f(Math.abs(qx / half) ** mascot.n + Math.abs(qy / mascot.b) ** mascot.n) <= 1;
  };
  const out: Cell[] = new Array(grid * grid).fill(null);
  for (let y = 0; y < grid; y++) {
    for (let x = 0; x < grid; x++) {
      const px = (((x + 0.5) / g) * 2 - 1) / ZOOM;
      const py = (((y + 0.5) / g) * 2 - 1) / ZOOM - centreY;
      if (inside(px, py)) out[y * grid + x] = "body";
    }
  }
  const bodies = out.map(c => c !== null);
  const solid = (x: number, y: number) => x >= 0 && y >= 0 && x < grid && y < grid && bodies[y * grid + x]!;
  const minSide = (2 / g) * 1.05 / ZOOM;
  const owner = new Array<number>(grid * grid).fill(0);
  for (const [n, eye] of mascot.eyes.entries()) {
    const id = n + 1;
    const [u, v] = [eye.x / mascot.a, eye.y / mascot.b];
    const z0 = depth * Math.sqrt(Math.max(1 - u * u - v * v, 0.05));
    const [x1, z1] = [eye.x * Math.cos(yaw) + z0 * Math.sin(yaw), -eye.x * Math.sin(yaw) + z0 * Math.cos(yaw)];
    const [y2, z2] = [eye.y * Math.cos(pitch) + z1 * Math.sin(pitch), -eye.y * Math.sin(pitch) + z1 * Math.cos(pitch)];
    if (z2 < 0.12 * depth) continue;
    const squeeze = (turned: number, still: number) => Math.min(1.2, Math.max(0.6, Math.cos(turned) / Math.cos(still)));
    const grow = 1 + p.wide;
    const shut = p.blink > 0.8;
    const slit = (eye.h * ZOOM * g) / 2 < 2.5;
    const w = Math.max(eye.w * grow * squeeze(Math.atan2(x1, z1), Math.atan2(eye.x, z0)) * (shut && slit ? 0.7 : 1), minSide);
    const h = Math.max(eye.h * grow * squeeze(Math.atan2(y2, z2), Math.atan2(eye.y, z0)) * (1 - p.blink), minSide);
    const cx = (((x1 + p.gaze[0]) * ZOOM + 1) / 2) * g;
    let wc = Math.max(1, roundAway((w * ZOOM * g) / 2));
    if (wc % 2 !== ((roundAway(cx * 2) % 2) + 2) % 2) {
      wc += (w * ZOOM * g) / 2 > wc ? 1 : -1;
      wc = Math.max(1, wc);
    }
    const hc = Math.max(1, roundAway((h * ZOOM * g) / 2));
    const cy = (((y2 + p.gaze[1] + centreY) * ZOOM + 1) / 2) * g;
    const x0 = Math.floor(cx - wc / 2 + 0.5);
    const y0 = Math.floor(cy - hc / 2 + 0.5);
    const put = (xx: number, yy: number) => {
      const ring = [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]] as const;
      const around = [-1, 0, 1].flatMap(dy => [-1, 0, 1].map(dx => [dx, dy] as const));
      const apart = around.every(([dx, dy]) => {
        const [x, y] = [xx + dx, yy + dy];
        return x < 0 || y < 0 || x >= grid || y >= grid || owner[y * grid + x] === 0 || owner[y * grid + x] === id;
      });
      if (apart && ring.every(([dx, dy]) => solid(xx + dx, yy + dy))) {
        out[yy * grid + xx] = "eye";
        owner[yy * grid + xx] = id;
      }
    };
    if (p.smile > 0.5) {
      const aw = wc < 3 ? wc + 2 : wc;
      const xs = x0 - Math.trunc((aw - wc) / 2);
      const top = y0 + Math.trunc(hc / 4);
      for (let xx = xs + 1; xx < xs + aw - 1; xx++) put(xx, top);
      for (let yy = top + 1; yy <= top + Math.max(1, Math.trunc(hc / 4)); yy++) {
        put(xs, yy);
        put(xs + aw - 1, yy);
      }
      continue;
    }
    const lowered = Math.min(hc - 1, roundAway(hc * p.lid * 0.7));
    const outerLeft = eye.x < 0;
    const round = wc >= 3 && hc >= 3 && wc + hc >= 7;
    for (let yy = y0; yy < y0 + hc; yy++) {
      for (let xx = x0; xx < x0 + wc; xx++) {
        const outward = wc > 1 ? (outerLeft ? x0 + wc - 1 - xx : xx - x0) / (wc - 1) : 0;
        const worried = roundAway(outward * hc * 0.5 * p.worry);
        const lid = y0 + Math.min(hc - 1, lowered + worried);
        const corner = (xx === x0 || xx === x0 + wc - 1) && ((yy === y0 && lowered === 0) || yy === y0 + hc - 1);
        if (yy < lid || (corner && round) || xx < 0 || yy < 0 || xx >= grid || yy >= grid) continue;
        put(xx, yy);
      }
    }
  }
  return out;
}

/** SVG path data covering every cell of `kind`, one rectangle per run in a row. */
export function cellPath(cells: Cell[], grid: number, kind: Cell) {
  let d = "";
  for (let y = 0; y < grid; y++) {
    let x = 0;
    while (x < grid) {
      if (cells[y * grid + x] !== kind) { x++; continue; }
      const start = x;
      while (x < grid && cells[y * grid + x] === kind) x++;
      d += `M${start} ${y}h${x - start}v1h${start - x}z`;
    }
  }
  return d;
}
