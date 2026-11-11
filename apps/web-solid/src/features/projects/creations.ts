// Host-owned project creations, watched per computer like desktop's AppState::watch_project_creations.
import { createEffect, createRoot, on } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { onConnected, rpc } from "@/core/connection";
import { M } from "@/core/methods";
import type { Subscription } from "@/core/rpc";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { target, unknownMethod } from "./hosts";
import { samePath } from "./paths";
import type { ProjectCreation } from "./types";

const LOCAL = "local";

/** Operations by watched computer ("local" for the engine's own). */
const [operations, setOperations] = createStore<Record<string, ProjectCreation[]>>({});

const targets = new Set<string>([LOCAL]);
const live = new Map<string, Subscription>();
const unsupported = new Set<string>();

const keyFor = (deviceId: string | null | undefined) => (!deviceId || deviceId === state.localDeviceId ? LOCAL : deviceId);

function open(key: string) {
  if (live.has(key) || unsupported.has(key) || !rpc.connected) return;
  const params = key === LOCAL ? {} : target(key);
  const sub = rpc.subscribe<ProjectCreation[]>(
    M.PROJECT_WATCH,
    params,
    (rows) => setOperations(key, reconcile(Array.isArray(rows) ? rows : [], { key: "id" })),
    (error) => {
      if (live.get(key) !== sub) return;
      live.delete(key);
      if (error && unknownMethod(error)) {
        unsupported.add(key);
        return;
      }
      // A relay hiccup ends remote streams; desktop retries every two seconds.
      setTimeout(() => open(key), 2000);
    },
  );
  live.set(key, sub);
}

onConnected(() => {
  unsupported.clear();
  for (const key of targets) open(key);
  return {
    cancel() {
      const subs = [...live.values()];
      live.clear();
      for (const sub of subs) sub.cancel();
    },
  };
});

/** Start watching a computer's creations for the rest of the session. */
export function watchCreations(deviceId: string | null | undefined) {
  const key = keyFor(deviceId);
  targets.add(key);
  open(key);
}

// Desktop watches the host of whatever is selected.
createRoot(() => {
  createEffect(
    on(
      () => {
        const chat = view.selectedChatId ? state.chats[view.selectedChatId] : undefined;
        return chat?.deviceId ?? draftSpace()?.deviceId;
      },
      (deviceId) => deviceId && watchCreations(deviceId),
    ),
  );
});

function draftSpace() {
  const path = view.draftProjectPath;
  return path ? Object.values(state.spaces).find((space) => samePath(space.path, path)) : undefined;
}

export function allCreations(): ProjectCreation[] {
  return Object.values(operations).flat();
}

/** The creation that produced a folder on a computer, if any. */
export function creationAt(deviceId: string, path: string): ProjectCreation | undefined {
  const local = state.localDeviceId;
  return allCreations().find(
    (op) => (op.deviceId === deviceId || (op.deviceId === local && deviceId === local)) && samePath(op.destination, path),
  );
}

export function creationForSpace(spaceId: string): ProjectCreation | undefined {
  const space = state.spaces[spaceId];
  return space ? creationAt(space.deviceId, space.path) : undefined;
}

/** Files are not ready yet: agents cannot run in this project. */
export const creationBlocks = (op: ProjectCreation | undefined) => !!op && !op.filesReady;

export const canRetry = (op: ProjectCreation) =>
  op.status === "failed" || op.status === "cancelled" || op.status === "interrupted" || !!op.publishError;

export function cancelCreation(op: ProjectCreation) {
  return rpc.call(M.PROJECT_CANCEL, target(op.deviceId, { id: op.id }));
}

export function retryCreation(op: ProjectCreation) {
  return rpc.call<ProjectCreation>(M.PROJECT_RETRY, target(op.deviceId, { id: op.id }));
}

/** Merge a just-started operation before the watch reports it. */
export function noteCreation(op: ProjectCreation) {
  const key = keyFor(op.deviceId);
  watchCreations(op.deviceId);
  if (!(operations[key] ?? []).some((existing) => existing.id === op.id))
    setOperations(key, (rows) => [...(rows ?? []), op]);
}

/** Status line for a creation, as desktop's composer tray words it. */
export function describeCreation(op: ProjectCreation): { tone: "default" | "warning" | "danger"; title: string; detail: string } | null {
  const name = op.request.name;
  switch (op.status) {
    case "running":
      return { tone: "default", title: `Setting up ${name}`, detail: `${op.stage}${op.percent != null ? ` ${op.percent}%` : ""}` };
    case "cancelled":
      return { tone: "default", title: `Setup of ${name} was cancelled`, detail: op.error ?? "" };
    case "interrupted":
      return { tone: "warning", title: `Setup of ${name} was interrupted`, detail: op.error ?? "" };
    case "failed":
      return { tone: "danger", title: `Couldn't create ${name}`, detail: op.error ?? "" };
    case "ready":
      if (op.publishError) return { tone: "warning", title: "Not published to GitHub", detail: op.publishError };
      if (op.warning) return { tone: "warning", title: `${name} is ready`, detail: op.warning };
      return null;
  }
}
