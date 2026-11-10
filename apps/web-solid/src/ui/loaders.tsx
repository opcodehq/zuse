// Desktop's loaders (crates/ui/src/loaders.rs), drawn the same way:
// - GlyphSpinner: `mini_glyph_spinner` / `mini_mono_spinner`, a 2×3 grid whose brightness chases around its ring,
//   each cell thresholded through a 4×4 Bayer matrix on device pixels so it dissolves through the dither.
// - DitherRing: `dither_ring`, a bright tail sweeping a faint dotted track on one-logical-pixel cells.
// - TerminalSpinner: `terminal_spinner`, the braille "dots" frames at 80 ms each.
// All loaders share one clock, so every instance stays phase-locked like desktop's pulse clock, and offscreen
// loaders skip their frames. Reduced motion follows desktop: activity glyphs keep a gentle uniform fade when the
// system asks for less motion, explicit "Reduce motion: On" holds them still, and the ring and braille hold a frame.
import { createEffect, onCleanup, onMount } from "solid-js";

/** Colour role. `glyph` is the accent family (desktop `theme.glyph`); the rest are single tints. */
export type LoaderTone = "glyph" | "muted" | "faint" | "accent" | "warning" | "text" | "current";

interface LoaderProps {
  /** Screen-reader label. Ignored when `decorative`. */
  label?: string;
  /** Hide from assistive tech when the surrounding text already says what is happening. */
  decorative?: boolean;
  tone?: LoaderTone;
  class?: string;
}

// ---- motion constants (crates/protocol/src/motion.rs, crates/ui/src/motion.rs) ----
const GRADIENT_SPIN_S = 0.75;
const ZUSE_PULSE_S = 2.4;
const GSPIN_DIM = 0.1;
/** Clockwise ring position of each (row, col) cell of the 2×3 grid. */
const MINI_RING = [[0, 1], [5, 2], [4, 3]] as const;
/** crates/viewport/src/dither.rs BAYER_4, read row-major. */
const BAYER_4 = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
const TERMINAL_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const TERMINAL_FRAME_MS = 80;
const RING_STILL_S = 0.4;
const RING_TRACK = 0.13;

const fract = (v: number) => v - Math.floor(v);
const wave = (phase: number) => 0.5 - 0.5 * Math.cos(phase * Math.PI * 2);
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;

function gspinOpacity(t: number, dim: number) {
  t = fract(t);
  if (t < 0.45) return lerp(1, dim, t / 0.45);
  if (t < 0.92) return dim;
  return lerp(dim, 1, (t - 0.92) / 0.08);
}

/** Viewport dither threshold (crates/viewport/src/dither.rs `threshold`). */
const viewportThreshold = (x: number, y: number) => (BAYER_4[(y & 3) * 4 + (x & 3)] ?? 0) / 16 + 0.5 / 16;

/** zkit-dither `Matrix::Bayer4.threshold`. */
function zkitBayer4(x: number, y: number) {
  let rank = 0;
  for (let bit = 1; bit < 4; bit *= 2) {
    const a = Math.floor(x / bit) & 1;
    const b = Math.floor(y / bit) & 1;
    rank = rank * 4 + 2 * (a ^ b) + b;
  }
  return (rank + 0.5) / 16;
}

/** zkit-dither `LoaderVariant::Ring.tone`. */
function ringTone(size: number, x: number, y: number, t: number) {
  const half = size / 2;
  const dx = (x + 0.5 - half) / half;
  const dy = (y + 0.5 - half) / half;
  const radius = Math.hypot(dx, dy);
  if (radius < 0.58 || radius > 1) return 0;
  const angle = fract(Math.atan2(dx, -dy) / (Math.PI * 2));
  const tail = 0.5 + 0.3 * (0.5 - 0.5 * Math.cos(t * 2.1));
  const behind = fract(t * 0.85 - angle);
  if (behind > tail) return RING_TRACK;
  return Math.max(Math.pow(1 - behind / tail, 1.4) * 1.1, RING_TRACK);
}

// ---- motion preference ----
type Motion = "full" | "subtle" | "still";
const systemReduced = typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
/** `subtle`: the system asks for less motion and the app follows it. `still`: the user turned motion off outright. */
function motion(): Motion {
  if (document.documentElement.dataset.reduceMotion !== "on") return "full";
  return systemReduced?.matches ? "subtle" : "still";
}

