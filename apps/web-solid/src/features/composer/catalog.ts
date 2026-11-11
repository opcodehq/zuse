// Agent and model catalogs per execution device, and the run config a send would carry.
import { batch, createRoot, createEffect, on, untrack } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { refreshHarnesses, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Chat, Harness, Space } from "@/core/protocol";
import { state } from "@/core/store";
import { setView, view } from "@/core/view";

export interface ModelOptionChoice { id: string; label: string }
export interface ModelOption { id: string; label: string; choices: ModelOptionChoice[]; defaultChoice: string }
export interface Model { id: string; label: string; description?: string | null; reasoningLevels?: string[]; options?: ModelOption[] }
export interface HarnessInfo extends Harness {
  reasoningLevels?: string[];
  supportsSteering?: boolean;
  steeringMode?: string;
}

export type Loadable<T> = { status: "loading" } | { status: "ready"; value: T } | { status: "error"; message: string };

export type Sandbox = "workspace-write" | "auto" | "danger-full-access";
export const SANDBOXES: Sandbox[] = ["workspace-write", "auto", "danger-full-access"];

/** Composer memory for new chats, persisted with the web preferences. */
export interface ComposerDefaults {
  harness?: string;
  models: Record<string, { id: string; label: string }>;
  reasoning: Record<string, string>;
  options: Record<string, Record<string, string>>;
  favorites: string[];
  sandbox?: Sandbox;
}
const emptyDefaults: ComposerDefaults = { models: {}, reasoning: {}, options: {}, favorites: [] };

export function defaults(): ComposerDefaults {
  const raw = view.preferences.composerDefaults as Partial<ComposerDefaults> | undefined;
  return { ...emptyDefaults, ...(raw ?? {}) };
}
export function updateDefaults(change: (draft: ComposerDefaults) => void) {
  const next = JSON.parse(JSON.stringify(defaults())) as ComposerDefaults;
  change(next);
  setView("preferences", "composerDefaults", next);
}

/** What the new-chat canvas has picked so far; existing chats read their saved config instead. */
export interface DraftConfig {
  harness?: string;
  model?: string;
  reasoning?: string;
  sandbox?: Sandbox;
  plan: boolean;
  /** Host for a project-less chat. */
  deviceId?: string;
  /** The user chose to work without a project. */
  noProject: boolean;
}

interface CatalogState {
  /** Harness lists for devices other than the connected engine. */
  harnesses: Record<string, Loadable<HarnessInfo[]>>;
  models: Record<string, Loadable<Model[]>>;
  draft: DraftConfig;
  saving: Record<string, boolean>;
}

const [catalog, setCatalog] = createStore<CatalogState>({ harnesses: {}, models: {}, draft: { plan: false, noProject: false }, saving: {} });
export { catalog, setCatalog };

const isLocal = (deviceId: string | null | undefined) => !deviceId || deviceId === state.localDeviceId;
const targetParams = (deviceId: string | null | undefined) => (isLocal(deviceId) ? {} : { targetDeviceId: deviceId });
const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

// Loads start outside the reading computation, once per key until the next connection.
const requested = new Set<string>();
function load<T>(domain: "harnesses" | "models", key: string, method: string, params: Record<string, unknown>, force = false) {
  if (state.connection.kind !== "connected" || (requested.has(`${domain}:${key}`) && !force)) return;
  requested.add(`${domain}:${key}`);
  // Revalidation keeps loaded rows on screen until the fresh list lands.
  const stale = untrack(() => catalog[domain][key]?.status === "ready");
  queueMicrotask(() => {
    if (!stale) setCatalog(domain, key, { status: "loading" });
    rpc.call<T[]>(method, params).then(
      (rows) =>
        // Unchanged rows keep their identity so open lists and menus stay put.
        catalog[domain][key]?.status === "ready"
          ? setCatalog(domain, key, "value" as never, reconcile(rows as never, { key: "id" }) as never)
          : setCatalog(domain, key, { status: "ready", value: rows as never }),
      (error) => setCatalog(domain, key, { status: "error", message: errorText(error) }),
    );
  });
}
// One effect, request memory first: readers re-run as soon as the lists clear and must be able to load again.
createRoot(() => {
  createEffect(on(() => state.epoch, () => {
    requested.clear();
    batch(() => {
      setCatalog("harnesses", reconcile<Record<string, Loadable<HarnessInfo[]>>, unknown>({}));
      setCatalog("models", reconcile<Record<string, Loadable<Model[]>>, unknown>({}));
    });
  }, { defer: true }));
});

