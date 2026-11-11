// Agent visuals (`show_visual`) rendered as real DOM controls, SVG charts and 2D canvases.
// The host runs the script; this view renders its tree and sends handler calls against the revision shown.
// 3D scenes and shaders need the desktop renderer and say so in place.
import { createEffect, createSignal, For, Index, type JSX, Match, onCleanup, onMount, Show, Switch, untrack } from "solid-js";
import { state } from "@/core/store";
import { Button } from "@/ui/primitives";
import { chartLabels, chartSeries, hoveredCategory, nearestPoint, planChart, scatterPoint, seriesColor, seriesValue, TICK_SIZE } from "./visual-chart";
import { canvasColor, CHART_COLORS, cssColor, fontWeight, formatNumber, num, textSize } from "./visual-style";
import {
  type Argument,
  canvasInternals,
  control,
  isObject,
  type Json,
  type JsonObject,
  openVisual,
  trackMounted,
  variant,
  visualKey,
  visuals,
} from "./visual-session";

/** A transcript `visual` part. Like desktop, it loads as soon as it is shown; the button is the fallback when it can't. */
export function VisualPart(props: { chatId: string; part: { id: string; title?: string } }) {
  const key = () => visualKey(props.chatId, props.part.id);
  const current = () => visuals[key()];
  const [refused, setRefused] = createSignal(false);
  const open = () => setRefused(!openVisual(props.chatId, props.part.id));
  const title = () => props.part.title?.trim() || "Visual";
  createEffect(() => onCleanup(trackMounted(key())));
  // Opens on mount and again after a reconnect retires the session; an error stays until the reader reopens.
  createEffect(() => {
    if (state.connection.kind === "connected" && !current() && !untrack(refused)) untrack(open);
  });
  return (
    <section class="x-visual" aria-label={title()}>
      <Show
        when={current()}
        fallback={
          <>
            <div class="x-visual-head">
              <span class="x-visual-title">{title()}</span>
              <Button size="sm" variant="outline" disabled={state.connection.kind !== "connected"} onClick={open}>
                Open visual
              </Button>
            </div>
            <Show when={refused()}>
              <p class="x-visual-note" role="status">Close another visual first; up to eight can be open at once.</p>
            </Show>
          </>
        }
      >
        {(v) => (
          <Switch>
            <Match when={v().error}>
              <div class="x-visual-error" role="alert">
                <span>Visual error: {v().error}</span>
                <Button size="sm" onClick={open}>Reopen visual</Button>
              </div>
            </Match>
            <Match when={!v().tree}>
              <p class="x-visual-note x-visual-loading" role="status">Loading visual…</p>
            </Match>
            <Match when={v().tree}>
              <div class="x-visual-body" aria-busy={v().busy} data-offline={v().offline ? "true" : undefined}>
                <VisualNode vkey={key()} path="" node={v().tree!} />
              </div>
              <Show when={v().offline}>
                <p class="x-visual-note x-visual-loading" role="status">Waiting for the connection to use this visual…</p>
              </Show>
            </Match>
          </Switch>
        )}
      </Show>
    </section>
  );
}

interface NodeProps {
  vkey: string;
  path: string;
  node: Json;
}

const children = (value: JsonObject) => (Array.isArray(value.children) ? value.children : []);
const label = (value: JsonObject, fallback = "") => (typeof value.label === "string" ? value.label : fallback);

