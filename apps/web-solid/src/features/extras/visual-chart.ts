// Chart layout for visual `Chart` nodes, ported from plan_chart and paint_chart_plan in crates/ui/src/visuals.rs:
// round value ticks with gridlines, a gutter sized to the widest tick, bars at 72% of their band,
// thinned category labels, and the hovered category or point.
import { isObject, type Json, type JsonObject } from "./visual-session";
import { formatNumber } from "./visual-style";

export const AXIS_GAP = 8;
/** Desktop's Xs chart text: 11px on a 16px line. */
export const TICK_SIZE = 11;
const TICK_LINE = 16;

export type ChartMark =
  | { kind: "box"; x: number; y: number; w: number; h: number; r: number; color: Json; series: number }
  | { kind: "dot"; x: number; y: number; r: number; color: Json; series: number }
  | { kind: "line"; points: [number, number][]; color: Json; series: number }
  | { kind: "area"; points: [number, number][]; color: Json; series: number };

export interface ChartLabel {
  text: string;
  x: number;
  y: number;
  anchor: "start" | "middle" | "end";
  baseline: "hanging" | "middle";
}

export interface ChartPlan {
  plot: { x: number; y: number; w: number; h: number };
  grid: [number, number, number, number][];
  labels: ChartLabel[];
  marks: ChartMark[];
  lo: number;
  hi: number;
  n: number;
}

const list = (value: Json | undefined) => (Array.isArray(value) ? value : []);
const finite = (value: Json | undefined): number | null => (typeof value === "number" && Number.isFinite(value) ? value : null);

export const chartSeries = (chart: JsonObject) => list(chart.series).filter(isObject);
export const chartLabels = (chart: JsonObject) => list(chart.x).map((x) => (typeof x === "string" ? x : String(x ?? "")));
const values = (series: JsonObject) => list(series.values).map(finite);
const points = (series: JsonObject) =>
  list(series.points).flatMap((p): [number, number][] => (Array.isArray(p) && finite(p[0]) !== null && finite(p[1]) !== null ? [[p[0] as number, p[1] as number]] : []));

export const categories = (chart: JsonObject) => Math.max(chartLabels(chart).length, ...chartSeries(chart).map((s) => list(s.values).length), 0);

/** zuse_visual::nice_ticks. */
export function niceTicks(lo: number, hi: number, count: number): number[] {
  if (hi < lo) [lo, hi] = [hi, lo];
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) return [];
  if (Math.abs(hi - lo) < Number.EPSILON) return [lo];
  const raw = (hi - lo) / Math.max(1, count);
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  const normalized = raw / magnitude;
  const step = magnitude * (normalized < 1.5 ? 1 : normalized < 3 ? 2 : normalized < 7 ? 5 : 10);
  const digits = 10 ** Math.max(0, -Math.floor(Math.log10(step)));
  const out: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9 && out.length < 100; v += step) out.push(Math.round(Math.round(v / step) * step * digits) / digits);
  return out;
}

function domain(lo: number, hi: number, includeZero: boolean): [number, number, number[]] {
  if (includeZero) {
    lo = Math.min(lo, 0);
    hi = Math.max(hi, 0);
  }
  if (!Number.isFinite(lo) || !Number.isFinite(hi)) return [0, 1, [0, 1]];
  if (Math.abs(hi - lo) < Number.EPSILON) hi = lo + Math.max(Math.abs(lo), 1);
  const ticks = niceTicks(lo, hi, 4);
  const step = ticks.length > 1 ? ticks[1]! - ticks[0]! : Math.max(hi - lo, 1);
  lo = Math.floor(lo / step) * step;
  hi = Math.ceil(hi / step) * step;
  return [lo, hi, niceTicks(lo, hi, 4)];
}

function valueRange(chart: JsonObject): [number, number] {
  let lo = Infinity;
  let hi = -Infinity;
  const series = chartSeries(chart);
  if (chart.kind === "Scatter") {
    for (const [, y] of series.flatMap(points)) [lo, hi] = [Math.min(lo, y), Math.max(hi, y)];
  } else if (chart.stacked === true) {
    const n = Math.max(0, ...series.map((s) => list(s.values).length));
    for (let i = 0; i < n; i++) {
      let pos = 0;
      let neg = 0;
      for (const s of series) {
        const v = values(s)[i];
        if (v === null || v === undefined) continue;
        if (v >= 0) pos += v;
        else neg += v;
      }
      [lo, hi] = [Math.min(lo, neg), Math.max(hi, pos)];
    }
  } else {
    for (const v of series.flatMap(values)) if (v !== null) [lo, hi] = [Math.min(lo, v), Math.max(hi, v)];
  }
  return [finite(chart.y_min) ?? lo, finite(chart.y_max) ?? hi];
}