export function harnessList(deviceId: string | null | undefined, force = false): Loadable<HarnessInfo[]> {
  if (isLocal(deviceId)) {
    if (force) void refreshHarnesses();
    return state.harnesses.length ? { status: "ready", value: state.harnesses as HarnessInfo[] } : { status: "loading" };
  }
  load<HarnessInfo>("harnesses", deviceId!, M.LIST_HARNESSES, targetParams(deviceId), force);
  return catalog.harnesses[deviceId!] ?? { status: "loading" };
}

export function modelList(deviceId: string | null | undefined, harness: string, force = false): Loadable<Model[]> {
  const key = `${isLocal(deviceId) ? "local" : deviceId}|${harness}`;
  load<Model>("models", key, M.LIST_MODELS, { harness, force, ...targetParams(deviceId) }, force);
  return catalog.models[key] ?? { status: "loading" };
}

/** Agents this device offers: installed and enabled. The mock agent only shows when nothing else exists. */
export function offeredHarnesses(list: HarnessInfo[]): HarnessInfo[] {
  const real = list.filter((h) => h.id !== "mock");
  const visible = real.length ? real : list;
  return visible.filter((h) => h.installed !== false && h.enabled !== false);
}

export const REASONING_LABELS: Record<string, string> = {
  minimal: "Minimal", low: "Low", medium: "Medium", high: "High", xhigh: "X-High",
  max: "Max", ultra: "Ultra", ultracode: "Ultracode", ultrathink: "Ultrathink",
};
export const reasoningLabel = (level: string) => REASONING_LABELS[level] ?? level;

export function defaultReasoning(ladder: string[]): string | undefined {
  if (ladder.includes("high")) return "high";
  if (ladder.includes("medium")) return "medium";
  return ladder[0];
}
export function clampReasoning(level: string | null | undefined, ladder: string[]) {
  return level && ladder.includes(level) ? level : defaultReasoning(ladder);
}

/** The option choice that means "fast", and its opposite. */
export function fastModeValues(option: ModelOption): { on: string; off: string } | null {
  const has = (id: string) => option.choices.some((c) => c.id === id);
  let on: string;
  if ((option.id === "fastMode" || option.id === "fast_mode") && has("on")) on = "on";
  else if (option.id === "fast" && has("true")) on = "true";
  else if (has("fast")) on = "fast";
  else return null;
  const off = option.defaultChoice !== on && has(option.defaultChoice) ? option.defaultChoice : option.choices.find((c) => c.id !== on)?.id;
  return off ? { on, off } : null;
}
export function effortOption(model: Model): ModelOption | undefined {
  return model.options?.find((o) => ["effort", "reasoning", "reasoning_effort"].includes(o.id) && o.choices.length > 1);
}
export function offeredOptions(model: Model | undefined, picks: Record<string, unknown>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [id, value] of Object.entries(picks)) {
    if (typeof value !== "string") continue;
    if (!model || model.options?.some((o) => o.id === id && o.choices.some((c) => c.id === value))) out[id] = value;
  }
  return out;
}

/** "Approve for me" needs the agent's own automatic approver. */
export function automaticReviewSupported(harness: string | undefined) {
  return !harness || harness === "claude-code" || harness === "codex" || harness === "mock";
}
export function effectiveSandbox(level: Sandbox, harness: string | undefined): Sandbox {
  return level === "auto" && !automaticReviewSupported(harness) ? "workspace-write" : level;
}
export function planAvailable(harness: string | undefined) {
  return harness !== "pi" && harness !== "muse";
}

export const selectedChat = (): Chat | undefined => (view.selectedChatId ? state.chats[view.selectedChatId] : undefined);

export function spaceName(space: Space) {
  return space.name?.trim() || space.path.replace(/\/+$/, "").split("/").pop() || space.path;
}

