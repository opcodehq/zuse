// Dithered meters and columns on 2px cells, the desktop usage meter's look (crates/ui/src/dither_bar.rs).
// Colors come from theme variables, so both themes repaint from the same tones.
import { createEffect, createSignal, For, onSettled } from "solid-js";
import type { JSX } from "@solidjs/web";
import { resolvedTheme } from "@/shell/theme";
import { CELL, columnCell, filledRows, litColumns, meterCell, type Cell } from "./ditherCells";

const ALPHA: Record<Cell, number> = { on: 1, faint: 0.25, track: 1, empty: 0 };

/** Sizes a canvas to its host on device pixels and repaints when the size, theme or inputs change. */
function useCanvas(paint: (ctx: CanvasRenderingContext2D, width: number, height: number, color: (css: string) => string) => void, height: () => number, inputs: () => unknown) {
  let host: HTMLDivElement | undefined;
  let canvas: HTMLCanvasElement | undefined;
  const [width, setWidth] = createSignal(0);
  onSettled(() => {
    const observer = new ResizeObserver(([entry]) => setWidth(Math.floor(entry?.contentRect.width ?? 0)));
    observer.observe(host!);
    return () => observer.disconnect();
  });
  createEffect(() => [width(), height(), resolvedTheme(), inputs()] as const, ([w, h]) => {
    if (!canvas || w <= 0) return;
    const dpr = Math.max(1, window.devicePixelRatio || 1);
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    // Theme variables resolve through the canvas's own computed color.
    const color = (css: string) => {
      canvas!.style.color = css;
      return getComputedStyle(canvas!).color;
    };
    paint(ctx, w, h, color);
  });
  return {
    host: (el: HTMLDivElement) => (host = el),
    canvas: (el: HTMLCanvasElement) => (canvas = el),
  };
}

function cell(ctx: CanvasRenderingContext2D, x: number, y: number, fill: string, alpha: number) {
  ctx.globalAlpha = alpha;
  ctx.fillStyle = fill;
  ctx.fillRect(x * CELL, y * CELL, CELL, CELL);
}

/** A horizontal meter filled to `fraction`, densest at the leading edge, over a sparse dotted track. */
export function DitherMeter(props: { fraction: number; tone?: string; height?: number; label: string; valueText: string; title?: string; class?: string }) {
  const height = () => props.height ?? 6;
  const refs = useCanvas((ctx, w, h, color) => {
    const on = color(props.tone ?? "var(--accent)");
    const track = color("var(--text-muted)");
    const columns = Math.floor(w / CELL);
    const rows = Math.floor(h / CELL);
    const lit = litColumns(props.fraction, columns);
    for (let y = 0; y < rows; y++) {
      for (let x = 0; x < columns; x++) {
        const kind = meterCell(x, y, lit);
        if (kind === "empty") continue;
        cell(ctx, x, y, kind === "track" ? track : on, kind === "track" ? 0.3 : ALPHA[kind]);
      }
    }
  }, height, () => [props.fraction, props.tone]);
  return (
    <div
      ref={refs.host}
      class={`pc-dither ${props.class ?? ""}`}
      style={{ height: `${height()}px` }}
      role="meter"
      aria-label={props.label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(Math.max(0, Math.min(1, props.fraction)) * 100)}
      aria-valuetext={props.valueText}
      title={props.title}
    >
      <canvas ref={refs.canvas} aria-hidden="true" />
    </div>
  );
}

export interface Column {
  label: string;
  value: number;
  /** Hover text and the column's accessible name. */
  title: string;
}

/** Dithered columns, one per entry, with hover titles, a readout line and first, middle and last labels below. */
export function DitherColumns(props: { columns: Column[]; label: string; idle: string; height?: number; tone?: string }) {
  const height = () => props.height ?? 112;
  const [hovered, setHovered] = createSignal<number | null>(null);
  const refs = useCanvas((ctx, w, h, color) => {
    const on = color(props.tone ?? "var(--accent)");
    const track = color("var(--text-muted)");
    const grid = color("var(--border)");
    const columns = props.columns;
    const count = columns.length;
    const rows = Math.floor(h / CELL);
    const cols = Math.floor(w / CELL);
    if (!count || cols <= 0) return;
    // Dotted guides at half and full scale.
    ctx.globalAlpha = 1;
    ctx.fillStyle = grid;
    for (const guide of [0, Math.floor(rows / 2)]) for (let x = 0; x < cols; x += 3) ctx.fillRect(x * CELL, guide * CELL, CELL, 1);
    const max = Math.max(0, ...columns.map((c) => c.value));
    const slot = cols / count;
    const bar = Math.max(1, Math.min(14, Math.floor(slot * 0.68)));
    const focus = hovered();
    columns.forEach((column, index) => {
      const left = Math.round(index * slot + (slot - bar) / 2);
      const filled = filledRows(column.value, max, rows - 1);
      for (let x = 0; x < bar; x++) {
        for (let y = 0; y < rows; y++) {
          const kind = columnCell(left + x, y, filled);
          if (kind === "empty") continue;
          const alpha = kind === "track" ? 0.35 : focus !== null && focus !== index ? ALPHA[kind] * 0.45 : ALPHA[kind];
          cell(ctx, left + x, rows - 1 - y, kind === "track" ? track : on, alpha);
        }
      }
    });
  }, height, () => [props.columns.map(column => column.value), props.tone]);
  const ticks = () => {
    const list = props.columns;
    if (list.length < 2) return list.map((c) => c.label);
    return [list[0]!.label, list.length > 4 ? list[Math.floor((list.length - 1) / 2)]!.label : "", list[list.length - 1]!.label];
  };
  const readout = () => {
    const index = hovered();
    return (index !== null && props.columns[index]?.title) || props.idle;
  };
  return (
    <div class="pc-columns" role="group" aria-label={props.label}>
      <div class="pc-columns-readout">
        {readout()}
      </div>
      <div ref={refs.host} class="pc-columns-plot" style={{ height: `${height()}px` }} onMouseLeave={() => setHovered(null)}>
        <canvas ref={refs.canvas} aria-hidden="true" />
        <div class="pc-columns-hits" style={{ "grid-template-columns": `repeat(${Math.max(1, props.columns.length)}, minmax(0, 1fr))` }}>
          <For each={props.columns}>
            {(column, index) => (
              <div
                class="pc-columns-hit"
                role="img"
                title={column.title}
                aria-label={column.title}
                data-active={hovered() === index() ? "true" : undefined}
                onMouseEnter={() => setHovered(index())}
              />
            )}
          </For>
        </div>
      </div>
      <div class="pc-columns-axis" aria-hidden="true">
        <For each={ticks()}>{(label) => <span>{label}</span>}</For>
      </div>
    </div>
  );
}

/** A labelled row with a thin dithered bar under it: breakdown lists. */
export function DitherShare(props: { label: JSX.Element; value: string; title: string; fraction: number; tone?: string }) {
  return (
    <div class="pc-share" title={props.title}>
      <div class="pc-share-head">
        <span class="pc-share-label">{props.label}</span>
        <span class="pc-share-value">{props.value}</span>
      </div>
      <DitherMeter fraction={props.fraction} tone={props.tone} height={4} label={typeof props.label === "string" ? props.label : props.value} valueText={props.value} />
    </div>
  );
}