function VisualNode(props: NodeProps): JSX.Element {
  const found = () => variant(props.node);
  const kind = () => found()?.[0] ?? "";
  const value = () => {
    const v = found()?.[1];
    return isObject(v) ? v : ({} as JsonObject);
  };
  const revision = () => visuals[props.vkey]?.revision ?? 0;
  const busy = () => visuals[props.vkey]?.busy ?? false;
  const run = (argument: Argument) => control(props.vkey, props.path, revision(), argument);
  const child = (index: number) => `${props.path}/${index}`;
  const wrapper = (): JSX.CSSProperties => {
    const node = isObject(props.node) ? props.node : {};
    return {
      flex: node.grow === true ? "1 1 0" : undefined,
      width: typeof node.width === "number" ? `${Math.min(4096, Math.max(0, node.width))}px` : undefined,
      "min-width": "0",
    };
  };

  return (
    <div class="x-vnode" style={wrapper()}>
      <Switch fallback={<span class="x-muted x-small">Unsupported visual element</span>}>
        <Match when={kind() === "Stack"}>
          {(() => {
            const style = (): JSX.CSSProperties => ({
              display: "flex",
              "flex-direction": value().axis === "Horizontal" ? "row" : "column",
              "flex-wrap": value().wrap === true ? "wrap" : undefined,
              gap: `${num(value().gap, 8)}px`,
              "align-items": value().align === "Center" ? "center" : value().align === "End" ? "flex-end" : undefined,
              "justify-content": { Center: "center", End: "flex-end", Between: "space-between" }[String(value().justify)] ?? undefined,
            });
            const clickable = () => typeof value().on_click === "number";
            return (
              <div
                class="x-vstack"
                style={style()}
                role={clickable() ? "button" : undefined}
                tabIndex={clickable() && !busy() ? 0 : undefined}
                aria-label={clickable() ? "Interactive group" : undefined}
                aria-disabled={clickable() ? busy() : undefined}
                data-clickable={clickable() ? "true" : undefined}
                onClick={(e) => {
                  if (!clickable()) return;
                  e.stopPropagation();
                  run({ kind: "none" });
                }}
                onKeyDown={(e) => {
                  if (clickable() && e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
                    e.preventDefault();
                    run({ kind: "none" });
                  }
                }}
              >
                <Index each={children(value())}>{(node, i) => <VisualNode vkey={props.vkey} path={child(i)} node={node()} />}</Index>
              </div>
            );
          })()}
        </Match>
        <Match when={kind() === "Grid"}>
          <div class="x-vgrid" style={{ "grid-template-columns": `repeat(${Math.min(12, Math.max(1, num(value().columns, 1)))}, minmax(0, 1fr))`, gap: `${num(value().gap, 8)}px` }}>
            <Index each={children(value())}>{(node, i) => <VisualNode vkey={props.vkey} path={child(i)} node={node()} />}</Index>
          </div>
        </Match>
        <Match when={kind() === "Text"}>
          <p
            class="x-vtext"
            data-truncate={value().truncate === true ? "true" : undefined}
            style={{ "font-size": `${textSize(value().size)}px`, "text-align": value().align === "Center" ? "center" : value().align === "End" ? "right" : undefined }}
          >
            <For each={Array.isArray(value().spans) ? (value().spans as Json[]).filter(isObject) : []}>
              {(span) => (
                <span
                  style={{
                    "font-family": span.mono === true || value().mono === true ? "var(--font-mono)" : undefined,
                    "font-weight": fontWeight(span.weight ?? value().weight),
                    "font-style": span.italic === true || value().italic === true ? "italic" : undefined,
                    color: cssColor(span.color ?? value().color),
                  }}
                >
                  {typeof span.text === "string" ? span.text : ""}
                </span>
              )}
            </For>
          </p>
        </Match>
        <Match when={kind() === "Stat"}>
          {(() => {
            // A tone marks the change, not the figure; a figure with no change takes the tone itself.
            const delta = () => (typeof value().delta === "string" && value().delta ? (value().delta as string) : null);
            const tone = () => (value().tone ? cssColor(value().tone) : null);
            return (
              <div class="x-vstat">
                <span class="x-vstat-row">
                  <span class="x-vstat-value" style={{ color: !delta() && tone() ? tone()! : undefined }}>
                    {typeof value().value === "string" ? (value().value as string) : String(value().value ?? "")}
                  </span>
                  <Show when={delta()}>
                    <span class="x-vstat-delta" style={{ color: tone() ?? undefined }}>{delta()}</span>
                  </Show>
                </span>
                <Show when={label(value())}><span class="x-vstat-label">{label(value())}</span></Show>
              </div>
            );
          })()}
        </Match>
        <Match when={kind() === "Rule"}>
          <hr class="x-vrule" />
        </Match>
        <Match when={kind() === "Space"}>
          <div style={{ height: `${Math.min(720, Math.max(0, num(found()?.[1], 8)))}px` }} />
        </Match>
        <Match when={kind() === "Button"}>
          <button type="button" class="btn" data-variant={value().primary === true ? "primary" : "default"} aria-disabled={busy()} onClick={(e) => (e.stopPropagation(), run({ kind: "none" }))}>
            {label(value(), "Button")}
          </button>
        </Match>
        <Match when={kind() === "Toggle"}>
          <label class="x-vtoggle">
            <button
              type="button"
              role="switch"
              class="toggle"
              aria-checked={value().value === true}
              aria-label={label(value(), "Toggle")}
              aria-disabled={busy()}
              onClick={(e) => (e.stopPropagation(), run({ kind: "bool", value: value().value !== true }))}
            />
            <span class="x-small">{label(value())}</span>
          </label>
        </Match>
        <Match when={kind() === "Tabs"}>
          <div class="x-vtabs" role="tablist" aria-label="Visual tabs">
            <For each={(Array.isArray(value().options) ? value().options as Json[] : []).filter((o): o is string => typeof o === "string")}>
              {(option) => (
                <button
                  type="button"
                  class="tab"
                  role="tab"
                  aria-selected={value().value === option}
                  aria-disabled={busy()}
                  onClick={(e) => (e.stopPropagation(), run({ kind: "text", value: option }))}
                >
                  {option}
                </button>
              )}
            </For>
          </div>
        </Match>
        <Match when={kind() === "Slider"}>
          <VisualSlider {...props} value={value()} busy={busy()} />
        </Match>
        <Match when={kind() === "Table"}>
          <VisualTable {...props} value={value()} />
        </Match>
        <Match when={kind() === "Chart"}>
          <VisualChart value={value()} />
        </Match>
        <Match when={kind() === "Link"}>
          {(() => {
            const url = () => (typeof value().url === "string" ? (value().url as string) : "");
            return (
              <Show when={/^https?:\/\//.test(url())} fallback={<span class="x-small">{label(value(), "Open link")}</span>}>
                <a class="x-vlink" href={url()} target="_blank" rel="noopener noreferrer">{label(value(), "Open link")}</a>
              </Show>
            );
          })()}
        </Match>
        <Match when={kind() === "Canvas"}>
          <VisualCanvas {...props} value={value()} />
        </Match>
        <Match when={kind() === "Scene" || kind() === "Shader"}>
          <p class="x-visual-note">{kind() === "Scene" ? "3D scenes" : "Shaders"} render in the desktop app; this browser view can't draw them yet.</p>
        </Match>
      </Switch>
    </div>
  );
}

function VisualSlider(props: NodeProps & { value: JsonObject; busy: boolean }) {
  const min = () => num(props.value.min, 0);
  const max = () => num(props.value.max, 1);
  // A zero step is continuous, as desktop's drag; keys then move a hundredth (a tenth with Page keys) of the range.
  const step = () => Math.max(0, num(props.value.step, 0));
  const send = (next: number) => {
    const clamped = Math.min(max(), Math.max(min(), next));
    const value = step() > 0 ? min() + Math.round((clamped - min()) / step()) * step() : clamped;
    control(props.vkey, props.path, visuals[props.vkey]?.revision ?? 0, { kind: "number", value });
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (step() > 0) return;
    const span = max() - min();
    const delta: Record<string, number> = { ArrowRight: span / 100, ArrowUp: span / 100, ArrowLeft: -span / 100, ArrowDown: -span / 100, PageUp: span / 10, PageDown: -span / 10 };
    const next = event.key === "Home" ? min() : event.key === "End" ? max() : delta[event.key] !== undefined ? current() + delta[event.key]! : null;
    if (next === null) return;
    event.preventDefault();
    send(next);
  };
  const target = () => {
    const t = visuals[props.vkey]?.sliderTarget;
    return t && t.path === props.path ? t.value : null;
  };
  const current = () => target() ?? num(props.value.value, min());
  const id = `x-vslider-${Math.random().toString(36).slice(2)}`;
  return (
    <div class="x-vslider">
      <div class="x-vslider-head">
        <label for={id} class="x-small">{label(props.value)}</label>
        <span class="x-small x-muted">{formatNumber(current(), props.value.format)}</span>
      </div>
      <input
        id={id}
        type="range"
        min={min()}
        max={max()}
        step={step() > 0 ? step() : "any"}
        value={current()}
        aria-label={label(props.value, "Value")}
        aria-valuetext={formatNumber(current(), props.value.format)}
        data-busy={props.busy ? "true" : undefined}
        onKeyDown={onKeyDown}
        onInput={(e) => send(Number(e.currentTarget.value))}
      />
    </div>
  );
}

function VisualTable(props: NodeProps & { value: JsonObject }) {
  const columns = () => (Array.isArray(props.value.columns) ? props.value.columns.filter(isObject) : []);
  const rows = () => (Array.isArray(props.value.rows) ? props.value.rows.filter(Array.isArray) : []);
  const count = () => Math.max(columns().length, ...rows().map((r) => r.length), 0);
  const indices = () => Array.from({ length: count() }, (_, i) => i);
  const align = (c: number) => {
    const a = columns()[c]?.align;
    return a === "Center" ? "center" : a === "End" ? "right" : "left";
  };
  const width = (c: number) => {
    const w = columns()[c]?.width;
    return typeof w === "number" && Number.isFinite(w) ? `${Math.min(4096, Math.max(0, w))}px` : undefined;
  };
  return (
    <table class="x-vtable" data-hover={props.value.hover === true ? "true" : undefined}>
      <Show when={columns().some((c) => typeof c.label === "string" && c.label !== "")}>
        <thead>
          <tr>
            <For each={indices()}>{(c) => <th scope="col" style={{ "text-align": align(c), width: width(c) }}>{label(columns()[c] ?? {})}</th>}</For>
          </tr>
        </thead>
      </Show>
      <tbody>
        <Index each={rows()}>
          {(cells, r) => (
            <tr>
              <For each={indices()}>
                {(c) => (
                  <td style={{ "text-align": align(c), width: width(c) }}>
                    <Show when={cells()[c] !== undefined}>
                      <VisualNode vkey={props.vkey} path={`${props.path}/${r}/${c}`} node={cells()[c]!} />
                    </Show>
                  </td>
                )}
              </For>
            </tr>
          )}
        </Index>
      </tbody>
    </table>
  );
}

let measureContext: CanvasRenderingContext2D | null = null;
function measureTick(text: string) {
  measureContext ??= document.createElement("canvas").getContext("2d");
  if (!measureContext) return text.length * 6;
  measureContext.font = `${TICK_SIZE}px ${getComputedStyle(document.body).fontFamily}`;
  return measureContext.measureText(text).width;
}

function VisualChart(props: { value: JsonObject }) {
  const [width, setWidth] = createSignal(0);
  const [pointer, setPointer] = createSignal<[number, number] | null>(null);
  let box!: HTMLDivElement;
  onMount(() => {
    const observer = new ResizeObserver(() => setWidth(box.clientWidth));
    observer.observe(box);
    setWidth(box.clientWidth);
    onCleanup(() => observer.disconnect());
  });
  const height = () => Math.min(720, Math.max(40, num(props.value.height, 200)));
  const base = () => planChart(props.value, Math.max(1, width()), height(), measureTick, null, null);
  const hover = () => {
    const p = pointer();
    return p ? hoveredCategory(props.value, base(), p[0], p[1]) : null;
  };
  const hoverPoint = () => {
    const p = pointer();
    return p ? nearestPoint(props.value, base(), p[0], p[1]) : null;
  };
  const plan = () => (hover() === null && hoverPoint() === null ? base() : planChart(props.value, Math.max(1, width()), height(), measureTick, hover(), hoverPoint()));
  const series = () => chartSeries(props.value);
  const labels = () => chartLabels(props.value);
  const named = () => series().some((s) => typeof s.name === "string" && s.name !== "");
  const showLegend = () => props.value.legend !== false && (series().length > 1 || hover() !== null) && named();
  const summary = () =>
    `${String(props.value.kind ?? "Chart")} chart of ${series().length} series` +
    (series().length ? `: ${series().map((s, i) => (typeof s.name === "string" && s.name ? s.name : `series ${i + 1}`)).join(", ")}` : "") +
    `, values from ${formatNumber(base().lo, props.value.format)} to ${formatNumber(base().hi, props.value.format)}.`;
  const readout = () => {
    const found = hoverPoint();
    if (!found) return null;
    const point = scatterPoint(props.value, found[0], found[1]);
    return point ? `${formatNumber(point[0], props.value.x_format)} · ${formatNumber(point[1], props.value.format)}` : null;
  };
  const band = () => {
    const i = hover();
    const p = plan();
    if (i === null || props.value.kind !== "Bar") return null;
    const horizontal = props.value.horizontal === true;
    const size = (horizontal ? p.plot.h : p.plot.w) / p.n;
    return horizontal ? { x: p.plot.x, y: p.plot.y + size * i, w: p.plot.w, h: size } : { x: p.plot.x + size * i, y: p.plot.y, w: size, h: p.plot.h };
  };
  const guide = () => {
    const i = hover();
    const p = plan();
    if (i === null || (props.value.kind !== "Line" && props.value.kind !== "Area")) return null;
    return p.n <= 1 ? p.plot.x + p.plot.w / 2 : p.plot.x + (p.plot.w * i) / (p.n - 1);
  };
  const move = (event: PointerEvent) => {
    const rect = box.getBoundingClientRect();
    setPointer([event.clientX - rect.left, event.clientY - rect.top]);
  };
  return (
    <figure class="x-vchart">
      {/* Always laid out, so a hover readout never shifts the plot. */}
      <figcaption class="x-vchart-legend">
          <Show when={hover() !== null && labels()[hover()!]}><span class="x-vchart-readout">{labels()[hover()!]}</span></Show>
          <Show when={readout()}><span class="x-vchart-point">{readout()}</span></Show>
          <Show when={showLegend() || hover() !== null}>
            <For each={series()}>
              {(s, i) => {
                const value = () => (hover() === null ? null : seriesValue(props.value, i(), hover()!));
                return (
                  <span class="x-vchart-key">
                    <span class="x-vchart-swatch" style={{ background: cssColor(seriesColor(s, i()), CHART_COLORS[0]) }} />
                    {typeof s.name === "string" ? s.name : ""}
                    <Show when={value() !== null}><span class="x-vchart-value">{formatNumber(value()!, props.value.format)}</span></Show>
                  </span>
                );
              }}
            </For>
          </Show>
      </figcaption>
      <div ref={box} class="x-vchart-plot" style={{ height: `${height()}px` }} onPointerMove={move} onPointerLeave={() => setPointer(null)}>
        <svg width={width()} height={height()} role="img" aria-label={summary()}>
          <Show when={band()}>{(b) => <rect class="x-vchart-band" x={b().x} y={b().y} width={b().w} height={b().h} rx={4} />}</Show>
          <For each={plan().grid}>{(g) => <line class="x-vchart-grid" x1={g[0]} y1={g[1]} x2={g[2]} y2={g[3]} />}</For>
          <Show when={guide() !== null}>
            <line class="x-vchart-guide" x1={guide()!} y1={plan().plot.y} x2={guide()!} y2={plan().plot.y + plan().plot.h} />
          </Show>
          <For each={plan().marks}>
            {(mark) => {
              const color = () => cssColor(mark.color, CHART_COLORS[0]);
              return (
                <Switch>
                  <Match when={mark.kind === "box" && mark}>{(m) => <rect x={m().x} y={m().y} width={Math.max(0, m().w)} height={Math.max(0, m().h)} rx={m().r} fill={color()} />}</Match>
                  <Match when={mark.kind === "dot" && mark}>{(m) => <circle cx={m().x} cy={m().y} r={m().r} fill={color()} />}</Match>
                  <Match when={mark.kind === "line" && mark}>
                    {(m) => <polyline points={m().points.map((p) => p.join(",")).join(" ")} fill="none" stroke={color()} stroke-width={1.75} stroke-linejoin="round" stroke-linecap="round" />}
                  </Match>
                  <Match when={mark.kind === "area" && mark}>
                    {(m) => <polygon points={m().points.map((p) => p.join(",")).join(" ")} fill={color()} fill-opacity={0.22} />}
                  </Match>
                </Switch>
              );
            }}
          </For>
          <For each={plan().labels}>
            {(l) => (
              <text class="x-vchart-tick" x={l.x} y={l.y} text-anchor={l.anchor} dominant-baseline={l.baseline === "middle" ? "central" : "hanging"}>
                {l.text}
              </text>
            )}
          </For>
        </svg>
      </div>
    </figure>
  );
}

function VisualCanvas(props: NodeProps & { value: JsonObject }) {
  let canvas!: HTMLCanvasElement;
  let area!: HTMLDivElement;
  const [visible, setVisible] = createSignal(false);
  const height = () => Math.min(720, Math.max(40, num(props.value.height, 200)));
  const data = () => visuals[props.vkey]?.canvases[props.path];
  const clickable = () => typeof props.value.on_click === "number";
  const internals = () => canvasInternals(props.vkey, props.path);

  onMount(() => {
    const observer = new IntersectionObserver(([entry]) => setVisible(!!entry?.isIntersecting));
    observer.observe(area);
    const resize = new ResizeObserver(() => paint());
    resize.observe(area);
    onCleanup(() => {
      observer.disconnect();
      resize.disconnect();
    });
  });
  // Internals are rebuilt per tree revision; keep them pointed at this element.
  createEffect(() => {
    void visuals[props.vkey]?.revision;
    const c = internals();
    if (!c) return;
    c.element = area;
    c.visible = visible();
  });
  createEffect(() => {
    void data()?.commands;
    paint();
  });

  function paint() {
    if (!canvas) return;
    const ratio = window.devicePixelRatio || 1;
    const width = area.clientWidth;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height() * ratio);
    canvas.style.width = `${width}px`;
    canvas.style.height = `${height()}px`;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
    ctx.clearRect(0, 0, width, height());
    for (const command of data()?.commands ?? []) paintCommand(ctx, command);
  }

  const point = (event: PointerEvent | MouseEvent) => {
    const rect = area.getBoundingClientRect();
    return {
      x: Math.min(Math.min(2048, rect.width), Math.max(0, event.clientX - rect.left)),
      y: Math.min(height(), Math.max(0, event.clientY - rect.top)),
    };
  };

  return (
    <div class="x-vcanvas">
      <div
        ref={area}
        class="x-vcanvas-area"
        style={{ height: `${height()}px` }}
        role={clickable() ? "button" : "img"}
        aria-label={clickable() ? "Interactive canvas" : "Canvas"}
        tabIndex={clickable() ? 0 : undefined}
        data-clickable={clickable() ? "true" : undefined}
        onPointerMove={(e) => {
          const c = internals();
          if (c) c.pointer = { ...point(e), down: e.buttons !== 0 };
        }}
        onPointerLeave={() => {
          const c = internals();
          if (c) c.pointer = null;
        }}
        onClick={(e) => {
          e.stopPropagation();
          if (!clickable()) return;
          const p = point(e);
          control(props.vkey, props.path, visuals[props.vkey]?.revision ?? 0, { kind: "point", value: [p.x, p.y] });
        }}
        onKeyDown={(e) => {
          if (!clickable() || (e.key !== "Enter" && e.key !== " ")) return;
          e.preventDefault();
          control(props.vkey, props.path, visuals[props.vkey]?.revision ?? 0, { kind: "point", value: [area.clientWidth / 2, height() / 2] });
        }}
      >
        <canvas ref={canvas} aria-hidden="true" />
      </div>
      <Show when={!data()?.commands.length && !data()?.error}>
        <span class="x-visual-note">Preparing canvas…</span>
      </Show>
      <Show when={data()?.error}>
        <span class="x-visual-note" role="status">{data()?.error}</span>
      </Show>
    </div>
  );
}

