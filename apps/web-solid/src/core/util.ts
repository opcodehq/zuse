// Small pure helpers shared across areas.
import { RpcError } from "./rpc";
import { state } from "./store";

/** RFC 4122 v4 id. crypto.randomUUID is missing on plain-http origins. */
export function uuid(): string {
  if (typeof crypto.randomUUID === "function" && window.isSecureContext) return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = (b[6]! & 0x0f) | 0x40;
  b[8] = (b[8]! & 0x3f) | 0x80;
  const h = Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

/** The message of a failed request or thrown value. */
export function errorText(error: unknown, fallback = "Something went wrong."): string {
  if (error instanceof RpcError && error.message) return error.message;
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return fallback;
}

/** Compact relative time ("now", "5m", "3h", "2d", "1w", "4mo", "2y"), as desktop's format_time_ago. */
export function timeAgo(then: number, at = Date.now()) {
  const s = Math.max(0, Math.floor((at - then) / 1000));
  if (s < 60) return "now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  if (d < 7) return `${d}d`;
  if (d < 35) return `${Math.floor(d / 7)}w`;
  if (d < 365) return `${Math.floor(d / 30)}mo`;
  return `${Math.floor(d / 365)}y`;
}

/** "j•••@example.com". */
export function maskedEmail(email: string): string {
  const at = email.indexOf("@");
  return at < 0 ? "•••" : `${email[0] ?? "•"}•••${email.slice(at)}`;
}

/** Desktop's 70 second presence window; this device and devices not yet listed count as online. */
export const ONLINE_WINDOW_MS = 70_000;
export function deviceOnline(deviceId: string | null | undefined, at = Date.now()): boolean {
  if (!deviceId || deviceId === state.localDeviceId) return true;
  const device = state.devices[deviceId];
  if (!device) return true;
  const seen = device.lastSeenAt ? Date.parse(device.lastSeenAt) : NaN;
  return Number.isFinite(seen) && at - seen <= ONLINE_WINDOW_MS;
}
