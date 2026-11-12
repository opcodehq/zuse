// Daily usage plot: dithered series on 2px cells, the reference plot's margins, axes and hover/day selection
// (crates/viewport/src/usage_dashboard/chart.rs). The raster redraws only when data, size or theme change.
import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { resolvedTheme } from "@/shell/theme";
import { money, tokens } from "./format";
import type { UsageGroup } from "./state";

export const SERIES = [
  { name: "Input", color: "#358ff3", rgb: [53, 143, 243] },
  { name: "Output", color: "#28d26e", rgb: [40, 210, 110] },
  { name: "Cache", color: "#966eff", rgb: [150, 110, 255] },
  { name: "Reasoning", color: "#ff9632", rgb: [255, 150, 50] },
] as const;
export const COST_SERIES = [{ name: "Estimated cost", color: "#358ff3", rgb: [53, 143, 243] }] as const;

const LEFT = 72;
const TOP = 10;
const RIGHT = 12;
const PLOT_H = 224;
const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

function tickStep(extent: number, count: number) {
  const raw = extent / count;
  const power = 10 ** Math.floor(Math.log10(raw));
  const error = raw / power;
  return power * (error >= Math.sqrt(50) ? 10 : error >= Math.sqrt(10) ? 5 : error >= Math.sqrt(2) ? 2 : 1);
}

function valuesOf(group: UsageGroup, cost: boolean) {
  return cost
    ? [group.costUsd ?? 0]
    : [group.inputTokens, group.outputTokens, group.cacheReadTokens + group.cacheCreationTokens, group.reasoningTokens];
}

const parseColor = (css: string): [number, number, number] => {
  const probe = document.createElement("canvas").getContext("2d");
  if (!probe) return [24, 24, 24];
  probe.fillStyle = css;
  const hex = probe.fillStyle;
  if (hex.startsWith("#")) return [parseInt(hex.slice(1, 3), 16), parseInt(hex.slice(3, 5), 16), parseInt(hex.slice(5, 7), 16)];
  const m = /(\d+),\s*(\d+),\s*(\d+)/.exec(hex);
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : [24, 24, 24];
};