/** The project a new chat starts in: the sidebar's pick, unless the user opted out. */
export function draftSpace(): Space | undefined {
  if (catalog.draft.noProject || !view.draftProjectPath) return undefined;
  const matches = Object.values(state.spaces).filter((s) => s.path === view.draftProjectPath);
  return matches.find((s) => s.deviceId === catalog.draft.deviceId) ?? matches.find((s) => s.deviceId === state.localDeviceId) ?? matches[0];
}

/** The device the next run executes on. */
export function targetDeviceId(): string | null {
  const chat = selectedChat();
  if (chat) return chat.deviceId;
  return draftSpace()?.deviceId ?? catalog.draft.deviceId ?? state.localDeviceId;
}

export { deviceOnline } from "@/core/util";

export interface Resolved {
  harness?: string;
  harnessInfo?: HarnessInfo;
  model?: Model;
  modelId?: string;
  modelLabel?: string;
  reasoning?: string;
  ladder: string[];
  options: Record<string, string>;
  sandbox: Sandbox;
  plan: boolean;
  /** Nothing runnable once the catalog has loaded. */
  noAgents: boolean;
  loading: boolean;
  locked: boolean;
}

const modelKey = (harness: string, model: string | undefined) => `${harness}|${model ?? ""}`;

export function resolve(): Resolved {
  const chat = selectedChat();
  const device = targetDeviceId();
  const list = harnessList(device);
  const ready = list.status === "ready" ? list.value : undefined;
  const offered = ready ? offeredHarnesses(ready) : [];
  const memory = defaults();
  const config = chat?.config ?? null;
  let harness: string | undefined;
  if (chat) harness = config?.harness;
  else {
    const draftPick = catalog.draft.harness;
    harness = [draftPick, memory.harness].find((h) => h && (!ready || offered.some((o) => o.id === h))) ?? offered[0]?.id;
  }
  const harnessInfo = ready?.find((h) => h.id === harness);
  const models = harness ? modelList(device, harness) : undefined;
  const catalogModels = models?.status === "ready" ? models.value : undefined;
  const explicit = chat ? config?.model ?? undefined : catalog.draft.model ?? (harness ? memory.models[harness]?.id : undefined);
  const model = catalogModels ? (explicit ? catalogModels.find((m) => m.id === explicit) : catalogModels[0]) : undefined;
  const modelId = model?.id ?? explicit;
  const modelLabel = model?.label ?? (harness && explicit ? memory.models[harness]?.label ?? explicit : undefined);
  const ladder = model?.reasoningLevels?.length ? model.reasoningLevels : model ? harnessInfo?.reasoningLevels ?? [] : [];
  const pickedReasoning = chat
    ? config?.reasoning ?? undefined
    : catalog.draft.reasoning ?? (harness ? memory.reasoning[modelKey(harness, modelId)] ?? memory.reasoning[harness] : undefined);
  const reasoning = model ? clampReasoning(pickedReasoning, ladder) : pickedReasoning;
  const options = offeredOptions(model, chat ? config?.modelOptions ?? {} : harness ? memory.options[modelKey(harness, modelId)] ?? {} : {});
  const savedSandbox = (chat ? (config?.sandbox === "read-only" ? "workspace-write" : config?.sandbox) : catalog.draft.sandbox ?? memory.sandbox) as Sandbox | undefined;
  const sandbox = effectiveSandbox(SANDBOXES.includes(savedSandbox as Sandbox) ? savedSandbox! : "workspace-write", harness);
  return {
    harness,
    harnessInfo,
    model,
    modelId,
    modelLabel,
    reasoning,
    ladder,
    options,
    sandbox,
    plan: chat ? config?.plan === true : catalog.draft.plan,
    noAgents: !chat && !!ready && offered.length === 0,
    loading: !ready || (!!harness && models?.status === "loading"),
    // Desktop lets a live chat hand off to another agent; only archived chats are fixed.
    locked: !!chat?.archived,
  };
}

/** The ChatConfig recorded on createChat and setChatConfig. */
export function chatConfig(r: Resolved) {
  if (!r.harness) return null;
  return { harness: r.harness, model: r.modelId ?? null, reasoning: r.reasoning ?? null, modelOptions: r.options, sandbox: r.sandbox, plan: r.plan };
}

