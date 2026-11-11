// The terminal surface: per-chat tabs over engine PTYs, rendered with xterm.
// Same data path as crates/ui/src/terminal/panel.rs: OpenTerminal, then SubscribeTerminal resuming from
// afterSeq with backoff; keystrokes coalesce 12 ms before WriteTerminal; resizes debounce 80 ms.
// Leaving a chat detaches; the PTY keeps running and its retained output replays on return.
import { FitAddon } from "@xterm/addon-fit";
import { Terminal as XTerm, type ITheme } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { batch, createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Subscription } from "@/core/rpc";
import { state } from "@/core/store";
import { resolvedTheme } from "@/shell/theme";
import { Icon } from "@/ui/Icon";
import { Button, IconButton } from "@/ui/primitives";
import { Empty } from "./parts";
import { actionTerminals, claimActionTerminal } from "@/features/projects/actions";
import { errorText, keyedWatch, type Target } from "./store";
import type { TerminalEvent, TerminalSession } from "./types";
import { decodeBase64Bytes } from "./util";

const COALESCE_MS = 12;
const RESIZE_DEBOUNCE_MS = 80;
const backoff = (attempt: number) => Math.min(8000, 500 * 2 ** Math.min(attempt, 4));

interface TermTab {
  key: number;
  terminalId: string | null;
  title: string;
  opening: boolean;
  exited: number | null;
  error: string | null;
}

interface ChatTerminals {
  tabs: TermTab[];
  active: number | null;
  restored: boolean;
  restoring: boolean;
  error: string | null;
}

const [terminals, setTerminals] = createStore<Record<string, ChatTerminals>>({});
let nextKey = 1;

function chatTerminals(chatId: string) {
  if (!terminals[chatId]) setTerminals(chatId, { tabs: [], active: null, restored: false, restoring: false, error: null });
  return terminals[chatId]!;
}

export function shellTitle(shell: string) {
  const name = shell.split(/[\\/]/).pop()?.trim();
  return name || "terminal";
}

function addTab(chatId: string, tab: Omit<TermTab, "key">) {
  const key = nextKey++;
  setTerminals(chatId, produce((c) => {
    c.tabs.push({ key, ...tab });
    c.active = key;
  }));
  return key;
}

function updateTab(chatId: string, key: number, patch: Partial<TermTab>) {
  const index = terminals[chatId]?.tabs.findIndex((t) => t.key === key) ?? -1;
  if (index >= 0) setTerminals(chatId, "tabs", index, patch);
}

/** Reattach the chat's retained PTYs, or open a first one. */
function restore(target: Target) {
  const chatId = target.chatId;
  const current = chatTerminals(chatId);
  if (current.restored || current.restoring) return;
  setTerminals(chatId, { restoring: true, error: null });
  rpc.call<TerminalSession[]>(M.LIST_TERMINALS, { chatId, targetDeviceId: target.deviceId }).then(
    (sessions) =>
      batch(() => {
        setTerminals(chatId, { restoring: false, restored: true });
        for (const session of Array.isArray(sessions) ? sessions : []) {
          if (terminals[chatId]!.tabs.some((t) => t.terminalId === session.id)) continue;
          addTab(chatId, { terminalId: session.id, title: shellTitle(session.shell), opening: false, exited: null, error: null });
        }
        if (!terminals[chatId]!.tabs.length) newTab(chatId);
      }),
    (error) => setTerminals(chatId, { restoring: false, error: `Couldn't load retained terminals: ${errorText(error)}` }),
  );
}

function newTab(chatId: string) {
  const count = terminals[chatId]?.tabs.length ?? 0;
  addTab(chatId, { terminalId: null, title: `Terminal ${count + 1}`, opening: false, exited: null, error: null });
}

/** Allocate the PTY. Runs in the store so a remount while it is in flight never opens a second one. */
function openSession(target: Target, key: number, cols: number, rows: number) {
  const chatId = target.chatId;
  updateTab(chatId, key, { opening: true, error: null });
  rpc.call<TerminalSession>(M.OPEN_TERMINAL, { chatId, cols, rows, targetDeviceId: target.deviceId }).then(
    (session) => {
      if (!terminals[chatId]?.tabs.some((t) => t.key === key)) {
        // The tab closed while the PTY was being created; it belongs to nobody.
        void rpc.call(M.CLOSE_TERMINAL, { terminalId: session.id, targetDeviceId: target.deviceId }).catch(() => {});
        return;
      }
      updateTab(chatId, key, { terminalId: session.id, title: shellTitle(session.shell), opening: false });
    },
    (error) => updateTab(chatId, key, { opening: false, error: `Failed to open terminal: ${errorText(error)}` }),
  );
}

