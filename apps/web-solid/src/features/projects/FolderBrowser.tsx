// Browse a computer's folders: devices, then locations, then folders, with Home-wide search.
import { createEffect, createMemo, For, Match, on, Show, Switch } from "solid-js";
import { state } from "@/core/store";
import type { Device } from "@/core/protocol";
import { Icon } from "@/ui/Icon";
import { Menu } from "@/ui/primitives";
import { deviceGlyph, deviceOnline } from "./hosts";
import { GroupLabel, Highlight, KeyCaps, KeyHint, Row, VirtualList } from "./Palette";
import { breadcrumbs, pathUnder, samePath } from "./paths";
import {
  acceptCompletion,
  backTo,
  browserDevices,
  browserLocations,
  cancelNewFolder,
  closeBrowser,
  closeSetup,
  completion,
  createFolder,
  descend,
  type FolderRow,
  folderRows,
  goUp,
  gotoLocation,
  loadFolders,
  openActive,
  pickDevice,
  setBrowserActive,
  setBrowserQuery,
  setNewFolderName,
  setup,
  startNewFolder,
  submitBrowser,
} from "./setup";
import { readyValue } from "./types";

type Entry =
  | { kind: "device"; device: Device; index: number }
  | { kind: "location"; name: string; path: string | null; index: number }
  | { kind: "folder"; row: FolderRow; index: number }
  | { kind: "label"; status: string | null };

const CRUMB_MAX = 3;
const CRUMB_TAIL = 2;