/** Persist a change to the selected chat's saved config, or remember it for the next new chat. */
export async function changeConfig(change: { harness?: string; model?: string; reasoning?: string; option?: { id: string; value: string; isDefault: boolean }; sandbox?: Sandbox; plan?: boolean }): Promise<void> {
  const chat = selectedChat();
  const r = resolve();
  if (!chat) {
    const harness = change.harness ?? r.harness;
    if (change.harness !== undefined && change.harness !== r.harness) setCatalog("draft", { harness: change.harness, model: undefined, reasoning: undefined });
    if (change.harness) updateDefaults((d) => (d.harness = change.harness));
    if (change.model !== undefined && harness) {
      const models = modelList(targetDeviceId(), harness);
      const label = models.status === "ready" ? models.value.find((m) => m.id === change.model)?.label ?? change.model : change.model;
      setCatalog("draft", { model: change.model, reasoning: undefined });
      updateDefaults((d) => (d.models[harness] = { id: change.model!, label }));
    }
    if (change.reasoning !== undefined && harness) {
      setCatalog("draft", "reasoning", change.reasoning);
      updateDefaults((d) => {
        d.reasoning[modelKey(harness, r.modelId)] = change.reasoning!;
        d.reasoning[harness] = change.reasoning!;
      });
    }
    if (change.option && harness && r.modelId) {
      const { id, value, isDefault } = change.option;
      updateDefaults((d) => {
        const picks = (d.options[modelKey(harness, r.modelId)] ??= {});
        if (isDefault) delete picks[id];
        else picks[id] = value;
      });
    }
    if (change.sandbox !== undefined) {
      setCatalog("draft", "sandbox", change.sandbox);
      updateDefaults((d) => (d.sandbox = change.sandbox));
    }
    if (change.plan !== undefined) setCatalog("draft", "plan", change.plan);
    return;
  }
  const base = chatConfig(r);
  if (!base) throw new Error("Choose an agent before changing this chat's settings.");
  const next = { ...base, modelOptions: { ...base.modelOptions } };
  if (change.harness !== undefined && change.harness !== next.harness) {
    next.harness = change.harness;
    next.model = null;
    next.reasoning = null;
    next.modelOptions = {};
  }
  if (change.model !== undefined) next.model = change.model;
  if (change.reasoning !== undefined) next.reasoning = change.reasoning;
  if (change.option) {
    if (change.option.isDefault) delete next.modelOptions[change.option.id];
    else next.modelOptions[change.option.id] = change.option.value;
  }
  if (change.sandbox !== undefined) next.sandbox = change.sandbox;
  if (change.plan !== undefined) next.plan = change.plan;
  const models = modelList(chat.deviceId, next.harness);
  const target = models.status === "ready" ? models.value.find((m) => m.id === next.model) : undefined;
  if (target) {
    const info = next.harness === r.harness ? r.harnessInfo : (harnessList(chat.deviceId) as { value?: HarnessInfo[] }).value?.find((h) => h.id === next.harness);
    const ladder = target.reasoningLevels?.length ? target.reasoningLevels : info?.reasoningLevels ?? [];
    if (ladder.length) next.reasoning = clampReasoning(next.reasoning, ladder) ?? null;
    next.modelOptions = offeredOptions(target, next.modelOptions);
  }
  if (change.sandbox !== undefined) updateDefaults((d) => (d.sandbox = change.sandbox));
  setCatalog("saving", chat.id, true);
  try {
    await rpc.call(M.MUTATE, {
      op: "setChatConfig",
      chatId: chat.id,
      targetDeviceId: chat.deviceId,
      config: { ...(chat.config ?? {}), ...next },
      expectedConfig: { deviceId: chat.deviceId, config: chat.config ?? null },
    });
  } finally {
    setCatalog("saving", chat.id, false);
  }
}

/** Every picker open revalidates: the enabled agents and their models move under the cache. */
export function revalidateCatalog() {
  const device = targetDeviceId();
  const list = harnessList(device, true);
  if (list.status !== "ready") return;
  for (const h of offeredHarnesses(list.value)) modelList(device, h.id, true);
}
