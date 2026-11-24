import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, For, onCleanup, Show } from "solid-js";
import { M } from "@/core/methods";
import { RpcClient } from "@/core/rpc";
import { Icon } from "@/ui/Icon";
import { toast, ToastRegion } from "@/ui/primitives";
import { describeLink, forget } from "../computers";
import { registerComputerAdder, type AddRequest } from "./navigation";
import { dispatchComputerAction, registerGlobalKeys, type ComputerAction, type GlobalKey } from "./bridge";
import { active, activateComputer, addRecipe, applyComputerSnapshot, computers, FRAME_PARAM, inComputerFrame, multiMode, nextThread, primaryRecipe, projectGroups, recipeKey, removeRecipe, stagePairing, type Computer, type Recipe, type ThreadTarget } from "./model";
import { computerTransport } from "./transports";
import "./multi.css";

export function MultiComputerShell(props: { children: JSX.Element }) {
  return <Show when={!inComputerFrame()} fallback={props.children}><ComputerHost>{props.children}</ComputerHost></Show>;
}
function ComputerHost(props: { children: JSX.Element }) {
  const frames = new Map<string, HTMLIFrameElement>();
  const ready = new Set<string>();
  const pending = new Map<string, ComputerAction[]>();
  const [query, setQuery] = createSignal("");
  const [collapsed, setCollapsed] = createSignal(new Set<string>());
  const [drawer, setDrawer] = createSignal(false);
  const [removing, setRemoving] = createSignal<string | null>(null);
  let search: HTMLInputElement | undefined;
  const groups = createMemo(() => projectGroups(computers(), query()));
  const order = createMemo(() => groups().filter(group => !collapsed().has(group.key)).flatMap(group => group.parts.flatMap(part => part.chats.map(chat => ({ computer: part.computer.id, chat: chat.id })))));
  const send = (computer: string, action: ComputerAction) => {
    activateComputer(computer); setDrawer(false);
    if (computer === "primary") { dispatchComputerAction(action); return; }
    const frame = frames.get(computer);
    if (frame?.contentWindow && ready.has(computer)) frame.contentWindow.postMessage({ type: "zuse.computer.action", frame: computer, action }, location.origin);
    else pending.set(computer, [...(pending.get(computer) ?? []), action]);
  };
  const pick = (target: ThreadTarget) => send(target.computer, { kind: "chat", chat: target.chat });
  const globalKey = (key: GlobalKey) => {
    if (key.kind === "search") { setDrawer(true); queueMicrotask(() => search?.focus()); return; }
    if (key.kind === "sidebar") { setDrawer(!drawer()); return; }
    if (key.kind === "jump") { const target = order()[key.slot]; if (target) pick(target); return; }
    const selected = computers().find(value => value.id === active())?.snapshot?.selected;
    const target = nextThread(order(), selected ? { computer: active(), chat: selected } : null, key.forward);
    if (target) pick(target);
  };
  // eslint-disable-next-line solid/reactivity -- this callback runs only from captured keyboard events
  registerGlobalKeys(key => globalKey(key));
  const add = (request: AddRequest, controller: Recipe) => {
    if (request.kind === "paired") {
      const described = describeLink(request.link);
      if (!described) throw new Error("Enter a valid private pairing link.");
      stagePairing(described.id, request.link);
      addRecipe({ kind: "paired", pairId: described.id }, described.origin);
    } else addRecipe({ kind: "account", target: request.target, controller }, request.target.label);
  };
  registerComputerAdder(request => add(request, primaryRecipe()));
  const receive = (event: MessageEvent) => {
    if (event.origin !== location.origin || !event.data || typeof event.data.frame !== "string") return;
    const computer = computers().find(value => value.id === event.data.frame);
    if (!computer || frames.get(computer.id)?.contentWindow !== event.source) return;
    if (event.data.type === "zuse.computer.snapshot") {
      if (event.data.snapshot?.synced) ready.add(computer.id);
      applyComputerSnapshot(computer.id, event.data.snapshot);
      const queue = ready.has(computer.id) ? pending.get(computer.id) ?? [] : [];
      if (ready.has(computer.id)) pending.delete(computer.id);
      for (const action of queue) frames.get(computer.id)?.contentWindow?.postMessage({ type: "zuse.computer.action", frame: computer.id, action }, location.origin);
    } else if (event.data.type === "zuse.computer.add") {
      try { add(event.data.request as AddRequest, computer.recipe); } catch (error) { toast(error instanceof Error ? error.message : "Could not add computer", "danger"); }
    } else if (event.data.type === "zuse.computer.return") {
      if (computer.recipe.kind === "account") {
        const controller = computer.recipe.controller;
        const existing = computers().find(value => recipeKey(value.recipe) === recipeKey(controller));
        if (existing) activateComputer(existing.id); else addRecipe(controller, "Controller");
      } else activateComputer("primary");
    } else if (event.data.type === "zuse.computer.forget") {
      if (computer.recipe.kind === "paired") forget(computer.recipe.pairId);
      removeRecipe(computer.id);
    } else if (event.data.type === "zuse.computer.key" && computer.id === active()) globalKey(event.data.key as GlobalKey);
  };
  window.addEventListener("message", receive); onCleanup(() => window.removeEventListener("message", receive));
  createEffect(() => computers().map(computer => ({ id: computer.id, recipe: computer.recipe, catalog: computer.snapshot?.catalog, dirty: computer.snapshot?.dirty })), values => {
    for (const computer of values) {
      if (computer.id === "primary" || computer.recipe.kind !== "account" || computer.dirty) continue;
      const { target, controller } = computer.recipe;
      const catalog = values.find(value => recipeKey(value.recipe) === recipeKey(controller))?.catalog;
      if (catalog?.subject === target.expectedSubject && !catalog.environments.includes(target.environmentId)) removeRecipe(computer.id);
    }
    if (!values.some(value => value.id === active())) activateComputer("primary");
  });
  const remove = async (computer: Computer) => {
    if (computer.id === "primary" || removing()) return;
    if (computer.snapshot?.dirty && !window.confirm(`Remove ${computer.label}? It has unsaved files or drafts. Save them first to keep your work.`)) return;
    setRemoving(computer.id);
    try {
      if (computer.recipe.kind === "account") {
        const transport = computerTransport(computer.recipe.controller); const client = new RpcClient();
        try {
          await transport.open(); client.attach(transport);
          await client.call(M.REMOVE_ACCOUNT_COMPUTER, { environmentId: computer.recipe.target.environmentId, expectedSubject: computer.recipe.target.expectedSubject });
        } finally { client.detach("Computer removal finished"); transport.close(); }
      } else if (computer.recipe.kind === "paired") forget(computer.recipe.pairId);
      removeRecipe(computer.id);
    } catch (error) { toast(error instanceof Error ? error.message : "Could not remove computer", "danger"); }
    finally { setRemoving(null); }
  };
  const frameUrl = (id: string) => { const url = new URL(location.href); url.hash = ""; url.searchParams.set(FRAME_PARAM, id); return url.href; };
  const selected = (computer: string, chat: string) => active() === computer && computers().find(value => value.id === computer)?.snapshot?.selected === chat;
  return <div class="mc-shell" data-multi={multiMode() ? "true" : undefined} data-drawer={drawer() ? "true" : undefined}>
    <Show when={multiMode()}>
      <button class="mc-mobile-open" aria-label="Show all computers" onClick={() => setDrawer(!drawer())}><Icon name="monitor" size={18} /></button>
      <aside class="mc-sidebar" aria-label="Projects across computers" onKeyDown={event => {
        if (event.key === "Escape") { setDrawer(false); search?.blur(); }
        else if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); globalKey({ kind: "cycle", forward: event.key === "ArrowDown" }); }
      }}>
        <div class="mc-actions">
          <button onClick={() => send(active(), { kind: "new" })}><Icon name="dither-chat" size={14} />New thread</button>
          <button onClick={() => send(active(), { kind: "newProject" })}><Icon name="dither-folder" size={14} />New project</button>
          <button onClick={() => send("primary", { kind: "settings", section: "devices" })}><Icon name="plus" size={14} />Connect computer</button>
        </div>
        <input ref={search} class="mc-search" aria-label="Search across computers" placeholder="Search all computers…" value={query()} onInput={event => setQuery(event.currentTarget.value)} />
        <div class="mc-projects"><For each={groups()}>{group => <section class="mc-project" data-project={group.key}>
          <button class="mc-project-heading" aria-expanded={collapsed().has(group.key) ? "false" : "true"} onClick={() => setCollapsed(previous => { const next = new Set(previous); if (!next.delete(group.key)) next.add(group.key); return next; })}>
            <Icon name={collapsed().has(group.key) ? "alt-arrow-right" : "alt-arrow-down"} size={12} /><span>{group.label}</span>
          </button>
          <Show when={!collapsed().has(group.key)}><For each={group.parts}>{part => <div class="mc-checkout">
            <div class="mc-computer-heading"><Icon name="monitor" size={12} /><span>{part.computer.label}</span><Show when={!part.computer.snapshot?.connected}><span class="mc-offline">Offline</span></Show>
              <button aria-label={`New thread on ${part.computer.label}`} onClick={() => send(part.computer.id, { kind: "new", project: part.space?.id })}><Icon name="plus" size={12} /></button>
            </div>
            <For each={part.chats}>{chat => <button class="mc-thread" aria-current={selected(part.computer.id, chat.id) ? "page" : undefined} data-computer={part.computer.id} data-chat={chat.id} onClick={() => pick({ computer: part.computer.id, chat: chat.id })}>
              <span class="mc-indicator" data-status={part.computer.snapshot?.sessions[chat.id]?.status ?? "idle"} /><span class="mc-thread-title">{chat.title || "New session"}</span>
            </button>}</For>
            <Show when={!part.chats.length}><div class="mc-empty">No threads yet</div></Show>
          </div>}</For></Show>
        </section>}</For><Show when={!groups().length}><div class="mc-empty">{query() ? "No matching threads" : "Connected computers will appear here."}</div></Show></div>
        <div class="mc-computers" aria-label="Connected computers"><For each={computers()}>{computer => <div class="mc-computer-row"><button class="mc-computer-select" title={computer.label} aria-pressed={active() === computer.id ? "true" : "false"} onClick={() => { activateComputer(computer.id); setDrawer(false); }}>{computer.label}</button><Show when={computer.id !== "primary"}><button aria-label={`Remove ${computer.label}`} disabled={removing() !== null} onClick={() => void remove(computer)}><Icon name="close" size={12} /></button></Show></div>}</For></div>
        <button class="mc-settings" onClick={() => send("primary", { kind: "settings" })}><Icon name="settings" size={14} />Settings</button>
      </aside>
    </Show>
    <div class="mc-outlets"><div class="mc-outlet" hidden={multiMode() && active() !== "primary"}>{props.children}</div>
      <For each={computers().filter(value => value.id !== "primary").map(computer => computer.id)}>{id => {
        onCleanup(() => { frames.delete(id); ready.delete(id); pending.delete(id); });
        return <iframe ref={element => { frames.set(id, element); }} class="mc-outlet mc-frame" title={computers().find(computer => computer.id === id)?.label ?? "Computer"} src={frameUrl(id)} hidden={active() !== id} />;
      }}</For>
    </div>
    <Show when={multiMode()}><ToastRegion /></Show>
  </div>;
}
