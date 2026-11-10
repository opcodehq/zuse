// Devices and spaces as the projects area needs them: routing, presence, names.
import { createRoot, createSignal, onCleanup } from "solid-js";
import { state } from "@/core/store";
import type { Device, Space } from "@/core/protocol";
import { folderName } from "./paths";
import { deviceOnline as onlineAt } from "@/core/util";

/** `targetDeviceId` only for another computer; local calls skip the relay. */
export function target(deviceId: string | null | undefined, params: Record<string, unknown> = {}) {
  return deviceId && deviceId !== state.localDeviceId ? { ...params, targetDeviceId: deviceId } : params;
}

export const unknownMethod = (error: unknown) => /unknown ?method/i.test(error instanceof Error ? error.message : String(error));

export { errorText, uuid } from "@/core/util";

// Presence decays with time even when no data changes.
const now = createRoot(() => {
  const [tick, setTick] = createSignal(Date.now());
  const timer = setInterval(() => setTick(Date.now()), 15_000);
  onCleanup(() => clearInterval(timer));
  return tick;
});

export const deviceOnline = (deviceId: string): boolean => onlineAt(deviceId, now());

export function deviceLabel(deviceId: string | null | undefined): string {
  if (!deviceId) return "This computer";
  if (deviceId === state.localDeviceId) return "This computer";
  return state.devices[deviceId]?.name ?? "Connected computer";
}

export function deviceGlyph(platform: string | undefined): string {
  switch (platform) {
    case "macos":
    case "darwin":
      return "laptop";
    case "web":
      return "global";
    case "ios":
    case "android":
      return "smartphone";
    default:
      return "monitor";
  }
}

/** The local device first, then the rest by name. */
export function deviceList(): Device[] {
  const local = state.localDeviceId;
  const rows = Object.values(state.devices);
  if (local && !state.devices[local]) rows.push({ id: local, name: "This computer", platform: "" });
  return rows.sort((a, b) => (a.id === local ? -1 : b.id === local ? 1 : a.name.localeCompare(b.name)));
}

/** Computers a project can be placed on; only offered when another one is online. */
export function placementComputers(current: string | null): Device[] {
  const local = state.localDeviceId;
  const othersOnline = Object.keys(state.devices).some((id) => id !== local && deviceOnline(id));
  if (!othersOnline) return [];
  return deviceList().filter((d) => d.id === local || d.id === current || deviceOnline(d.id));
}

export const spaceName = (space: Pick<Space, "name" | "path">) =>
  space.name && space.name.trim() ? space.name : folderName(space.path);

/** Spaces of one project: a shared repository groups checkouts on several devices. */
export const projectKey = (space: Space) => (space.repositoryId ? `repo:${space.repositoryId}` : space.id);

