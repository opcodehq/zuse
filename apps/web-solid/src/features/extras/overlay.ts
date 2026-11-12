// Which global overlay is open. One at a time; closing returns focus to where it came from.
import { createSignal } from "solid-js";

export type Overlay = "palette" | "shortcuts" | "import" | null;
export type PaletteTab = "all" | "threads" | "commands" | "files";

const [overlay, setOverlay] = createSignal<Overlay>(null);
const [paletteTab, setPaletteTab] = createSignal<PaletteTab>("all");
let returnFocus: HTMLElement | null = null;

export { overlay, paletteTab };

export function openOverlay(kind: Exclude<Overlay, null>) {
  if (overlay() === null) returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  setOverlay(kind);
}

/** The button that opened the current overlay, when one did (the landing's Import chat chip); menus anchor to it. */
export function overlayAnchor(): HTMLElement | null {
  const origin = returnFocus;
  return origin instanceof HTMLButtonElement && origin.isConnected && !origin.closest(".x-palette, .x-scrim") ? origin : null;
}

export function closeOverlay(restore = true) {
  if (overlay() === null) return;
  setOverlay(null);
  const target = returnFocus;
  returnFocus = null;
  if (restore) queueMicrotask(() => target?.isConnected && target.focus());
}

export const toggleOverlay = (kind: Exclude<Overlay, null>) => (overlay() === kind ? closeOverlay() : openOverlay(kind));

/** Desktop's Mod+K opens on All and Mod+P on Files; pressing the same key again closes it. */
export function togglePalette(tab: PaletteTab = "all") {
  if (overlay() === "palette" && paletteTab() === tab) return closeOverlay();
  setPaletteTab(tab);
  openOverlay("palette");
}

export const openPalette = (tab: PaletteTab = "all") => {
  setPaletteTab(tab);
  openOverlay("palette");
};
export const openShortcuts = () => openOverlay("shortcuts");
export const openImportChat = () => openOverlay("import");