function closeTab(target: Target, key: number) {
  const chatId = target.chatId;
  const tabs = terminals[chatId]?.tabs ?? [];
  const index = tabs.findIndex((t) => t.key === key);
  const tab = tabs[index];
  if (!tab) return;
  if (tab.terminalId) void rpc.call(M.CLOSE_TERMINAL, { terminalId: tab.terminalId, targetDeviceId: target.deviceId }).catch(() => {});
  setTerminals(chatId, produce((c) => {
    c.tabs.splice(index, 1);
    if (c.active === key) c.active = c.tabs[Math.min(index, c.tabs.length - 1)]?.key ?? null;
  }));
}

/** xterm colours from the desktop terminal palette (crates/theme/src/builtins.rs). */
function xtermTheme(dark: boolean): ITheme {
  const ansi = dark
    ? ["#242424", "#f87171", "#4ade80", "#facc15", "#60a5fa", "#c084fc", "#22d3ee", "#d4d4d8", "#52525b", "#fca5a5", "#86efac", "#fde047", "#93c5fd", "#d8b4fe", "#67e8f9", "#fafafa"]
    : ["#1f1f1f", "#dc2626", "#16a34a", "#b45309", "#2563eb", "#9333ea", "#0e7490", "#3f3f46", "#71717a", "#b91c1c", "#15803d", "#92400e", "#1d4ed8", "#7e22ce", "#155e75", "#18181b"];
  const [black, red, green, yellow, blue, magenta, cyan, white, brightBlack, brightRed, brightGreen, brightYellow, brightBlue, brightMagenta, brightCyan, brightWhite] = ansi;
  return {
    background: dark ? "#121212" : "#fafafa",
    foreground: dark ? "#e8e8e8" : "#2a2a2a",
    cursor: dark ? "#e8e8e8" : "#2a2a2a",
    cursorAccent: dark ? "#121212" : "#fafafa",
    selectionBackground: dark ? "rgba(137, 202, 33, 0.3)" : "rgba(129, 191, 29, 0.3)",
    black, red, green, yellow, blue, magenta, cyan, white,
    brightBlack, brightRed, brightGreen, brightYellow, brightBlue, brightMagenta, brightCyan, brightWhite,
  };
}

