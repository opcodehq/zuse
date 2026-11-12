// Small pure helpers shared by the sidebar modules.
import { createSignal } from "solid-js";
import { toast } from "@/ui/primitives";
import { writeClipboard } from "@/ui/clipboard";
import { RpcError } from "@/core/rpc";

// Reads are exact; the tick only re-runs readers so ages advance while nothing else changes.
const [tick, setTick] = createSignal(0);
if (typeof window !== "undefined") setInterval(() => setTick((n) => n + 1), 15_000);
/** Wall clock for relative ages and session staleness. */
export const now = () => (tick(), Date.now());

export const time = (iso: string | null | undefined) => {
  if (!iso) return 0;
  const value = Date.parse(iso);
  return Number.isNaN(value) ? 0 : value;
};

export { timeAgo, uuid } from "@/core/util";
export { harnessIcon, harnessTint } from "@/ui/harness";

/** "+1.2K" style line counts. */
export function compactCount(count: number) {
  if (count < 1000) return String(count);
  const tenths = Math.round(count / 100) / 10;
  return Number.isInteger(tenths) ? `${tenths}K` : `${tenths.toFixed(1)}K`;
}

export function basename(path: string) {
  const trimmed = path.replace(/[/\\]+$/, "");
  const name = trimmed.split(/[/\\]/).pop();
  return name || trimmed || path;
}

/** One line for single-line surfaces: model-written titles can carry newlines. */
export const singleLine = (text: string) => text.split(/\s+/).filter(Boolean).join(" ");

export async function copyText(text: string, what: string) {
  if (await writeClipboard(text)) toast(`${what} copied`);
  else toast(`Couldn't copy ${what.toLowerCase()}`, "danger");
}

/** Report a failed request. Interrupted mutations may have landed, so they say so instead of retrying. */
export function reportError(prefix: string, error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  if (error instanceof RpcError && error.outcomeUnknown) toast(message, "danger");
  else toast(`${prefix}: ${message}`, "danger");
}

/** Two-letter initials: first letters of the first two words, else the first two letters. */
export function initials(name: string) {
  const words = name.split(/[-_.\s]+/).filter(Boolean);
  const [first, second] = words;
  const letters = first && second ? first[0]! + second[0]! : first ? [...first].slice(0, 2).join("") : "?";
  return letters.toUpperCase();
}

// Desktop's MONOGRAM_PALETTE (crates/ui project_icon.rs): [dark, light].
const MONOGRAM: [string, string][] = [
  ["#94a3b8", "#475569"],
  ["#93c5fd", "#2563eb"],
  ["#c4b5fd", "#7c3aed"],
  ["#fda4af", "#be123c"],
  ["#fcd34d", "#a16207"],
  ["#6ee7b7", "#047857"],
  ["#5eead4", "#0f766e"],
  ["#fdba74", "#c2410c"],
];

/** A stable palette pair for `seed` (FNV-1a over its UTF-8 bytes, as desktop hashes it). */
export function monogramTone(seed: string): [string, string] {
  let hash = 2166136261;
  for (const byte of new TextEncoder().encode(seed)) hash = Math.imul(hash ^ byte, 16777619) >>> 0;
  return MONOGRAM[hash % MONOGRAM.length]!;
}

function githubOwner(path: string) {
  const [owner, repository] = path.split("/");
  const valid = (part: string | undefined): part is string => !!part && /^[A-Za-z0-9_.-]+$/.test(part);
  return valid(owner) && !!repository && !owner.startsWith(".") ? owner : null;
}

/** The GitHub owner avatar for a project whose remote is on github.com. */
export function githubAvatar(remotes: (string | null | undefined)[]) {
  for (const remote of remotes) {
    const path = remote?.startsWith("github.com/") ? remote.slice("github.com/".length) : null;
    const owner = path ? githubOwner(path) : null;
    if (owner) return `https://github.com/${owner}.png?size=80`;
  }
  return null;
}