// ---- shared clock ----
interface Sub {
  /** Frame length in ms; a sub repaints only when its frame index changes. */
  step: number;
  last: number;
  visible: boolean;
  paint: (seconds: number) => void;
}
const subs = new Set<Sub>();
let raf = 0;
function tick(now: number) {
  raf = 0;
  if (!subs.size) return;
  for (const sub of subs) {
    if (!sub.visible) continue;
    const frame = Math.floor(now / sub.step);
    if (frame === sub.last) continue;
    sub.last = frame;
    sub.paint(now / 1000);
  }
  raf = requestAnimationFrame(tick);
}
function subscribe(sub: Sub) {
  subs.add(sub);
  if (!raf) raf = requestAnimationFrame(tick);
  return () => subs.delete(sub);
}

const visibility = new WeakMap<Element, Sub>();
const observer = typeof IntersectionObserver === "function"
  ? new IntersectionObserver((entries) => {
    for (const entry of entries) {
      const sub = visibility.get(entry.target);
      if (sub) sub.visible = entry.isIntersecting;
    }
  })
  : null;

// Theme, accent and motion changes repaint every loader with fresh colours.
const listeners = new Set<() => void>();
let colorCache = new Map<string, string>();
let probe: HTMLSpanElement | undefined;
function notify() {
  colorCache = new Map();
  for (const run of listeners) run();
}
if (typeof MutationObserver === "function") {
  new MutationObserver(notify).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "data-reduce-motion", "style", "class"] });
  systemReduced?.addEventListener("change", notify);
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", notify);
}
/** Resolve a CSS colour (tokens, relative colours) to something a canvas accepts. */
function resolveColor(css: string) {
  let value = colorCache.get(css);
  if (value !== undefined) return value;
  if (!probe) {
    probe = document.createElement("span");
    probe.style.display = "none";
    document.body.append(probe);
  }
  probe.style.color = "";
  probe.style.color = css;
  value = getComputedStyle(probe).color;
  colorCache.set(css, value);
  return value;
}

/** Drive one loader: repaint on its clock, on theme changes and when it scrolls back into view. */
function useLoader(el: () => HTMLElement | undefined, step: () => number, animated: () => boolean, paint: (seconds: number | null) => void) {
  onMount(() => {
    const node = el();
    if (!node) return;
    const sub: Sub = { step: step(), last: -1, visible: true, paint: (s) => paint(s) };
    let stop: (() => void) | undefined;
    const sync = () => {
      sub.step = step();
      sub.last = -1;
      if (animated()) {
        stop ??= subscribe(sub);
      } else {
        stop?.();
        stop = undefined;
      }
      paint(animated() ? performance.now() / 1000 : null);
    };
    visibility.set(node, sub);
    observer?.observe(node);
    listeners.add(sync);
    createEffect(sync);
    onCleanup(() => {
      stop?.();
      listeners.delete(sync);
      observer?.unobserve(node);
      visibility.delete(node);
    });
  });
}


/** Size a canvas to whole device pixels and return its device-pixel dimensions. */
function fit(canvas: HTMLCanvasElement, width: number, height: number) {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(width * dpr));
  const h = Math.max(1, Math.round(height * dpr));
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
    canvas.style.width = `${w / dpr}px`;
    canvas.style.height = `${h / dpr}px`;
  }
  return { dpr, w, h };
}

/**
 * Desktop's activity glyph (`mini_glyph_spinner`): the default loader for anything that loads.
 * `cell` is the block size in px (desktop uses 1.5 to 2.5); the glyph is 2.5 × 4 cells, centred in a `box` px slot.
 * `tone="glyph"` (default) uses the accent family per row; any other tone is the mono variant.
 */
export function GlyphSpinner(props: LoaderProps & { cell?: number; box?: number }) {
  let root: HTMLSpanElement | undefined;
  let canvas: HTMLCanvasElement | undefined;
  let tint = "";
  let tintRead = 0;
  const cell = () => props.cell ?? 2;
  const tone = () => props.tone ?? "glyph";
  useLoader(
    () => root,
    () => (motion() === "full" ? 33 : 66),
    () => motion() !== "still",
    (seconds) => {
      if (!canvas || !root) return;
      const c = cell();
      const { dpr, w, h } = fit(canvas, c * 2.5, c * 4);
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, w, h);
      const mode = motion();
      const rows = tone() === "glyph"
        ? ["var(--glyph-light)", "var(--glyph-mid)", "var(--glyph-deep)"].map(resolveColor)
        : (() => {
          // A mono tint follows the element's colour; re-read it now and then so hover colours carry over.
          const now = performance.now();
          if (!tint || now - tintRead > 500) {
            tint = getComputedStyle(root).color;
            tintRead = now;
          }
          return [tint, tint, tint];
        })();
      const t = seconds ?? 0;
      const level = (ring: number) =>
        mode === "full" ? gspinOpacity(fract(t / GRADIENT_SPIN_S) + ring / 6, GSPIN_DIM) : 0.6 + 0.2 * (mode === "subtle" ? wave(fract(t / ZUSE_PULSE_S)) : 0);
      const dots = Math.max(1, Math.round(c * dpr));
      const dot = (c * dpr) / dots;
      for (let row = 0; row < 3; row++) {
        ctx.fillStyle = rows[row] ?? tint;
        for (let col = 0; col < 2; col++) {
          const lit = level(MINI_RING[row]?.[col] ?? 0);
          const bx = col * c * 1.5 * dpr;
          const by = row * c * 1.5 * dpr;
          for (let y = 0; y < dots; y++) {
            for (let x = 0; x < dots; x++) {
              if (lit <= viewportThreshold(x, y)) continue;
              const x0 = Math.round(bx + x * dot);
              const y0 = Math.round(by + y * dot);
              ctx.fillRect(x0, y0, Math.max(1, Math.round(bx + (x + 1) * dot) - x0), Math.max(1, Math.round(by + (y + 1) * dot) - y0));
            }
          }
        }
      }
    },
  );
  return (
    <span
      ref={root}
      class={`loader loader-glyph ${props.class ?? ""}`}
      data-tone={tone()}
      style={props.box === undefined ? undefined : { width: `${props.box}px`, height: `${props.box}px` }}
      role={props.decorative ? undefined : "status"}
      aria-label={props.decorative ? undefined : (props.label ?? "Loading")}
      aria-hidden={props.decorative ? "true" : undefined}
    >
      <canvas ref={canvas} aria-hidden="true" />
    </span>
  );
}

