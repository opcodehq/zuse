// Continue a recent agent conversation found on the host as a Zuse thread.
// Same flow as crates/viewport/src/import_chat.rs: list on open, import with a stable request id so a retry is idempotent.
import { createEffect, createMemo, createRoot, createSignal, For, type JSX, onCleanup, onMount, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { RpcError } from "@/core/rpc";
import { state } from "@/core/store";
import { openChat, view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { Spinner } from "@/ui/primitives";
import { Skeleton } from "./feedback";
import { modalKeys } from "./modal";
import { closeOverlay, openImportChat, overlayAnchor } from "./overlay";
import { errorText, harnessIcon, harnessName, timeAgo } from "./util";

const LIST = "ListExternalChats";
const IMPORT = "ImportExternalChat";
const TEMPORARY = new Set(["t", "temp", "temporary", "temporary folder"]);

interface ExternalChat {
  id: string;
  provider: string;
  title: string;
  preview: string;
  projectPath: string;
  projectName: string;
  updatedAt: number;
  available: boolean;
}
interface Imported {
  chat: { id: string };
  project: { id: string };
}

const importable = (row: ExternalChat) =>
  row.available && row.projectPath.trim() !== "" && !TEMPORARY.has(row.projectName.trim().toLowerCase());

/** The host a new thread would start on: the draft project's device, else this engine. */
function targetDevice() {
  const path = view.draftProjectPath;
  const space = path ? Object.values(state.spaces).find((s) => s.path === path) : undefined;
  return space && space.deviceId !== state.localDeviceId ? space.deviceId : null;
}

/** Open the imported thread once the chats watch has delivered it, so requests route to its host. */
function openWhenSynced(chatId: string) {
  if (state.chats[chatId]) return openChat(chatId);
  createRoot((dispose) => {
    const timer = setTimeout(() => {
      dispose();
      openChat(chatId);
    }, 10_000);
    createEffect(() => {
      if (!state.chats[chatId]) return;
      clearTimeout(timer);
      dispose();
      openChat(chatId);
    });
  });
}

/** A chip for the new-chat landing that opens the import dialog. */
export function ImportChatButton() {
  return (
    <button type="button" class="x-chip" onClick={() => openImportChat()} disabled={state.connection.kind !== "connected"}>
      <Icon name="history" size={14} />
      Import chat
    </button>
  );
}

const MENU_WIDTH = 288;
const MARGIN = 8;

/** Desktop's import menu (crates/viewport/src/import_chat.rs, mounted by crates/ui's landing): a small card above its chip.
 * Opened without a chip (from the palette), the same card sits centred over a scrim. */
export function ImportChat() {
  const [rows, setRows] = createSignal<ExternalChat[]>([]);
  const [loading, setLoading] = createSignal(true);
  const [error, setError] = createSignal<string | null>(null);
  const [importing, setImporting] = createSignal<string | null>(null);
  const [search, setSearch] = createSignal("");
  const [active, setActive] = createSignal(0);
  const device = targetDevice();
  const anchor = overlayAnchor();
  const [place, setPlace] = createSignal<JSX.CSSProperties>({});
  let attempt: { id: string; requestId: string } | null = null;
  let input!: HTMLInputElement;
  let list!: HTMLDivElement;
  let card: HTMLDivElement | undefined;
  const trap = modalKeys(() => card, () => closeOverlay());
  let alive = true;
  onCleanup(() => (alive = false));

  // Bottom-left on the chip, 4px above it, kept 8px inside the window; below it when there is no room above.
  function position() {
    if (!anchor || !card) return;
    const chip = anchor.getBoundingClientRect();
    const width = Math.min(MENU_WIDTH, window.innerWidth - MARGIN * 2);
    const left = Math.min(Math.max(MARGIN, chip.left), window.innerWidth - width - MARGIN);
    const above = chip.top - 4 - MARGIN;
    const below = window.innerHeight - chip.bottom - 4 - MARGIN;
    const height = card.offsetHeight;
    setPlace(
      height <= above || above >= below
        ? { left: `${left}px`, bottom: `${window.innerHeight - chip.top + 4}px`, width: `${width}px`, "max-height": `${above}px` }
        : { left: `${left}px`, top: `${chip.bottom + 4}px`, width: `${width}px`, "max-height": `${below}px` },
    );
  }

  async function load() {
    setLoading(true);
    setError(null);
    try {
      const found = await rpc.call<ExternalChat[]>(LIST, { targetDeviceId: device });
      if (!alive) return;
      setRows((Array.isArray(found) ? found : []).filter(importable));
    } catch (e) {
      if (alive) setError(errorText(e, "Couldn't read provider chats."));
    } finally {
      if (alive) setLoading(false);
    }
  }
  onMount(() => {
    input.focus();
    void load();
    if (!anchor) return;
    position();
    const observer = new ResizeObserver(position);
    if (card) observer.observe(card);
    window.addEventListener("resize", position);
    onCleanup(() => {
      observer.disconnect();
      window.removeEventListener("resize", position);
    });
  });

  const filtered = createMemo(() => {
    const query = search().trim().toLowerCase();
    if (!query) return rows();
    return rows().filter((row) =>
      [row.title, row.preview, row.projectName, row.projectPath, harnessName(row.provider)].some((field) => field.toLowerCase().includes(query)),
    );
  });
  createEffect(() => {
    search();
    setActive(0);
    if (list) list.scrollTop = 0;
  });

  async function choose(row: ExternalChat) {
    if (importing()) return;
    // Retrying the same row reuses its request id, so the host recovers the same import.
    if (attempt?.id !== row.id) attempt = { id: row.id, requestId: crypto.randomUUID() };
    setImporting(row.id);
    setError(null);
    try {
      const imported = await rpc.call<Imported>(IMPORT, { ...attempt, targetDeviceId: device });
      if (!alive) return;
      if (!imported?.chat?.id) throw new Error("The import response was interrupted. Retry to recover it.");
      attempt = null;
      closeOverlay(false);
      openWhenSynced(imported.chat.id);
    } catch (e) {
      if (!alive) return;
      setError(
        e instanceof RpcError && e.outcomeUnknown
          ? "The connection dropped during the import. Retry to recover it."
          : errorText(e, "Import was interrupted. Retry to recover it."),
      );
    } finally {
      if (alive) setImporting(null);
    }
  }

  const optionId = (index: number) => `x-import-${index}`;
  function onKeyDown(event: KeyboardEvent) {
    const count = filtered().length;
    let next = active();
    switch (event.key) {
      case "Escape":
        closeOverlay();
        break;
      case "ArrowDown":
        next = Math.min(count - 1, active() + 1);
        break;
      case "ArrowUp":
        next = Math.max(0, active() - 1);
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = count - 1;
        break;
      case "Enter": {
        const row = filtered()[active()];
        if (row) void choose(row);
        break;
      }
      case "Tab":
        // Rows are options of the field; Tab only cycles the card's own controls.
        trap(event);
        return;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (next !== active() && next >= 0) {
      setActive(next);
      document.getElementById(optionId(next))?.scrollIntoView({ block: "nearest" });
    }
  }


  const menu = (
    <div
      ref={card}
      class="x-import"
      data-placement={anchor ? "anchored" : "centred"}
      style={place()}
      role="dialog"
      aria-modal={anchor ? undefined : "true"}
      aria-labelledby="x-import-title"
      onKeyDown={onKeyDown}
    >
      <div class="x-import-head">
        <span id="x-import-title" class="x-import-label">Recent provider chats</span>
        <Show when={device}>
          <span class="x-import-host">{state.devices[device!]?.name ?? "Project's computer"}</span>
        </Show>
      </div>
      <label class="x-import-search">
        <Icon name="magnifer" size={14} />
        <input
          ref={input}
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls="x-import-list"
          aria-activedescendant={filtered().length ? optionId(active()) : undefined}
          aria-label="Search chats"
          placeholder="Search chats…"
          autocomplete="off"
          spellcheck={false}
          value={search()}
          onInput={(e) => setSearch(e.currentTarget.value)}
        />
      </label>
      <div ref={list} id="x-import-list" class="x-import-list" role="listbox" aria-label="Recent provider chats" aria-busy={loading()}>
        <Show
          when={rows().length}
          fallback={
            <Show when={!loading()} fallback={<Skeleton rows={3} label="Finding chats" />}>
              {/* A load failure explains itself below, with Retry. */}
              <Show when={!error()}><p class="x-import-caption">No recent chats found.</p></Show>
            </Show>
          }
        >
          <Show when={filtered().length} fallback={<p class="x-import-caption">No chats match “{search().trim()}”.</p>}>
            <For each={filtered()}>
              {(row, index) => {
                const project = () => (row.projectName.trim().length > 1 ? row.projectName : null);
                return (
                  <div
                    id={optionId(index())}
                    class="x-import-row"
                    role="option"
                    aria-selected={active() === index()}
                    aria-label={`${row.title} · ${row.projectPath}`}
                    aria-disabled={importing() !== null}
                    onMouseDown={(e) => e.preventDefault()}
                    onMouseMove={() => setActive(index())}
                    onClick={() => void choose(row)}
                  >
                    <Icon name={harnessIcon(row.provider)} size={14} class="x-import-icon" label={harnessName(row.provider)} />
                    <span class="x-import-text">
                      <span class="x-import-title">{row.title}</span>
                      <Show when={project()}><span class="x-import-project">{project()}</span></Show>
                    </span>
                    <span class="x-import-meta">
                      <Show when={importing() === row.id} fallback={timeAgo(row.updatedAt)}>
                        <Spinner label="Importing" />
                      </Show>
                    </span>
                  </div>
                );
              }}
            </For>
          </Show>
        </Show>
      </div>
      <Show when={error()}>
        <div class="x-import-error" role="alert">
          <span>{rows().length ? error() : `Couldn't load chats. ${error()}`}</span>
          <Show when={!rows().length}>
            <button type="button" class="x-ghost" onClick={() => void load()}>Retry</button>
          </Show>
        </div>
      </Show>
    </div>
  );

  return (
    <div
      class={anchor ? "x-import-layer" : "x-scrim"}
      onPointerDown={(e) => e.target === e.currentTarget && closeOverlay()}
    >
      {menu}
    </div>
  );
}