export function FolderBrowser() {
  let input: HTMLInputElement | undefined;
  const b = () => setup.browser!;
  const loadingFolders = () => b().step === "folders" && (b().listing.state === "loading" || b().listing.state === "idle");
  const loadError = () => (b().step === "folders" && b().listing.state === "error" ? (b().listing as { message: string }).message : null);
  const homeStatus = () => (b().homeSearch.indexing ? "Indexing…" : b().homeSearch.loading ? "Searching…" : null);

  const entries = createMemo<Entry[]>(() => {
    const browser = setup.browser;
    if (!browser) return [];
    if (browser.step === "devices") return browserDevices().map((device, index) => ({ kind: "device", device, index }));
    if (browser.step === "locations") return browserLocations().map((l, index) => ({ kind: "location", ...l, index }));
    if (loadingFolders() || loadError()) return [];
    const out: Entry[] = [];
    let homeLabel = false;
    folderRows().forEach((row, index) => {
      if (row.kind === "home" && !homeLabel) {
        out.push({ kind: "label", status: homeStatus() });
        homeLabel = true;
      }
      out.push({ kind: "folder", row, index });
    });
    return out;
  });
  const activeEntry = createMemo(() => {
    const at = entries().findIndex((e) => e.kind !== "label" && e.index === b().active);
    return at >= 0 ? at : undefined;
  });
  const placeholder = () => (b().step === "devices" ? "Search devices…" : b().step === "locations" ? "Search locations…" : "Search folders…");

  createEffect(
    on(
      () => [b().step, b().browserPath, b().newFolder === null] as const,
      () => queueMicrotask(() => !setup.browser?.newFolder && input?.focus()),
    ),
  );

  const onKeyDown = (event: KeyboardEvent) => {
    const el = event.currentTarget as HTMLInputElement;
    const caretStart = el.selectionStart === 0 && el.selectionEnd === 0;
    const caretEnd = el.selectionStart === el.value.length;
    let key = event.key;
    if (event.ctrlKey && key === "n") key = "ArrowDown";
    if (event.ctrlKey && key === "p") key = "ArrowUp";
    switch (key) {
      case "Escape":
        event.preventDefault();
        closeSetup();
        return;
      case "ArrowDown":
      case "ArrowUp": {
        event.preventDefault();
        const count = entries().filter((e) => e.kind !== "label").length;
        if (!count) return;
        const delta = key === "ArrowDown" ? 1 : -1;
        setBrowserActive((((b().active + delta) % count) + count) % count);
        return;
      }
      case "ArrowRight":
        if (caretEnd) {
          event.preventDefault();
          openActive();
        }
        return;
      case "ArrowLeft":
        if (caretStart) {
          event.preventDefault();
          goUp();
        }
        return;
      case "Tab":
        if (completion()) {
          event.preventDefault();
          acceptCompletion();
        }
        return;
      case "Enter":
        event.preventDefault();
        if (event.metaKey || event.ctrlKey) void submitBrowser();
        else openActive();
        return;
      case "Backspace":
        if (!el.value) {
          event.preventDefault();
          goUp();
        }
    }
  };

  const canAdd = () => !b().busy && b().step === "folders" && b().listing.state === "ready";
  const addLabel = () => (b().busy ? "Adding…" : b().purpose === "parent" ? "Choose folder" : "Add project");

  return (
    <section class="pj-card" aria-label={b().purpose === "parent" ? "Choose a folder" : "Open folder"}>
      <div class="pj-header">
        <span class="pj-lead">
          <Icon name="palette-search" size={16} />
        </span>
        <div class="pj-input-wrap">
          <Show when={completion()}>
            {(next) => (
              <span class="pj-ghost" aria-hidden="true">
                <span class="pj-ghost-typed">{b().query}</span>
                {next().suffix}
              </span>
            )}
          </Show>
          <input
            ref={input}
            class="pj-input"
            value={b().query}
            placeholder={placeholder()}
            aria-label={placeholder()}
            role="combobox"
            aria-expanded="true"
            aria-controls="pj-browser-list"
            aria-activedescendant={activeEntry() !== undefined ? `pj-browser-${activeEntry()}` : undefined}
            autocomplete="off"
            spellcheck={false}
            onInput={(e) => setBrowserQuery(e.currentTarget.value)}
            onKeyDown={onKeyDown}
          />
        </div>
      </div>
      <div class="pj-panel">
        <Crumbs />
        <Show when={b().newFolder}>
          {(draft) => (
            <form
              class="pj-new-folder"
              onSubmit={(e) => {
                e.preventDefault();
                void createFolder();
              }}
            >
              <Icon name="folder-add" size={16} class="pj-glyph" />
              <input
                class="pj-new-folder-input"
                ref={(el) => queueMicrotask(() => el.focus())}
                value={draft().name}
                placeholder="New folder name"
                aria-label="New folder name"
                disabled={draft().busy}
                onInput={(e) => setNewFolderName(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    cancelNewFolder();
                    queueMicrotask(() => input?.focus());
                  }
                }}
              />
              <button type="submit" class="pj-text-btn" disabled={draft().busy || !draft().name.trim()}>
                {draft().busy ? "Creating…" : "Create"}
              </button>
              <button type="button" class="pj-text-btn" onClick={() => cancelNewFolder()}>
                Cancel
              </button>
              <Show when={draft().error}>
                <span class="pj-new-folder-error" role="alert">
                  {draft().error}
                </span>
              </Show>
            </form>
          )}
        </Show>
        <VirtualList
          id="pj-browser-list"
          class="pj-list-browser"
          label={b().step === "devices" ? "Devices" : b().step === "locations" ? "Locations" : "Folders"}
          items={entries()}
          active={activeEntry()}
          estimate={(e) => (e.kind === "label" ? 30 : 32)}
          itemKey={(e, i) =>
            e.kind === "device"
              ? `d-${e.device.id}`
              : e.kind === "location"
                ? `l-${e.path ?? "home"}`
                : e.kind === "folder"
                  ? `f-${e.row.kind === "home" ? e.row.match.path : e.row.path}`
                  : `label-${i}`
          }
          footer={<ListState />}
        >
          {(entry, index) => <EntryView entry={entry} id={`pj-browser-${index}`} />}
        </VirtualList>
        <Show when={b().error}>
          <div class="pj-error" role="alert">
            <Icon name="danger-triangle" size={13} />
            <span>{b().error}</span>
          </div>
        </Show>
      </div>
      <footer class="pj-footer">
        <span class="pj-hints">
          <KeyHint keys={["up", "down"]} label="Navigate" />
          <KeyHint keys={["enter"]} label={b().step === "folders" ? "Open" : "Select"} />
          <Show when={b().step !== "devices"}>
            <KeyHint keys={["left"]} label="Back" />
          </Show>
          <KeyHint keys={["Esc"]} label="Close" />
        </span>
        <Show when={b().step === "folders"}>
          <button type="button" class="pj-submit pj-submit-add" disabled={!canAdd()} aria-label={addLabel()} onClick={() => void submitBrowser()}>
            <KeyCaps keys={["mod", "enter"]} />
            {addLabel()}
          </button>
        </Show>
      </footer>
    </section>
  );
}