function xExtent(chart: JsonObject): [number, number] {
  let lo = Infinity;
  let hi = -Infinity;
  for (const [x] of chartSeries(chart).flatMap(points)) [lo, hi] = [Math.min(lo, x), Math.max(hi, x)];
  return [lo, hi];
}

export const seriesColor = (series: JsonObject, index: number): Json => series.color ?? { token: { Chart: index } };

/** Lay out a chart in a `width` × `height` box. `measure` gives a label's width at the tick size. */
export function planChart(chart: JsonObject, width: number, height: number, measure: (text: string) => number, hover: number | null, hoverPoint: [number, number] | null): ChartPlan {
  const scatter = chart.kind === "Scatter";
  const bars = chart.kind === "Bar";
  const horizontal = bars && chart.horizontal === true;
  const stacked = chart.stacked === true;
  const series = chartSeries(chart);
  const labels = chartLabels(chart);
  const [rawLo, rawHi] = valueRange(chart);
  const [lo, hi, ticks] = domain(rawLo, rawHi, !scatter);
  const [xLo, xHi, xTicks] = scatter ? domain(...xExtent(chart), false) : [0, 1, [] as number[]];
  const n = Math.max(1, categories(chart));
  const left =
    (horizontal
      ? Math.min(Math.max(0, ...labels.map(measure)), width * 0.35)
      : Math.max(0, ...ticks.map((t) => measure(formatNumber(t, chart.format))))) + AXIS_GAP;
  const top = TICK_LINE / 2;
  const bottom = TICK_LINE + AXIS_GAP / 2;
  const right = horizontal || scatter ? 12 : 4;
  const plot = { x: left, y: top, w: Math.max(0, width - left - right), h: Math.max(0, height - top - bottom) };
  const stride = horizontal || scatter ? 1 : Math.max(1, Math.ceil((Math.max(0, ...labels.map(measure)) + 12) / Math.max(1, plot.w / n)));
  const out: ChartPlan = { plot, grid: [], labels: [], marks: [], lo, hi, n };
  if (plot.w <= 4 || plot.h <= 4) return out;

  const { x: ox, y: oy, w: pw, h: ph } = plot;
  const t = (v: number) => (v - lo) / (hi - lo);
  const vy = (v: number) => oy + ph * (1 - t(v));
  const vx = (v: number) => ox + pw * t(v);
  for (const tick of ticks) {
    const text = formatNumber(tick, chart.format);
    if (horizontal) {
      const x = vx(tick);
      out.grid.push([x, oy, x, oy + ph]);
      out.labels.push({ text, x, y: oy + ph + AXIS_GAP / 2, anchor: "middle", baseline: "hanging" });
    } else {
      const y = vy(tick);
      out.grid.push([ox, y, ox + pw, y]);
      out.labels.push({ text, x: ox - AXIS_GAP, y, anchor: "end", baseline: "middle" });
    }
  }

  if (bars) {
    const band = (horizontal ? ph : pw) / n;
    const groups = stacked ? 1 : Math.max(1, series.length);
    const inner = band * 0.72;
    const bar = Math.max(1, inner / groups);
    for (let i = 0; i < n; i++) {
      const start = band * i + (band - inner) / 2;
      let pos = 0;
      let neg = 0;
      series.forEach((s, index) => {
        const v = values(s)[i];
        if (v === null || v === undefined) return;
        let from: number;
        let to: number;
        if (stacked) {
          if (v >= 0) [from, to] = [pos, (pos += v)];
          else [from, to] = [neg, (neg += v)];
        } else [from, to] = [Math.min(hi, Math.max(lo, 0)), v];
        const offset = stacked ? 0 : bar * index;
        const gap = groups > 1 && !stacked ? 1 : 0;
        const r = stacked ? 0 : 2;
        const color = seriesColor(s, index);
        if (horizontal) {
          const [a, b] = [vx(from), vx(to)];
          out.marks.push({ kind: "box", x: Math.min(a, b), y: oy + start + offset, w: Math.abs(b - a), h: bar - gap, r, color, series: index });
        } else {
          const [a, b] = [vy(from), vy(to)];
          out.marks.push({ kind: "box", x: ox + start + offset, y: Math.min(a, b), w: bar - gap, h: Math.abs(b - a), r, color, series: index });
        }
      });
    }
    labels.slice(0, n).forEach((text, i) => {
      if (i % stride) return;
      const center = band * (i + 0.5);
      out.labels.push(
        horizontal
          ? { text, x: ox - AXIS_GAP, y: oy + center, anchor: "end", baseline: "middle" }
          : { text, x: ox + center, y: oy + ph + AXIS_GAP / 2, anchor: "middle", baseline: "hanging" },
      );
    });
  } else if (scatter) {
    const sx = (x: number) => ox + (pw * (x - xLo)) / (xHi - xLo);
    for (const tick of xTicks) {
      const x = sx(tick);
      out.grid.push([x, oy, x, oy + ph]);
      out.labels.push({ text: formatNumber(tick, chart.x_format), x, y: oy + ph + AXIS_GAP / 2, anchor: "middle", baseline: "hanging" });
    }
    series.forEach((s, index) =>
      points(s).forEach(([x, y], p) =>
        out.marks.push({ kind: "dot", x: sx(x), y: vy(y), r: hoverPoint?.[0] === index && hoverPoint[1] === p ? 5 : 3, color: seriesColor(s, index), series: index }),
      ),
    );
  } else {
    const xAt = (i: number) => (n <= 1 ? ox + pw / 2 : ox + (pw * i) / (n - 1));
    const base = vy(Math.min(hi, Math.max(lo, 0)));
    series.forEach((s, index) => {
      const color = seriesColor(s, index);
      const runs: [number, number][][] = [[]];
      values(s).forEach((v, i) => (v === null ? runs.push([]) : runs[runs.length - 1]!.push([xAt(i), vy(v)])));
      for (const run of runs.filter((r) => r.length)) {
        if (chart.kind === "Area" && run.length > 1) out.marks.push({ kind: "area", points: [...run, [run[run.length - 1]![0], base], [run[0]![0], base]], color, series: index });
        if (run.length === 1) out.marks.push({ kind: "dot", x: run[0]![0], y: run[0]![1], r: 2.5, color, series: index });
        else out.marks.push({ kind: "line", points: run, color, series: index });
      }
      const hovered = hover === null ? null : values(s)[hover];
      if (hover !== null && hovered !== null && hovered !== undefined) out.marks.push({ kind: "dot", x: xAt(hover), y: vy(hovered), r: 3.5, color, series: index });
    });
    labels.slice(0, n).forEach((text, i) => {
      if (i % stride) return;
      const anchor = n > 1 && i === 0 ? "start" : n > 1 && i === n - 1 ? "end" : "middle";
      out.labels.push({ text, x: xAt(i), y: oy + ph + AXIS_GAP / 2, anchor, baseline: "hanging" });
    });
  }
  return out;
}