export function Chart(props: { groups: UsageGroup[]; cost: boolean; hovered: number | null; onHover: (index: number | null) => void; onViewDay: (index: number) => void }) {
  let host: HTMLDivElement | undefined;
  let base: HTMLCanvasElement | undefined;
  let marker: HTMLCanvasElement | undefined;
  const [width, setWidth] = createSignal(0);
  const [cursor, setCursor] = createSignal<number | null>(null);

  const groups = createMemo(() => props.groups.slice(-90));
  const values = createMemo(() => groups().map((g) => valuesOf(g, props.cost)));
  const scale = createMemo(() => {
    const highest = Math.max(0, ...values().flat()) || 1;
    const nice = tickStep(highest, 10);
    const max = Math.ceil(highest / nice) * nice;
    const step = tickStep(max, 4);
    const ticks: number[] = [];
    for (let i = 0; i <= Math.floor(max / step + 1e-9); i++) ticks.push(i * step);
    return { max, ticks };
  });
  const plotWidth = () => Math.max(1, width() - LEFT - RIGHT);
  const count = () => groups().length;
  const fmt = (v: number) => (props.cost ? money(v) : tokens(Math.round(v)));

  onMount(() => {
    const observer = new ResizeObserver(([entry]) => setWidth(Math.floor(entry!.contentRect.width)));
    observer.observe(host!);
    onCleanup(() => observer.disconnect());
  });

  const geometry = () => {
    const w = plotWidth();
    const cols = Math.min(520, Math.max(8, Math.round(w / 2)));
    const rows = Math.min(200, Math.max(8, Math.round(PLOT_H / 2)));
    return { w, cols, rows, cw: w / cols, ch: PLOT_H / rows };
  };

  // Base raster: grid ticks plus dithered series. Theme is read so a switch repaints with the new surface.
  createEffect(() => {
    const canvas = base;
    if (!canvas || width() <= 0) return;
    resolvedTheme();
    const { w, cols, rows, cw, ch } = geometry();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(PLOT_H * dpr);
    canvas.style.width = `${w}px`;
    canvas.style.height = `${PLOT_H}px`;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const styles = getComputedStyle(canvas);
    const bg = parseColor(styles.getPropertyValue("--card").trim() || "#181818");
    const border = styles.getPropertyValue("--border").trim() || "#272727";
    ctx.fillStyle = `rgb(${bg.join(",")})`;
    ctx.fillRect(0, 0, w, PLOT_H);
    const { max, ticks } = scale();
    ctx.fillStyle = border;
    for (const tick of ticks) {
      const y = Math.max(0, Math.round((1 - tick / max) * PLOT_H));
      for (let x = 0; x < w; x += 6) ctx.fillRect(x, Math.min(y, PLOT_H - 1), 3, 1);
    }
    const data = values();
    const n = data.length;
    if (!n) return;
    const colors = props.cost ? COST_SERIES : SERIES;
    for (let series = 0; series < (data[0]?.length ?? 0); series++) {
      const dotted = !props.cost && series % 2 === 1;
      const [r, g, b] = colors[series]!.rgb;
      for (let x = 0; x < cols; x++) {
        const position = cols > 1 ? (x / (cols - 1)) * Math.max(0, n - 1) : 0;
        const a = Math.min(Math.floor(position), n - 1);
        const bIndex = Math.min(a + 1, n - 1);
        const t = position - a;
        const value = data[a]![series]! * (1 - t) + data[bIndex]![series]! * t;
        const start = Math.min(rows, Math.max(0, Math.round((1 - value / max) * rows)));
        const depth = rows - start;
        for (let y = start; y < rows; y++) {
          let alpha: number;
          if (y === start) alpha = 0.72;
          else if (y === start + 1) alpha = 0.36;
          else {
            const density = (y - start) / Math.max(1, depth);
            const bias = (dotted ? 0.12 : 0) - series * 0.14;
            const lit = density > (BAYER[(y & 3) * 4 + (x & 3)]! + 0.5) / 16 - bias;
            if (dotted && !lit) continue;
            alpha = (0.3 + density * 0.7) * (lit ? 1 : 0.4);
          }
          ctx.fillStyle = `rgba(${r},${g},${b},${alpha})`;
          ctx.fillRect(x * cw, y * ch, cw + 0.01, ch + 0.01);
        }
      }
    }
  });

  // The hover marker: a tinted column and a dot per series at the hovered day.
  createEffect(() => {
    const canvas = marker;
    if (!canvas || width() <= 0) return;
    const { w, cols, rows, cw, ch } = geometry();
    const dpr = window.devicePixelRatio || 1;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(PLOT_H * dpr);
    canvas.style.width = `${w}px`;
    canvas.style.height = `${PLOT_H}px`;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, PLOT_H);
    const i = props.hovered;
    const n = count();
    if (i === null || n < 2 || (props.cost && groups()[i]?.costUsd == null)) return;
    const { max } = scale();
    const colors = props.cost ? COST_SERIES : SERIES;
    const x = Math.round((i / (n - 1)) * (cols - 1)) * cw;
    values()[i]!.forEach((value, series) => {
      const y = Math.round((1 - value / max) * (rows - 1)) * ch;
      ctx.globalAlpha = 0.55;
      ctx.fillStyle = colors[series]!.color;
      ctx.fillRect(x, y, cw, Math.max(0, PLOT_H - y));
      ctx.globalAlpha = 1;
      ctx.fillRect(x - cw, y - ch, cw * 3, ch * 3);
    });
  });

  // A new period or measure drops a hover that may point past the end.
  createEffect(on([groups, () => props.cost], () => props.onHover(null), { defer: true }));

  const indexAt = (clientX: number) => {
    const rect = host!.getBoundingClientRect();
    const x = clientX - rect.left - LEFT;
    return { index: Math.round(Math.min(1, Math.max(0, x / plotWidth())) * Math.max(0, count() - 1)), cursor: clientX - rect.left };
  };

  const axis = createMemo(() => {
    const n = count();
    const stride = Math.max(1, Math.ceil(n / 7));
    const out: { label: string; fraction: number }[] = [];
    for (let i = 0; i < n; i += stride) out.push({ label: groups()[i]!.label, fraction: n > 1 ? i / (n - 1) : 0.5 });
    return out;
  });

  const tip = createMemo(() => {
    const i = props.hovered;
    const c = cursor();
    if (i === null || c === null) return null;
    const group = groups()[i];
    if (!group) return null;
    const rows = props.cost
      ? [{ name: "Estimated cost", color: COST_SERIES[0].color, value: money(group.costUsd) }]
      : SERIES.map((s, k) => ({ name: s.name, color: s.color, value: tokens(valuesOf(group, false)[k]!) }));
    const highest = Math.max(...values()[i]!);
    return { group, rows, left: Math.max(48, Math.min(c, plotWidth() + 24)), top: Math.max(54, TOP + PLOT_H * (1 - highest / scale().max)) };
  });

  const describe = (i: number) => {
    const group = groups()[i];
    return group ? `${group.label}: ${tokens(valuesOf(group, false).reduce((a, b) => a + b, 0))} tokens, ${money(group.costUsd)}` : "";
  };

  return (
    <div
      ref={host}
      class="us-chart"
      role="img"
      tabIndex={0}
      aria-roledescription="chart"
      aria-label={`Daily ${props.cost ? "estimated cost" : "tokens"} for ${count()} days. Use the arrow keys to move between days and Enter to view one.${props.hovered !== null ? ` ${describe(props.hovered)}` : ""}`}
      onPointerMove={(e) => {
        const at = indexAt(e.clientX);
        setCursor(at.cursor);
        if (at.index !== props.hovered) props.onHover(at.index);
      }}
      onPointerLeave={() => {
        setCursor(null);
        props.onHover(null);
      }}
      onKeyDown={(e) => {
        const n = count();
        if (!n) return;
        const current = props.hovered ?? (e.key === "ArrowLeft" ? n : -1);
        let next: number | null = null;
        if (e.key === "ArrowRight") next = Math.min(n - 1, current + 1);
        else if (e.key === "ArrowLeft") next = Math.max(0, current - 1);
        else if (e.key === "Home") next = 0;
        else if (e.key === "End") next = n - 1;
        else if ((e.key === "Enter" || e.key === " ") && props.hovered !== null) {
          e.preventDefault();
          props.onViewDay(props.hovered);
          return;
        }
        if (next === null) return;
        e.preventDefault();
        setCursor(LEFT + (n > 1 ? (next / (n - 1)) * plotWidth() : plotWidth() / 2));
        props.onHover(next);
      }}
      onBlur={() => {
        setCursor(null);
        props.onHover(null);
      }}
    >
      <canvas ref={base} aria-hidden="true" />
      <canvas ref={marker} aria-hidden="true" />
      <For each={scale().ticks}>
        {(value) => (
          <span class="us-tick" aria-hidden="true" style={{ top: `${TOP + PLOT_H * (1 - value / scale().max) - 6.75}px` }}>
            {fmt(value)}
          </span>
        )}
      </For>
      <div class="us-axis" aria-hidden="true">
        <For each={axis()}>{(tick) => <span style={{ left: `${tick.fraction * 100}%` }}>{tick.label}</span>}</For>
      </div>
      <Show when={tip()}>
        {(t) => (
          <div class="us-tip" aria-hidden="true" style={{ left: `${t().left}px`, top: `${t().top}px`, transform: "translate(-50%, -115%)" }}>
            <div class="us-tip-date">{t().group.label}</div>
            <For each={t().rows}>
              {(row) => (
                <div class="us-tip-row">
                  <i style={{ background: row.color }} />
                  <span>{row.name}</span>
                  <b>{row.value}</b>
                </div>
              )}
            </For>
          </div>
        )}
      </Show>
    </div>
  );
}