function ListState() {
  const b = () => setup.browser!;
  const query = () => b().query;
  const count = () =>
    b().step === "devices" ? browserDevices().length : b().step === "locations" ? browserLocations().length : folderRows().length;
  const empty = (): [string, string] | null => {
    if (count() > 0) return null;
    if (b().step === "devices") return ["No devices found", "Try another device name."];
    if (b().step === "locations") return b().drives.state === "loading" ? null : ["No locations found", "Try Home or a drive name."];
    if (b().listing.state !== "ready") return null;
    if (!query()) return ["No folders here", "Add this folder with the button below, or go back with ←."];
    if (b().homeSearch.loading) return ["Searching Home…", "Looking for matching folders anywhere under Home."];
    if (b().homeSearch.indexing) return ["No folders match yet", "Home is still being indexed — keep typing or try again."];
    return ["No folders match", "Type a path like ~/code or /mnt to jump there."];
  };
  return (
    <Switch>
      <Match when={b().step === "folders" && (b().listing.state === "loading" || b().listing.state === "idle")}>
        <div class="pj-skeletons" aria-label="Loading folders" role="status">
          <For each={[0, 1, 2, 3, 4]}>
            {() => (
              <div class="pj-skeleton">
                <span />
                <span />
              </div>
            )}
          </For>
        </div>
      </Match>
      <Match when={b().step === "folders" && b().listing.state === "error" && (b().listing as { message: string }).message}>
        {(message) => (
          <div class="pj-error pj-error-block" role="alert">
            <Icon name="danger-triangle" size={13} />
            <span>{message()}</span>
            <button type="button" class="pj-text-btn" onClick={() => void loadFolders(b().browserPath)}>
              Retry
            </button>
          </div>
        )}
      </Match>
      <Match when={empty()}>
        {(text) => (
          <div class="pj-empty">
            <span>{text()[0]}</span>
            <span class="pj-empty-hint">{text()[1]}</span>
          </div>
        )}
      </Match>
      <Match when={b().step === "locations" && b().drives.state === "loading"}>
        <div class="pj-note pj-note-left">Loading locations…</div>
      </Match>
    </Switch>
  );
}

function EntryView(props: { entry: Entry; id: string }) {
  const b = () => setup.browser!;
  const highlighted = (index: number) => b().active === index;
  const hover = (index: number) => () => b().active !== index && setBrowserActive(index);
  return (
    <Switch>
      <Match when={props.entry.kind === "label" && props.entry}>{(e) => <GroupLabel status={e().status}>In Home</GroupLabel>}</Match>
      <Match when={props.entry.kind === "device" && props.entry}>
        {(e) => (
          <Row
            id={props.id}
            glyph={deviceGlyph(e().device.platform)}
            highlighted={highlighted(e().index)}
            onPointerMove={hover(e().index)}
            onClick={() => pickDevice(e().device.id)}
            aria-label={`${e().device.name}${deviceOnline(e().device.id) ? "" : ", offline"}`}
          >
            <span class="pj-title pj-grow">
              <Highlight text={e().device.id === state.localDeviceId && !state.devices[e().device.id] ? "This computer" : e().device.name} query={b().query} />
            </span>
            <span class="pj-dot" data-online={deviceOnline(e().device.id) ? "true" : undefined} />
          </Row>
        )}
      </Match>
      <Match when={props.entry.kind === "location" && props.entry}>
        {(e) => (
          <Row
            id={props.id}
            glyph={e().path === null ? "home" : "hard-drive"}
            highlighted={highlighted(e().index)}
            onPointerMove={hover(e().index)}
            onClick={() => gotoLocation(e().name, e().path)}
          >
            <span class="pj-title pj-grow">
              <Highlight text={e().name} query={b().query} />
            </span>
          </Row>
        )}
      </Match>
      <Match when={props.entry.kind === "folder" && props.entry}>
        {(e) => {
          const row = () => e().row;
          return (
            <Switch>
              <Match when={row().kind === "local" && (row() as Extract<FolderRow, { kind: "local" }>)}>
                {(local) => (
                  <Row
                    id={props.id}
                    glyph="folder"
                    highlighted={highlighted(e().index)}
                    onPointerMove={hover(e().index)}
                    onClick={() => descend(local().path, local().entry.isRepo)}
                    aria-label={`${local().entry.name}${local().entry.isRepo ? ", Git repository" : ""}`}
                  >
                    <span class="pj-title pj-grow">
                      <Highlight text={local().entry.name} query={b().query} />
                    </span>
                    <Show when={local().entry.isRepo}>
                      <Icon name="git-branch" size={14} class="pj-glyph" />
                    </Show>
                  </Row>
                )}
              </Match>
              <Match when={row().kind === "home" && (row() as Extract<FolderRow, { kind: "home" }>).match}>
                {(match) => {
                  const split = () => {
                    const at = match().relative.lastIndexOf("/");
                    return at < 0
                      ? { name: match().relative, parent: "~" }
                      : { name: match().relative.slice(at + 1), parent: `~/${match().relative.slice(0, at)}` };
                  };
                  return (
                    <Row
                      id={props.id}
                      glyph="folder"
                      highlighted={highlighted(e().index)}
                      onPointerMove={hover(e().index)}
                      onClick={() => descend(match().path, match().isRepo)}
                      aria-label={`${split().name} in ${split().parent}`}
                    >
                      <span class="pj-title pj-title-fixed">
                        <Highlight text={split().name} query={b().query} />
                      </span>
                      <span class="pj-detail">{split().parent}</span>
                      <Show when={match().isRepo}>
                        <Icon name="git-state-branch" size={12} class="pj-glyph" />
                      </Show>
                    </Row>
                  );
                }}
              </Match>
            </Switch>
          );
        }}
      </Match>
    </Switch>
  );
}

