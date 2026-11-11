// The right panel: a strip of open surfaces with a `+` menu, like desktop's right pane
// (crates/ui/src/shell/panel_menu.rs). Open surfaces stay mounted so terminals and editors keep state.
import { createEffect, createMemo, createSignal, For, lazy, Match, on, onCleanup, onMount, Show, Suspense, Switch } from "solid-js";
import { setView, view, type RightPanel as Panel } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { IconButton, Menu, Spinner } from "@/ui/primitives";
import { closePanelTab, openPanel, selectedTarget, targetMemoOptions, workspace } from "./store";
// Surfaces load when first opened; the chat screen does not need them.
const Changes = lazy(() => import("./Changes").then((m) => ({ default: m.Changes })));
const GitPanel = lazy(() => import("./Git").then((m) => ({ default: m.GitPanel })));
const History = lazy(() => import("./History").then((m) => ({ default: m.History })));
const PullRequest = lazy(() => import("./PullRequest").then((m) => ({ default: m.PullRequest })));
const Browser = lazy(() => import("./Browser").then((m) => ({ default: m.Browser })));

const Files = lazy(() => import("./files/Files").then((m) => ({ default: m.Files })));
const Terminal = lazy(() => import("./Terminal").then((m) => ({ default: m.Terminal })));

type Surface = Exclude<Panel, null>;

const SURFACES: Record<Surface, { label: string; icon: string; git?: boolean }> = {
  files: { label: "Files", icon: "folder" },
  pullRequest: { label: "PR", icon: "pull-request", git: true },
  changes: { label: "Changes", icon: "git-compare", git: true },
  git: { label: "Git", icon: "git-merge", git: true },
  history: { label: "History", icon: "git-branch", git: true },
  terminal: { label: "Terminal", icon: "terminal" },
  browser: { label: "Browser", icon: "globe" },
};
// Desktop order (crates/ui/src/shell/panel_menu.rs), with Git beside Changes and History last.
const ORDER: Surface[] = ["files", "pullRequest", "changes", "git", "terminal", "browser", "history"];