/** Desktop's `dither_ring`: the transcript's working row and quieter in-place waits. `size` in px (12 to 14). */
export function DitherRing(props: LoaderProps & { size?: number }) {
  let root: HTMLSpanElement | undefined;
  let canvas: HTMLCanvasElement | undefined;
  let tint = "";
  let tintRead = 0;
  const size = () => props.size ?? 12;
  useLoader(
    () => root,
    () => 66,
    () => motion() === "full",
    (seconds) => {
      if (!canvas || !root) return;
      const cells = Math.max(Math.round(size()), 6);
      const { dpr, w, h } = fit(canvas, cells, cells);
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, w, h);
      const now = performance.now();
      if (!tint || now - tintRead > 500) {
        tint = getComputedStyle(root).color;
        tintRead = now;
      }
      ctx.fillStyle = tint;
      const t = seconds ?? RING_STILL_S;
      for (let y = 0; y < cells; y++) {
        for (let x = 0; x < cells; x++) {
          if (ringTone(cells, x, y, t) <= zkitBayer4(x, y)) continue;
          const x0 = Math.round(x * dpr);
          const y0 = Math.round(y * dpr);
          ctx.fillRect(x0, y0, Math.round((x + 1) * dpr) - x0, Math.round((y + 1) * dpr) - y0);
        }
      }
    },
  );
  return (
    <span
      ref={root}
      class={`loader loader-ring ${props.class ?? ""}`}
      data-tone={props.tone ?? "muted"}
      style={{ width: `${size()}px`, height: `${size()}px` }}
      role={props.decorative ? undefined : "status"}
      aria-label={props.decorative ? undefined : (props.label ?? "Loading")}
      aria-hidden={props.decorative ? "true" : undefined}
    >
      <canvas ref={canvas} aria-hidden="true" />
    </span>
  );
}

/** Desktop's `terminal_spinner`: braille "dots" frames, for running work in lists (sidebar rows, checks). */
export function TerminalSpinner(props: LoaderProps & { size?: number }) {
  let root: HTMLSpanElement | undefined;
  let glyph: HTMLSpanElement | undefined;
  const size = () => props.size ?? 13;
  useLoader(
    () => root,
    () => TERMINAL_FRAME_MS,
    () => motion() === "full",
    (seconds) => {
      if (!glyph) return;
      // Reduced motion snaps the animation to its end, like desktop's gpui animations.
      const frame = seconds === null ? TERMINAL_FRAMES.length - 1 : Math.floor((seconds * 1000) / TERMINAL_FRAME_MS) % TERMINAL_FRAMES.length;
      const next = TERMINAL_FRAMES[frame] ?? "";
      if (glyph.textContent !== next) glyph.textContent = next;
    },
  );
  return (
    <span
      ref={root}
      class={`loader loader-terminal ${props.class ?? ""}`}
      data-tone={props.tone ?? "muted"}
      style={{ width: `${size()}px`, height: `${size()}px`, "font-size": `${size()}px`, "line-height": `${size()}px` }}
      role={props.decorative ? undefined : "status"}
      aria-label={props.decorative ? undefined : (props.label ?? "Loading")}
      aria-hidden={props.decorative ? "true" : undefined}
    >
      <span ref={glyph} aria-hidden="true">{TERMINAL_FRAMES[0]}</span>
    </span>
  );
}