interface Crumb {
  name: string;
  glyph?: string;
  current: boolean;
  go?: () => void;
  hidden?: [string, string][];
}

function Crumbs() {
  let strip: HTMLDivElement | undefined;
  const b = () => setup.browser!;
  const crumbs = createMemo<Crumb[]>(() => {
    const browser = b();
    const out: Crumb[] = [{ name: "New project", current: browser.step === "devices", go: () => backTo("devices") }];
    if (browser.deviceId) {
      const device = state.devices[browser.deviceId];
      out.push({
        name: device?.name ?? "This computer",
        glyph: deviceGlyph(device?.platform),
        current: browser.step === "locations",
        go: () => backTo("locations"),
      });
    }
    const location = browser.location;
    if (location) {
      const root = location.path ?? browser.home;
      // While a descend loads, keep the requested path so the trail does not jump back.
      const open = readyValue(browser.listing)?.path ?? browser.browserPath ?? root;
      const atRoot = !open || (!!root && samePath(open, root));
      out.push({
        name: location.name,
        glyph: location.path === null ? "home" : "hard-drive",
        current: atRoot,
        go: () => gotoLocation(location.name, location.path),
      });
      if (open) {
        const folders = breadcrumbs(open).filter(([, full]) => !(root && pathUnder(root, full)));
        const hidden = folders.length > CRUMB_MAX ? folders.splice(0, folders.length - CRUMB_TAIL) : [];
        if (hidden.length) out.push({ name: "…", current: false, hidden });
        for (const [name, full] of folders) out.push({ name, current: samePath(full, open), go: () => descend(full, false) });
      }
    }
    return out;
  });
  createEffect(
    on(
      () => crumbs().map((c) => c.name).join("/"),
      () => queueMicrotask(() => strip && (strip.scrollLeft = strip.scrollWidth)),
    ),
  );
  const backLabel = () => (b().step === "devices" ? (b().purpose === "parent" ? "Back to project" : "Back to New project") : "Back");
  return (
    <div class="pj-crumbs">
      <button
        type="button"
        class="pj-crumb-back"
        aria-label={backLabel()}
        title={backLabel()}
        onClick={() => (b().step === "devices" ? closeBrowser() : goUp())}
      >
        <Icon name="arrow-left" size={16} />
      </button>
      <nav class="pj-crumb-trail" ref={strip} aria-label="Folder path">
        <For each={crumbs()}>
          {(crumb, index) => (
            <>
              <Show when={index() > 0}>
                <Icon name="alt-arrow-right" size={12} class="pj-crumb-sep" />
              </Show>
              <Switch>
                <Match when={crumb.hidden}>
                  {(hidden) => (
                    <Menu
                      items={hidden().map(([name, full]) => ({ label: name, icon: "folder", onSelect: () => descend(full, false) }))}
                      trigger={(triggerProps) => (
                        <button type="button" {...triggerProps} class="pj-crumb pj-crumb-more" aria-label="Show hidden folders">
                          …
                        </button>
                      )}
                    />
                  )}
                </Match>
                <Match when={crumb.current}>
                  <span class="pj-crumb" data-current="true" aria-current="location" title={crumb.name.length > 26 ? crumb.name : undefined}>
                    <Show when={crumb.glyph}>{(g) => <Icon name={g()} size={14} />}</Show>
                    <span class="pj-crumb-name">{crumb.name}</span>
                  </span>
                </Match>
                <Match when={!crumb.current}>
                  <button type="button" class="pj-crumb" onClick={() => crumb.go?.()} title={crumb.name.length > 26 ? crumb.name : undefined}>
                    <Show when={crumb.glyph}>{(g) => <Icon name={g()} size={14} />}</Show>
                    <span class="pj-crumb-name">{crumb.name}</span>
                  </button>
                </Match>
              </Switch>
            </>
          )}
        </For>
      </nav>
      <Show when={b().step === "folders" && b().listing.state === "ready" && !b().newFolder}>
        <button type="button" class="pj-crumb-back" aria-label="New folder" title="New folder" onClick={() => startNewFolder()}>
          <Icon name="folder-add" size={16} />
        </button>
      </Show>
    </div>
  );
}