export function Terminal(props: { target: Target; active: boolean }) {
  // eslint-disable-next-line solid/reactivity -- RightPanel remounts this per chat
  const chatId = props.target.chatId;
  const data = () => terminals[chatId];
  const connected = () => state.connection.kind === "connected";

  keyedWatch(
    () => [connected(), state.epoch] as const,
    ([live]) => {
      if (live) restore(props.target);
    },
  );

  // A project action's PTY is not in ListTerminals; the panel takes it over as a tab.
  createEffect(() => {
    for (const started of actionTerminals()) {
      if (started.chatId !== chatId) continue;
      claimActionTerminal(started.terminalId);
      chatTerminals(chatId);
      if (!terminals[chatId]!.tabs.some((t) => t.terminalId === started.terminalId))
        addTab(chatId, { terminalId: started.terminalId, title: started.title, opening: false, exited: null, error: null });
    }
  });

  const retry = () => {
    setTerminals(chatId, { restored: false, error: null });
    restore(props.target);
  };

  const onTabKeys = (event: KeyboardEvent) => {
    if (event.key !== "ArrowRight" && event.key !== "ArrowLeft") return;
    const tabs = data()?.tabs ?? [];
    const index = tabs.findIndex((t) => t.key === data()?.active);
    const next = tabs[(index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length];
    if (!next) return;
    event.preventDefault();
    setTerminals(chatId, "active", next.key);
    (event.currentTarget as HTMLElement).querySelector<HTMLElement>(`[data-key="${next.key}"]`)?.focus();
  };

  return (
    <div class="ws-surface-col ws-terminal">
      <div class="ws-term-tabs">
        <div class="ws-term-tablist" role="tablist" aria-label="Terminals" onKeyDown={onTabKeys}>
          <For each={data()?.tabs ?? []}>
            {(tab) => (
              <div class="ws-term-tab" data-active={data()?.active === tab.key ? "true" : undefined}>
                <button
                  type="button"
                  role="tab"
                  class="ws-term-tab-main"
                  data-key={tab.key}
                  aria-selected={data()?.active === tab.key}
                  tabIndex={data()?.active === tab.key ? 0 : -1}
                  onClick={() => setTerminals(chatId, "active", tab.key)}
                  onAuxClick={(e) => e.button === 1 && closeTab(props.target, tab.key)}
                  title={tab.title}
                >
                  <Icon name="terminal" size={12} class="ws-muted" />
                  <span class="ws-truncate">{tab.title}</span>
                  <Show when={tab.exited !== null}>
                    <span class="ws-faint">· exited</span>
                  </Show>
                </button>
                <button type="button" class="ws-tab-close" aria-label={`Close ${tab.title}`} title="Close terminal" onClick={() => closeTab(props.target, tab.key)}>
                  <Icon name="close" size={12} />
                </button>
              </div>
            )}
          </For>
        </div>
        <IconButton icon="plus" label="New terminal" size="sm" iconSize={14} disabled={!connected() || props.target.archived} onClick={() => newTab(chatId)} />
      </div>
      <Show when={data()?.error}>
        <Empty icon="terminal" title="Terminal unavailable" detail={data()?.error ?? undefined}>
          <Button size="sm" onClick={retry}>Retry</Button>
        </Empty>
      </Show>
      <Show when={!data()?.error && data()?.restored && !data()?.tabs.length}>
        <Empty icon="terminal" title="No terminals open">
          <Button size="sm" icon="plus" disabled={!connected()} onClick={() => newTab(chatId)}>New terminal</Button>
        </Empty>
      </Show>
      <Show when={!data()?.restored && !data()?.error}>
        <Empty icon="terminal" title={connected() ? "Opening terminal…" : "Waiting for the connection…"} />
      </Show>
      <div class="ws-term-body">
        <For each={data()?.tabs ?? []}>
          {(tab) => <TerminalView target={props.target} tab={tab} visible={props.active && data()?.active === tab.key} />}
        </For>
      </div>
    </div>
  );
}

function TerminalView(props: { target: Target; tab: TermTab; visible: boolean }) {
  // eslint-disable-next-line solid/reactivity -- RightPanel remounts this per chat
  const chatId = props.target.chatId;
  // eslint-disable-next-line solid/reactivity -- each view is keyed by its tab
  const key = props.tab.key;
  let host!: HTMLDivElement;
  let term: XTerm | undefined;
  let fit: FitAddon | undefined;
  let lastSeq = 0;
  // eslint-disable-next-line solid/reactivity -- initial state; later exits arrive on the stream
  let exited = props.tab.exited !== null;
  const [ready, setReady] = createSignal(false);
  const connected = () => state.connection.kind === "connected";

  // Input coalescing.
  let pending: Uint8Array[] = [];
  let flushTimer: ReturnType<typeof setTimeout> | undefined;
  const encoder = new TextEncoder();
  const flush = () => {
    flushTimer = undefined;
    const id = props.tab.terminalId;
    if (!pending.length) return;
    if (!id) {
      // OpenTerminal still in flight: keep the bytes and try again shortly.
      if (!exited && !props.tab.error) flushTimer = setTimeout(flush, 50);
      return;
    }
    const size = pending.reduce((n, b) => n + b.length, 0);
    const bytes = new Uint8Array(size);
    let at = 0;
    for (const chunk of pending) {
      bytes.set(chunk, at);
      at += chunk.length;
    }
    pending = [];
    void rpc.call(M.WRITE_TERMINAL, { terminalId: id, data: bytesToBase64(bytes), targetDeviceId: props.target.deviceId }).catch(() => {});
  };
  const queue = (bytes: Uint8Array) => {
    if (exited) return;
    pending.push(bytes);
    if (!flushTimer) flushTimer = setTimeout(flush, COALESCE_MS);
  };

  let resizeTimer: ReturnType<typeof setTimeout> | undefined;
  const sendResize = () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      const id = props.tab.terminalId;
      if (!id || !term || exited) return;
      void rpc.call(M.RESIZE_TERMINAL, { terminalId: id, cols: term.cols, rows: term.rows, targetDeviceId: props.target.deviceId }).catch(() => {});
    }, RESIZE_DEBOUNCE_MS);
  };

  const refit = () => {
    if (!term || !fit || !host.offsetWidth || !host.offsetHeight) return;
    try {
      fit.fit();
    } catch {
      // A detached or zero-sized host has no geometry yet.
    }
  };

  onMount(() => {
    term = new XTerm({
      fontFamily: '"Geist Mono", ui-monospace, monospace',
      fontSize: 13,
      lineHeight: 1.25,
      cursorBlink: true,
      scrollback: 5000,
      allowProposedApi: false,
      theme: xtermTheme(resolvedTheme() === "dark"),
      screenReaderMode: false,
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    term.textarea?.setAttribute("aria-label", `Terminal, ${props.tab.title}`);
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") return true;
      const mod = event.ctrlKey && event.shiftKey;
      if (mod && event.key.toLowerCase() === "c" && term?.hasSelection()) {
        void navigator.clipboard?.writeText(term.getSelection()).catch(() => {});
        return false;
      }
      // Let the browser deliver the paste event, which xterm turns into input.
      if (mod && event.key.toLowerCase() === "v") return false;
      // The app's terminal toggle stays global, as on desktop.
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && event.key.toLowerCase() === "j") return false;
      return true;
    });
    term.onData((text) => queue(encoder.encode(text)));
    term.onBinary((text) => queue(Uint8Array.from(text, (c) => c.charCodeAt(0) & 0xff)));
    term.onResize(sendResize);
    refit();
    const observer = new ResizeObserver(() => refit());
    observer.observe(host);
    setReady(true);
    onCleanup(() => {
      observer.disconnect();
      clearTimeout(flushTimer);
      clearTimeout(resizeTimer);
      flush();
      term?.dispose();
      term = undefined;
    });
  });

  createEffect(() => {
    const dark = resolvedTheme() === "dark";
    if (term) term.options.theme = xtermTheme(dark);
  });

  createEffect(
    on(
      () => props.visible,
      (visible) => {
        if (!visible || !term) return;
        queueMicrotask(() => {
          refit();
          term?.focus();
        });
      },
    ),
  );

  // Allocate the PTY once the grid has a size.
  createEffect(() => {
    if (!ready() || !connected()) return;
    const tab = props.tab;
    if (tab.terminalId || tab.opening || tab.error || tab.exited !== null) return;
    refit();
    openSession(props.target, key, term?.cols ?? 80, term?.rows ?? 24);
  });

  createEffect(() => {
    const error = props.tab.error;
    if (error && term) term.write(`\x1b[31m${error}\x1b[0m\r\n`);
  });

  // Stream output, resuming from the last sequence after drops and reconnects.
  keyedWatch(
    () => [ready() ? props.tab.terminalId : null, connected(), state.epoch] as const,
    ([id, live]) => {
      if (!id || !live || exited) return;
      let sub: Subscription | undefined;
      let timer: ReturnType<typeof setTimeout> | undefined;
      let attempt = 0;
      let stopped = false;
      const open = () => {
        sub = rpc.subscribe<TerminalEvent>(
          M.SUBSCRIBE_TERMINAL,
          { terminalId: id, afterSeq: lastSeq, targetDeviceId: props.target.deviceId },
          (event) => {
            attempt = 0;
            if (event.seq <= lastSeq) return;
            lastSeq = event.seq;
            if (event.type === "data") term?.write(decodeBase64Bytes(event.data));
            else {
              exited = true;
              term?.write(`\r\n\x1b[90m[process exited ${event.exitCode}]\x1b[0m\r\n`);
              updateTab(chatId, key, { exited: event.exitCode });
            }
          },
          () => {
            if (stopped || exited || !rpc.connected) return;
            timer = setTimeout(open, backoff(attempt++));
          },
        );
      };
      open();
      return {
        cancel() {
          stopped = true;
          clearTimeout(timer);
          sub?.cancel();
        },
      };
    },
  );

  return (
    <div
      class="ws-term-view"
      role="tabpanel"
      aria-label={props.tab.title}
      hidden={!props.visible}
      data-exited={props.tab.exited !== null ? "true" : undefined}
    >
      <div class="ws-term-host" ref={host} />
    </div>
  );
}

function bytesToBase64(bytes: Uint8Array) {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}
