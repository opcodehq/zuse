// The cheat sheet of shortcuts in effect right now (desktop's shell/chat_shortcuts.rs panel).
// Application and chat rows come from what core/keys has registered, so a shortcut only shows once something handles it;
// composer rows are the keys the composer handles itself.
import { For, onMount } from "solid-js";
import { formatKeys, listShortcuts } from "@/core/keys";
import { navigate } from "@/core/view";
import { IconButton } from "@/ui/primitives";
import { closeOverlay } from "./overlay";
import { modalKeys } from "./modal";

interface Row {
  label: string;
  keys: string;
}

const mac = /Mac|iPhone|iPad/.test(navigator.platform);

const COMPOSER: Row[] = [
  { label: "Send message", keys: mac ? "↩" : "Enter" },
  { label: "Insert newline", keys: mac ? "⇧↩" : "Shift+Enter" },
  { label: "Send message now", keys: mac ? "⌘↩" : "Ctrl+Enter" },
  { label: "Previous sent message", keys: "↑" },
  { label: "Next sent message", keys: "↓" },
  { label: "Stop agent (press twice)", keys: "Esc" },
];

const TRIGGERS: Row[] = [
  { label: "Run a command", keys: "/" },
  { label: "Mention a file", keys: "@" },
];

/** Desktop's CHAT_COMMANDS; every other registered shortcut is an application one. */
const CHAT_LABELS = new Set([
  "Switch model",
  "Change reasoning effort",
  "Toggle fast mode",
  "Expand or collapse tool details",
  "Show or hide task list",
  "Find in chat",
  "Stop background tasks",
]);

export function Shortcuts() {
  let card: HTMLDivElement | undefined;
  let close: HTMLButtonElement | undefined;
  // Read once on open: the registry is a plain set, and the panel is rebuilt each time it opens.
  const seen = new Set<string>();
  const registered = listShortcuts()
    .filter((s) => !seen.has(s.keys) && seen.add(s.keys))
    .map((s) => ({ label: s.label, keys: formatKeys(s.keys) }));
  const chat = registered.filter((row) => CHAT_LABELS.has(row.label));
  const app = registered.filter((row) => !CHAT_LABELS.has(row.label));
  onMount(() => close?.focus());

  return (
    <div class="x-scrim" onPointerDown={(e) => e.target === e.currentTarget && closeOverlay()}>
      <div
        ref={card}
        class="x-dialog x-shortcuts"
        role="dialog"
        aria-modal="true"
        aria-labelledby="x-shortcuts-title"
        aria-describedby="x-shortcuts-description"
        onKeyDown={modalKeys(() => card, () => closeOverlay())}
      >
        <div class="x-dialog-head">
          <h2 id="x-shortcuts-title" class="x-dialog-title">Keyboard shortcuts</h2>
          <IconButton ref={close} icon="close" label="Close" onClick={() => closeOverlay()} />
        </div>
        <p id="x-shortcuts-description" class="x-dialog-body x-shortcuts-description">
          Type ? in an empty composer to open this anytime.
        </p>
        <div class="x-shortcuts-columns">
          <div class="x-shortcuts-column">
            <ShortcutSection title="Composer" rows={COMPOSER} />
            <ShortcutSection title="Type in the composer" rows={TRIGGERS} />
          </div>
          <div class="x-shortcuts-column">
            <ShortcutSection title="Chat" rows={chat} />
            <ShortcutSection title="Application" rows={app} />
          </div>
        </div>
        <div class="x-shortcuts-foot">
          <button
            type="button"
            class="x-ghost"
            onClick={() => {
              closeOverlay(false);
              navigate({ kind: "settings", section: "shortcuts" });
            }}
          >
            Change shortcuts in Settings
          </button>
        </div>
      </div>
    </div>
  );
}

function ShortcutSection(props: { title: string; rows: Row[] }) {
  return (
    <section class="x-shortcuts-section" aria-label={props.title} hidden={!props.rows.length}>
      <h3 class="x-shortcuts-heading">{props.title}</h3>
      <dl class="x-shortcuts-list">
        <For each={props.rows}>
          {(row) => (
            <div class="x-shortcut">
              <dt>{row.label}</dt>
              <dd><kbd class="x-chip-key">{row.keys}</kbd></dd>
            </div>
          )}
        </For>
      </dl>
    </section>
  );
}
