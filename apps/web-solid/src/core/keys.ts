// One global shortcut registry. "mod" is Ctrl on Linux and Windows, Cmd on macOS.
export interface Shortcut {
  keys: string;
  label: string;
  run: (event: KeyboardEvent) => void;
  /** When this returns false the key goes to the browser (or the focused editor) untouched. */
  when?: (event: KeyboardEvent) => boolean;
  /** Allow while typing in an input. */
  inInputs?: boolean;
}

const shortcuts = new Set<Shortcut>();
const mac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

export function registerShortcut(shortcut: Shortcut): () => void {
  shortcuts.add(shortcut);
  return () => shortcuts.delete(shortcut);
}

export const listShortcuts = () => [...shortcuts];

export function formatKeys(keys: string) {
  return keys
    .split("+")
    .map((part) => (part === "mod" ? (mac ? "⌘" : "Ctrl") : part === "shift" ? (mac ? "⇧" : "Shift") : part === "alt" ? (mac ? "⌥" : "Alt") : part.toUpperCase()))
    .join(mac ? "" : "+");
}

function matches(event: KeyboardEvent, keys: string) {
  const parts = keys.toLowerCase().split("+");
  const key = parts[parts.length - 1];
  const mod = parts.includes("mod");
  return (
    event.key.toLowerCase() === key &&
    (mac ? event.metaKey : event.ctrlKey) === mod &&
    event.shiftKey === parts.includes("shift") &&
    event.altKey === parts.includes("alt")
  );
}

if (typeof window !== "undefined") {
  window.addEventListener("keydown", (event) => {
    const target = event.target as HTMLElement | null;
    const typing = !!target?.closest("input, textarea, [contenteditable=true], .cm-editor, .xterm");
    for (const shortcut of shortcuts) {
      if ((!typing || shortcut.inInputs) && matches(event, shortcut.keys) && (shortcut.when?.(event) ?? true)) {
        event.preventDefault();
        shortcut.run(event);
        return;
      }
    }
  });
}
