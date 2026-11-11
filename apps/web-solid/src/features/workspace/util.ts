// Small helpers shared by workspace surfaces.
import { toast } from "@/ui/primitives";
import { writeClipboard } from "@/ui/clipboard";

export async function copyText(text: string, done: string) {
  if (await writeClipboard(text)) toast(done);
  else toast("Couldn't copy to the clipboard.", "danger");
}

export function basename(path: string) {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.slice(Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1) || trimmed;
}

export function dirname(path: string) {
  const index = path.lastIndexOf("/");
  return index > 0 ? path.slice(0, index) : "";
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function encodeBase64(text: string) {
  const bytes = encoder.encode(text);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

export function decodeBase64Bytes(data: string) {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function decodeBase64Text(data: string) {
  return decoder.decode(decodeBase64Bytes(data));
}

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

/** "3 minutes ago" style text for a timestamp. */
export function timeAgo(value: string | number | null | undefined) {
  if (value === null || value === undefined || value === "") return "";
  const time = typeof value === "number" ? value : Date.parse(value);
  if (!Number.isFinite(time)) return "";
  const seconds = Math.round((time - Date.now()) / 1000);
  const abs = Math.abs(seconds);
  if (abs < 45) return "just now";
  if (abs < 3600) return relative.format(Math.round(seconds / 60), "minute");
  if (abs < 86400) return relative.format(Math.round(seconds / 3600), "hour");
  if (abs < 86400 * 30) return relative.format(Math.round(seconds / 86400), "day");
  if (abs < 86400 * 365) return relative.format(Math.round(seconds / (86400 * 30)), "month");
  return relative.format(Math.round(seconds / (86400 * 365)), "year");
}

export function plural(count: number, one: string, many = `${one}s`) {
  return `${count} ${count === 1 ? one : many}`;
}

/** Arrow-key movement between the focusable rows inside a container. */
export function rovingKeys(event: KeyboardEvent, selector: string) {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
  const container = event.currentTarget as HTMLElement;
  const rows = [...container.querySelectorAll<HTMLElement>(selector)].filter((row) => !row.hasAttribute("disabled"));
  if (!rows.length) return;
  event.preventDefault();
  const index = rows.indexOf(document.activeElement as HTMLElement);
  const next =
    event.key === "Home" ? 0 : event.key === "End" ? rows.length - 1 : event.key === "ArrowDown" ? Math.min(rows.length - 1, index + 1) : Math.max(0, index - 1);
  rows[next]?.focus();
}
