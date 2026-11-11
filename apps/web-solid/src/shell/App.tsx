// The window frame: sidebar, main pane and optional right panel, matching desktop's layout.
import { ErrorBoundary, Match, Show, Switch, batch, createEffect, createSignal, lazy, on, onCleanup, type JSX } from "solid-js";
import { Dynamic } from "solid-js/web";
import { narrowLayout, setSidebarVisible, setView, sidebarVisible, view } from "@/core/view";
import { ToastRegion } from "@/ui/primitives";
import { Sidebar } from "@/features/sidebar";
import { Transcript } from "@/features/transcript";
import { Composer } from "@/features/composer";
import { RightPanel, TopBar } from "@/features/workspace";
import { ConnectionGate } from "@/features/connection";
import { ErrorScreen, Extras } from "@/features/extras";
// App-wide effects that must run before Settings is ever opened: session chimes, banners, reduced motion.
import "@/features/settings/alerts";
// The composer tray shows project setup progress, so the projects styles load with the frame.
import "@/features/projects/projects.css";

// Routes off the chat screen load on demand and are warmed once the first screen is idle.
const loadSettings = () => import("@/features/settings");
const loadProjects = () => import("@/features/projects");
const loadPlugins = () => import("@/features/plugins-cloud");
const Settings = lazy(() => loadSettings().then((m) => ({ default: m.Settings })));
const Usage = lazy(() => loadSettings().then((m) => ({ default: m.Usage })));
const Plugins = lazy(() => loadPlugins().then((m) => ({ default: m.Plugins })));
const NewProject = lazy(() => loadProjects().then((m) => ({ default: m.NewProject })));
const ProjectArchive = lazy(() => loadProjects().then((m) => ({ default: m.ProjectArchive })));
if (typeof window !== "undefined") {
  const warm = () => void Promise.all([loadSettings(), loadProjects(), loadPlugins()]).catch(() => {});
  if ("requestIdleCallback" in window) requestIdleCallback(warm, { timeout: 4000 });
  else setTimeout(warm, 2000);
}

export function App() {
  return (
    <ErrorBoundary fallback={(error, reset) => <Fatal error={error} reset={reset} />}>
      <ConnectionGate>
        <Frame />
      </ConnectionGate>
      <Extras />
      <ToastRegion />
    </ErrorBoundary>
  );
}

// Desktop panel metrics: crates/ui/src/settings.rs (widths), crates/ui/src/shell/panel_layout.rs (equal split) and
// WidthTween::panel in crates/ui/src/shell.rs (motion).
const SIDEBAR_MIN = 200;
const SIDEBAR_MAX = 300;
const SIDEBAR_DEFAULT = 232;
const RIGHT_MIN = 360;
const CHAT_MIN = 300;
const GUTTER = 4;
/** The titlebar band; seams start below it, like desktop's resize hitboxes. */
const TITLEBAR = 38;

/** A column moving `distance` px: 180ms up to the sidebar's width, growing with the square root of the distance to 240ms. */
const panelDuration = (distance: number) => 180 * Math.min(4 / 3, Math.max(1, Math.sqrt(distance / SIDEBAR_DEFAULT)));
/** Desktop's dialog entrance curve (motion::EASE): eases in gently and lands without overshoot. */
const PANEL_EASE = "cubic-bezier(0.25, 0.1, 0.25, 1)";
// Settings → Reduce motion resolves the system preference into this attribute (features/settings/alerts.ts).
const reducedMotion = () => document.documentElement.dataset.reduceMotion === "on";
const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/**
 * Eases an element's width to `to` from whatever it paints now, so a toggle mid-flight reverses from the visible
 * width. `direct` (dragging, window resizing, first paint) and reduced motion apply the width at once.
 */
function widthMotion() {
  let running: Animation | undefined;
  return (el: HTMLElement, to: number, options: { direct?: boolean; done?: () => void } = {}) => {
    const from = el.getBoundingClientRect().width;
    running?.cancel();
    running = undefined;
    el.style.width = `${to}px`;
    if (options.direct || reducedMotion() || Math.abs(to - from) < 0.5 || typeof el.animate !== "function") {
      options.done?.();
      return;
    }
    const animation = el.animate([{ width: `${from}px` }, { width: `${to}px` }], { duration: panelDuration(Math.abs(to - from)), easing: PANEL_EASE });
    running = animation;
    animation.onfinish = () => {
      if (running === animation) running = undefined;
      options.done?.();
    };
  };
}