export function RightPanel() {
  const target = createMemo(selectedTarget, null, targetMemoOptions);
  const [expanded, setExpanded] = createSignal<number | null>(null);
  const available = (surface: Surface) => !SURFACES[surface].git || !!target()?.git;
  const choices = () => ORDER.filter((s) => available(s) && !workspace.tabs.includes(s));
  // While the panel eases closed it keeps showing the surface it had, like desktop's last frame behind the mask.
  const current = () => view.rightPanel ?? workspace.lastPanel;
  // Tabs scrolled out of the strip fade at that edge (desktop's tab_strip_region).
  let strip: HTMLDivElement | undefined;
  const [fade, setFade] = createSignal({ left: false, right: false });
  const measure = () => {
    if (!strip) return;
    const max = strip.scrollWidth - strip.clientWidth;
    setFade({ left: strip.scrollLeft > 1, right: strip.scrollLeft < max - 1 });
  };
  onMount(() => {
    if (!strip || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(strip);
    onCleanup(() => observer.disconnect());
  });
  // Keep the active surface's tab in view when the strip overflows, once a newly added tab has laid out. Only the
  // strip scrolls: scrollIntoView would also scroll the shell's clipping column mid-animation.
  createEffect(
    on(
      () => [view.rightPanel, workspace.tabs.length] as const,
      ([panel]) =>
        requestAnimationFrame(() => {
          const tab = panel && document.getElementById(`ws-tab-${panel}`)?.parentElement;
          if (strip && tab) {
            const box = strip.getBoundingClientRect();
            const chip = tab.getBoundingClientRect();
            if (chip.left < box.left) strip.scrollLeft -= box.left - chip.left;
            else if (chip.right > box.right) strip.scrollLeft += chip.right - box.right;
          }
          measure();
        }),
    ),
  );

  // Expanding gives the panel all the width the conversation can spare (the shell keeps its minimum); collapsing
  // returns to the width it had.
  const toggleExpand = () => {
    const previous = expanded();
    if (previous !== null) {
      setView("preferences", "rightPanelWidth", previous);
      setExpanded(null);
    } else {
      setExpanded(view.preferences.rightPanelWidth);
      setView("preferences", "rightPanelWidth", window.innerWidth);
    }
  };

  return (
    <div class="ws-right">
      <div class="ws-right-strip">
        <div
          class="ws-tabs"
          ref={strip}
          role="tablist"
          aria-label="Panel surfaces"
          data-fade-left={fade().left ? "true" : undefined}
          data-fade-right={fade().right ? "true" : undefined}
          onScroll={measure}
        >
          <For each={workspace.tabs.filter(available)}>
            {(surface) => (
              <div class="ws-tab" data-active={current() === surface ? "true" : undefined}>
                <button
                  type="button"
                  role="tab"
                  class="ws-tab-main"
                  id={`ws-tab-${surface}`}
                  aria-selected={current() === surface}
                  aria-controls={`ws-surface-${surface}`}
                  aria-label={SURFACES[surface].label}
                  title={SURFACES[surface].label}
                  onClick={() => openPanel(surface)}
                  onAuxClick={(e) => e.button === 1 && closePanelTab(surface)}
                >
                  <Icon name={SURFACES[surface].icon} size={14} class="ws-tab-icon" />
                  <span>{SURFACES[surface].label}</span>
                </button>
                <button type="button" class="ws-tab-close" aria-label={`Close ${SURFACES[surface].label}`} title="Close tab" onClick={() => closePanelTab(surface)}>
                  <Icon name="close" size={12} />
                </button>
              </div>
            )}
          </For>
        </div>
        <Show when={choices().length}>
          <Menu
            placement="bottom-start"
            trigger={(p) => (
              <button {...p} type="button" class="ws-strip-add" aria-label="Add to panel" title="Add panel">
                <Icon name="plus" size={14} />
              </button>
            )}
            items={choices().map((surface) => ({ label: SURFACES[surface].label, icon: SURFACES[surface].icon, onSelect: () => openPanel(surface) }))}
          />
        </Show>
        <div class="ws-strip-spacer" />
        <IconButton
          icon={expanded() !== null ? "collapse-arrows" : "expand-arrows"}
          label={expanded() !== null ? "Collapse panel" : "Expand panel"}
          size="sm"
          iconSize={14}
          class="ws-strip-btn"
          onClick={toggleExpand}
        />
        <IconButton icon="close" label="Close panel" size="sm" iconSize={14} class="ws-strip-btn" onClick={() => setView("rightPanel", null)} />
      </div>
      <div class="ws-right-body">
        <Show when={target()?.chatId} fallback={<div class="ws-empty ws-muted">Select a conversation to see its workspace.</div>} keyed>
          {(chatId: string) => (
            <For each={workspace.tabs.filter(available)}>
              {(surface) => (
                <section
                  class="ws-surface"
                  data-chat={chatId}
                  id={`ws-surface-${surface}`}
                  role="tabpanel"
                  aria-labelledby={`ws-tab-${surface}`}
                  hidden={current() !== surface}
                >
                  <Suspense fallback={<div class="ws-empty"><Spinner label="Loading" /></div>}>
                    <Switch>
                      <Match when={surface === "files"}><Files target={target()!} /></Match>
                      <Match when={surface === "changes"}><Changes target={target()!} /></Match>
                      <Match when={surface === "git"}><GitPanel target={target()!} /></Match>
                      <Match when={surface === "history"}><History target={target()!} active={view.rightPanel === "history"} /></Match>
                      <Match when={surface === "pullRequest"}><PullRequest target={target()!} active={view.rightPanel === "pullRequest"} /></Match>
                      <Match when={surface === "terminal"}><Terminal target={target()!} active={view.rightPanel === "terminal"} /></Match>
                      <Match when={surface === "browser"}><Browser target={target()!} /></Match>
                    </Switch>
                  </Suspense>
                </section>
              )}
            </For>
          )}
        </Show>
      </div>
    </div>
  );
}