/** The category under the pointer, as desktop picks it: bands for bars, nearest sample for lines. */
export function hoveredCategory(chart: JsonObject, plan: ChartPlan, x: number, y: number): number | null {
  if (chart.kind === "Scatter" || plan.plot.w <= 0) return null;
  const horizontal = chart.kind === "Bar" && chart.horizontal === true;
  const along = horizontal ? (y - plan.plot.y) / plan.plot.h : (x - plan.plot.x) / plan.plot.w;
  const index = chart.kind === "Bar" || plan.n <= 1 ? Math.floor(along * plan.n) : Math.round(along * (plan.n - 1));
  return index >= 0 && index < plan.n ? index : null;
}

/** The scatter point within 12px of the pointer. */
export function nearestPoint(chart: JsonObject, plan: ChartPlan, x: number, y: number): [number, number] | null {
  if (chart.kind !== "Scatter") return null;
  let best: [number, number, number] | null = null;
  for (const mark of plan.marks) {
    if (mark.kind !== "dot") continue;
    const d = Math.hypot(mark.x - x, mark.y - y);
    if (d < 12 && (!best || d < best[0])) best = [d, mark.series, 0];
  }
  if (!best) return null;
  // Recover the point index within its series.
  const dots = plan.marks.filter((m): m is Extract<ChartMark, { kind: "dot" }> => m.kind === "dot" && m.series === best![1]);
  const index = dots.findIndex((m) => Math.hypot(m.x - x, m.y - y) === best![0]);
  return [best[1], Math.max(0, index)];
}

export function scatterPoint(chart: JsonObject, series: number, index: number): [number, number] | null {
  const s = chartSeries(chart)[series];
  return s ? (points(s)[index] ?? null) : null;
}

export const seriesValue = (chart: JsonObject, series: number, index: number) => {
  const s = chartSeries(chart)[series];
  return s ? (values(s)[index] ?? null) : null;
};