/** The column seam's grabber: a 20px target centred on the gutter, with desktop's fading centre line on hover. */
function Seam(props: {
  label: string;
  style: JSX.CSSProperties;
  value: number;
  onDrag: (clientX: number) => void;
  onKey: (delta: number) => void;
  onReset: () => void;
  onActive: (active: boolean) => void;
}) {
  const [active, setActive] = createSignal(false);
  const start = (event: PointerEvent) => {
    if (event.button !== 0) return;
    event.preventDefault();
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    setActive(true);
    props.onActive(true);
    const move = (e: PointerEvent) => props.onDrag(e.clientX);
    const up = () => {
      setActive(false);
      props.onActive(false);
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  };
  return (
    <div
      class="app-seam"
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      aria-valuenow={Math.round(props.value)}
      tabIndex={0}
      data-active={active() ? "true" : undefined}
      style={props.style}
      onPointerDown={start}
      onDblClick={() => props.onReset()}
      onKeyDown={(event) => {
        if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
        event.preventDefault();
        const step = event.shiftKey ? 48 : 16;
        props.onKey(event.key === "ArrowLeft" ? -step : step);
      }}
    />
  );
}

function Frame() {
  const [dragging, setDragging] = createSignal(false);
  const [windowWidth, setWindowWidth] = createSignal(window.innerWidth);
  // Set around writes that follow the window edge or a key press, so the columns move with them at once.
  let snap = false;
  const direct = () => snap || dragging();

  const sidebarShown = () => sidebarVisible() && !narrowLayout();
  const sidebarWidth = () => clamp(view.preferences.sidebarWidth, SIDEBAR_MIN, SIDEBAR_MAX);
  const rightOpen = () => view.route.kind === "chat" && !!view.rightPanel;
  // The width app-main shares between its cards once the sidebar settles: the window less the sidebar and three gutters.
  const space = () => windowWidth() - (sidebarShown() ? sidebarWidth() : 0) - 3 * GUTTER;
  const rightMax = () => Math.max(RIGHT_MIN, space() - CHAT_MIN);
  const rightWidth = () => clamp(view.preferences.rightPanelWidth, RIGHT_MIN, rightMax());
  // Desktop's content_split: equal halves, the panel keeping its minimum and the conversation as much of its own as fits.
  const resplit = () => setView("preferences", "rightPanelWidth", Math.round(clamp(space() / 2, RIGHT_MIN, rightMax())));
  const snapTo = (write: () => void) => {
    snap = true;
    batch(write);
    snap = false;
  };

  // A column stays mounted while it eases closed. `enter` marks a mount caused by opening, which eases in from zero;
  // anything already open on first paint (a reload, crossing the narrow breakpoint) appears at its width.
  const [sidebarMounted, setSidebarMounted] = createSignal(sidebarShown());
  const [rightMounted, setRightMounted] = createSignal(rightOpen() && !narrowLayout());
  const enter = { sidebar: false, right: false };
  const take = (key: keyof typeof enter) => {
    const value = enter[key];
    enter[key] = false;
    return value;
  };
  createEffect(
    on(sidebarShown, (shown) => {
      if (shown && !sidebarMounted()) {
        enter.sidebar = true;
        setSidebarMounted(true);
      }
      if (!shown && narrowLayout()) setSidebarMounted(false);
      // Opening or closing a column re-splits the conversation and the panel, like desktop; both ease together.
      if (rightOpen() && !narrowLayout()) resplit();
    }, { defer: true }),
  );
  createEffect(
    on(rightOpen, (open) => {
      if (narrowLayout()) return;
      if (open) {
        resplit();
        if (!rightMounted()) {
          enter.right = true;
          setRightMounted(true);
        }
      }
    }, { defer: true }),
  );
  createEffect(on(narrowLayout, (narrow) => {
    setSidebarMounted(sidebarShown());
    setRightMounted(rightOpen() && !narrow);
  }, { defer: true }));
  const onResize = () =>
    // eslint-disable-next-line solid/reactivity -- runs from the window's resize event
    snapTo(() => {
      setWindowWidth(window.innerWidth);
      // Desktop keeps the halves equal while the window changes size; a dragged seam holds until then.
      if (rightOpen() && !narrowLayout()) resplit();
    });
  window.addEventListener("resize", onResize);
  onCleanup(() => window.removeEventListener("resize", onResize));

  // The drawer takes focus when it opens and gives it back to whatever opened it.
  let drawer: HTMLElement | undefined;
  let opener: HTMLElement | null = null;
  createEffect(
    on(
      () => narrowLayout() && view.drawerOpen,
      (open) => {
        if (open) {
          opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
          queueMicrotask(() => drawer?.focus());
        } else {
          // Closing by Escape or the scrim drops focus with the drawer; a pick that moved focus elsewhere keeps it.
          const lost = !document.activeElement || document.activeElement === document.body || !!drawer?.contains(document.activeElement);
          if (lost && opener?.isConnected) opener.focus();
          opener = null;
        }
      },
      { defer: true },
    ),
  );
  return (
    <div class="app" data-narrow={narrowLayout() ? "true" : undefined} data-resizing={dragging() ? "true" : undefined}>
      <Show
        when={narrowLayout()}
        fallback={
          <Show when={sidebarMounted()}>
            <Column
              tag="aside"
              class="app-sidebar"
              label="Sidebar"
              open={sidebarShown()}
              width={sidebarWidth()}
              direct={direct}
              enter={take("sidebar")}
              onClosed={() => setSidebarMounted(false)}
            >
              <div class="app-sidebar-inner" style={{ width: `${sidebarWidth()}px` }}>
                <Sidebar />
              </div>
            </Column>
            <Show when={sidebarShown()}>
              <Seam
                label="Resize sidebar"
                value={sidebarWidth()}
                style={{ left: `${sidebarWidth() + GUTTER / 2 - 10}px`, top: `${TITLEBAR}px` }}
                onActive={setDragging}
                onDrag={(x) => setView("preferences", "sidebarWidth", clamp(Math.round(x - GUTTER / 2), SIDEBAR_MIN, SIDEBAR_MAX))}
                onKey={(delta) => snapTo(() => setView("preferences", "sidebarWidth", clamp(sidebarWidth() + delta, SIDEBAR_MIN, SIDEBAR_MAX)))}
                onReset={() => batch(() => {
                  setView("preferences", "sidebarWidth", SIDEBAR_DEFAULT);
                  if (rightOpen()) resplit();
                })}
              />
            </Show>
          </Show>
        }
      >
        <Show when={sidebarVisible()}>
          <button type="button" class="app-scrim" aria-label="Close sidebar" tabIndex={-1} onClick={() => setSidebarVisible(false)} />
          <aside
            ref={drawer}
            class="app-sidebar"
            aria-label="Sidebar"
            tabIndex={-1}
            onKeyDown={(event) => {
              if (event.key === "Escape" && !event.defaultPrevented) {
                event.preventDefault();
                setSidebarVisible(false);
              }
            }}
          >
            <div class="app-sidebar-inner">
              <Sidebar />
            </div>
          </aside>
        </Show>
      </Show>
      <div class="app-main">
        <main class="app-pane">
          <ErrorBoundary fallback={(error, reset) => <Fatal error={error} reset={reset} />}>
            <Switch>
              <Match when={view.route.kind === "settings"}><Settings /></Match>
              <Match when={view.route.kind === "usage"}><Usage /></Match>
              <Match when={view.route.kind === "plugins"}><Plugins /></Match>
              <Match when={view.route.kind === "newProject"}><NewProject /></Match>
              <Match when={view.route.kind === "archive"}><ProjectArchive /></Match>
              <Match when={view.route.kind === "chat"}>
                <TopBar />
                <div class="app-chat">
                  <Transcript />
                  <Composer />
                </div>
              </Match>
            </Switch>
          </ErrorBoundary>
        </main>
        <Show
          when={!narrowLayout()}
          fallback={
            <Show when={rightOpen()}>
              <section class="app-right" aria-label="Workspace panel">
                <RightPanel />
              </section>
            </Show>
          }
        >
          <Show when={rightMounted()}>
            {/* The column includes the gutter before the card and eases its width; the card inside keeps its final
                width and stays anchored right, so opening uncovers it rather than reflowing it (desktop's mask). */}
            <Column
              tag="div"
              class="app-right-slot"
              open={rightOpen()}
              width={rightWidth() + GUTTER}
              direct={() => direct() || view.route.kind !== "chat"}
              enter={take("right")}
              onClosed={() => setRightMounted(false)}
            >
              <section class="app-right" style={{ width: `${rightWidth()}px` }} aria-label="Workspace panel">
                <RightPanel />
              </section>
            </Column>
            <Show when={rightOpen()}>
              <Seam
                label="Resize panel"
                value={rightWidth()}
                style={{ right: `${GUTTER + rightWidth() + GUTTER / 2 - 10}px`, top: `${TITLEBAR}px` }}
                onActive={setDragging}
                onDrag={(x) => setView("preferences", "rightPanelWidth", clamp(Math.round(windowWidth() - 1.5 * GUTTER - x), RIGHT_MIN, rightMax()))}
                // eslint-disable-next-line solid/reactivity -- a key handler
                onKey={(delta) => snapTo(() => setView("preferences", "rightPanelWidth", clamp(rightWidth() - delta, RIGHT_MIN, rightMax())))}
                onReset={resplit}
              />
            </Show>
          </Show>
        </Show>
      </div>
    </div>
  );
}

/** A column that eases its width open and closed (see `widthMotion`). */
function Column(props: {
  tag: "aside" | "div";
  class: string;
  label?: string;
  open: boolean;
  width: number;
  direct: () => boolean;
  enter: boolean;
  onClosed: () => void;
  children: JSX.Element;
}) {
  let el!: HTMLElement;
  const move = widthMotion();
  // eslint-disable-next-line solid/reactivity -- read once: whether this mount eases in
  const enter = props.enter;
  let first = true;
  createEffect(
    on(
      () => [props.open, props.width] as const,
      ([open, width]) => {
        const direct = (first && !enter) || props.direct();
        first = false;
        move(el, open ? width : 0, { direct, done: open ? undefined : () => props.onClosed() });
      },
    ),
  );
  return (
    <Dynamic component={props.tag} ref={(node: HTMLElement) => ((el = node), (node.style.width = enter ? "0px" : `${props.open ? props.width : 0}px`))} class={props.class} aria-label={props.label}>
      {props.children}
    </Dynamic>
  );
}

function Fatal(props: { error: unknown; reset: () => void }) {
  return <ErrorScreen error={props.error} reset={props.reset} />;
}
