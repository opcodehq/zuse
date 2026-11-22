import { createSignal, createStore, reconcile } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { RpcError } from "@/core/rpc";
import { state, setState } from "@/core/store";
import { uuid, errorText } from "@/core/util";
import { openChat, view } from "@/core/view";
import { zapperForChat } from "@/core/zappers";
import { eventsFromWire, wireMethod, wireParams } from "@/core/zapper-legacy";
import type {
  Zapper,
  ZapperAction,
  ZapperCommand,
  ZapperFile,
  ZapperEvent,
} from "@/core/zapper-types";

export const selectedZapper = () => {
  const route = view.route;
  return route.kind === "zappers"
    ? state.zappers.find((z) => z.id === route.zapperId)
    : zapperForChat(view.selectedChatId);
};
export const [commandState, setCommandState] = createStore({
  busy: false,
  error: null as string | null,
  uncertain: false,
});
let pending = false;
let retry: { command: ZapperCommand; targetDeviceId?: string } | null = null;

function accept(zapper: Zapper | null, id: string) {
  setState((draft) => {
    const index = draft.zappers.findIndex((z) => z.id === id);
    if (!zapper) {
      if (index >= 0) draft.zappers.splice(index, 1);
    } else if (index < 0) draft.zappers.push(zapper);
    else if (
      draft.zappers[index]!.revision < zapper.revision ||
      (draft.zappers[index]!.revision === zapper.revision &&
        draft.zappers[index]!.activityRevision <= zapper.activityRevision)
    )
      reconcile(zapper)(draft.zappers[index]!);
  });
}
async function execute(request: NonNullable<typeof retry>) {
  if (pending) throw new Error("Wait for the current Zapper action to finish.");
  pending = true;
  setCommandState((draft) => {
    draft.busy = true;
    draft.error = null;
    draft.uncertain = false;
  });
  try {
    const result = await rpc.call<Zapper | null>(
      wireMethod(M.CONTROL_ZAPPER_COMMAND),
      wireParams({ ...request.command, targetDeviceId: request.targetDeviceId }),
    );
    accept(result, request.command.zapperId);
    retry = null;
    return result;
  } catch (error) {
    const uncertain = error instanceof RpcError && error.outcomeUnknown;
    retry = uncertain ? request : null;
    setCommandState((draft) => {
      draft.error = uncertain
        ? "The connection dropped before confirmation. Check the refreshed Zapper, or retry this same action safely."
        : errorText(error);
      draft.uncertain = uncertain;
    });
    throw error;
  } finally {
    pending = false;
    setCommandState((draft) => {
      draft.busy = false;
    });
  }
}
export function command(id: string, action: ZapperAction, home?: string) {
  const zapper = state.zappers.find((z) => z.id === id);
  const commandId = uuid();
  if (action.type === "delegate")
    action = {
      ...action,
      worker: { ...action.worker, chatId: `zapper-worker-${commandId}` },
    };
  return execute({
    command: {
      commandId,
      zapperId: id,
      expectedRevision: zapper?.revision ?? null,
      action,
    },
    targetDeviceId: home ?? zapper?.config.homeDeviceId,
  });
}
export async function retryCommand() {
  const request = retry;
  if (!request) return null;
  const result = await execute(request);
  if (
    result &&
    ["create", "configure", "freshConversation"].includes(
      request.command.action.type,
    )
  ) {
    await Promise.resolve();
    openZapper(result);
  }
  return result;
}
export const dismissError = () =>
  setCommandState((draft) => {
    draft.error = null;
  });
export function act(id: string, action: ZapperAction) {
  void command(id, action).catch(() => {});
}
export function openZapper(zapper: Zapper) {
  openChat(zapper.coordinatorChatId);
}

export type Tab = "overview" | "issues" | "threads" | "drive" | "history";
export const [tab, setTab] = createSignal<Tab>("overview");
export const [inspectorOpen, setInspectorOpen] = createSignal(true);
export const [resources, setResources] = createStore({
  id: "",
  files: [] as ZapperFile[],
  events: [] as ZapperEvent[],
  loading: false,
  error: null as string | null,
});
let requestNumber = 0;
let displayedZapperId = "";
let refreshTask: Promise<void> | null = null;
let queuedRefresh: Zapper | null = null;
/** Collapse bursts of watch updates into one in-flight read and one trailing refresh. */
export function loadResources(zapper: Zapper): Promise<void> {
  queuedRefresh = zapper;
  displayedZapperId = zapper.id;
  setResources((draft) => {
    if (draft.id !== zapper.id) {
      draft.id = zapper.id;
      draft.files = [];
      draft.events = [];
    }
  });
  if (!refreshTask) {
    refreshTask = (async () => {
      while (queuedRefresh) {
        const next = queuedRefresh;
        queuedRefresh = null;
        await fetchResources(next);
      }
    })().finally(() => {
      refreshTask = null;
      if (queuedRefresh) void loadResources(queuedRefresh);
    });
  }
  return refreshTask;
}
async function fetchResources(zapper: Zapper) {
  const mine = ++requestNumber;
  const id = zapper.id;
  const target = { zapperId: id, targetDeviceId: zapper.config.homeDeviceId };
  setResources((draft) => {
    if (draft.id !== id) {
      draft.files = [];
      draft.events = [];
    }
    draft.id = id;
    draft.loading = true;
    draft.error = null;
  });
  try {
    const [files, events] = await Promise.all([
      rpc.call<ZapperFile[]>(wireMethod(M.CONTROL_ZAPPER_FILES), wireParams({ ...target, history: true })),
      rpc.call<ZapperEvent[]>(wireMethod(M.CONTROL_ZAPPER_EVENTS), wireParams(target)).then(eventsFromWire),
    ]);
    if (mine !== requestNumber || displayedZapperId !== id) return;
    setResources((draft) => {
      reconcile(files, (file) => `${file.path}:${file.version}`)(draft.files);
      reconcile(events, "id")(draft.events);
    });
  } catch (error) {
    if (mine === requestNumber && displayedZapperId === id)
      setResources((draft) => {
        draft.error = errorText(error);
      });
  } finally {
    if (mine === requestNumber && displayedZapperId === id)
      setResources((draft) => {
        draft.loading = false;
      });
  }
}
export const statusLabel = (value: string | undefined) =>
  (
    ({
      active: "Active",
      paused: "Paused",
      stopped: "Stopped",
      archived: "Archived",
      working: "Working",
      awaitingInput: "Needs you",
      errored: "Failed",
    }) as Record<string, string>
  )[value ?? ""] ?? "Idle";
export const threadTitle = (id: string, fallback = "Conversation") => {
  const title = state.chats[id]?.title?.trim();
  return title && !/^\[Message from (?:Zapper|Zapper) /.test(title) ? title : fallback;
};
export const workerVisible = (id: string) => !state.chats[id]?.archived;
export const threadStarted = (id: string) =>
  !!(state.chats[id]?.lastMessageAt || state.sessions[id]?.startedAt);
export const threadActive = (id: string) =>
  ["working", "awaitingInput", "errored"].includes(
    state.sessions[id]?.status ?? "",
  ) ||
  !threadStarted(id) ||
  view.selectedChatId === id;