function paintCommand(ctx: CanvasRenderingContext2D, command: Json) {
  if (!isObject(command)) return;
  const text = command.Text;
  if (isObject(text)) {
    const size = textSize(text.size);
    ctx.font = `${fontWeight(text.weight)} ${size}px ${text.mono === true ? '"Geist Mono", monospace' : '"Geist", sans-serif'}`;
    ctx.fillStyle = canvasColor(text.color);
    ctx.textAlign = text.align === "Center" ? "center" : text.align === "End" ? "right" : "left";
    ctx.textBaseline = text.baseline === "Center" ? "middle" : text.baseline === "End" ? "bottom" : "top";
    ctx.fillText(typeof text.text === "string" ? text.text : "", num(text.x, 0), num(text.y, 0));
    return;
  }
  const stroke = isObject(command.Stroke) ? command.Stroke : null;
  const shape = stroke ?? (isObject(command.Fill) ? command.Fill : null);
  if (!shape) return;
  const points = (Array.isArray(shape.points) ? shape.points : []).filter(Array.isArray);
  if (!points.length) return;
  ctx.beginPath();
  points.forEach((p, i) => (i === 0 ? ctx.moveTo(num(p[0], 0), num(p[1], 0)) : ctx.lineTo(num(p[0], 0), num(p[1], 0))));
  if (!stroke || shape.closed === true) ctx.closePath();
  if (stroke) {
    const dash = num(shape.dash, 0);
    ctx.setLineDash(dash > 0 ? [dash, dash] : []);
    ctx.lineWidth = num(shape.width, 1);
    ctx.strokeStyle = canvasColor(shape.color);
    ctx.lineJoin = "round";
    ctx.stroke();
  } else {
    ctx.fillStyle = canvasColor(shape.color);
    ctx.fill();
  }
}
