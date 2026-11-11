// One session row (crates/ui shell.rs render_chat_row, sidebar branch): one line under a project
// heading, or the two-line card with avatar, title, age, location and diff size.
import { ContextMenu } from "@kobalte/core/context-menu";
import { createMemo, createSignal, type JSX, Match, onMount, Show, Switch, untrack } from "solid-js";
import type { Chat } from "@/core/protocol";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { TerminalSpinner } from "@/ui/loaders";
import { renameChat, selectChat, setArchived } from "./actions";
import { diffFor } from "./diffs";
import { dropHandlers, startDrag, endDrag } from "./drag";
import { chatEntries, MenuEntries } from "./menus";
import { avatarSpace, awaitingKind, branchOf, deviceName, harnessOf, locationLabel, projectRemotes, spaceName, type Indicator, type RowItem } from "./model";
import { dragging, isArchived, renaming, setRenaming, titleOf } from "./overlay";
import { option } from "./prefs";
import { compactCount, githubAvatar, harnessIcon, harnessTint, initials, now, singleLine, time, timeAgo } from "./util";

const LABELS: Record<Indicator, string | null> = { working: "Working", awaitingInput: "Input", errored: "Failed", completed: "Done", idle: null };

const statusColor = (status: Indicator) =>
  status === "working"
    ? "color-mix(in srgb, var(--accent) 55%, transparent)"
    : status === "awaitingInput"
      ? "color-mix(in srgb, var(--accent) 60%, transparent)"
      : status === "errored"
        ? "color-mix(in srgb, var(--danger) 65%, transparent)"
        : status === "completed"
          ? "color-mix(in srgb, var(--success) 90%, transparent)"
          : "var(--text-faint)";

/** The attention mark: each reason its own shape; routine states have none. */
function attentionGlyph(chatId: string, status: Indicator): { icon: string; color: string } | null {
  if (status === "errored") return { icon: "status-failed", color: "color-mix(in srgb, var(--danger) 80%, transparent)" };
  if (status !== "awaitingInput") return null;
  const kind = awaitingKind(chatId);
  if (kind === "plan") return { icon: "status-plan", color: "var(--success)" };
  if (kind === "permission") return { icon: "status-permission", color: "var(--warning)" };
  return { icon: "status-question", color: "var(--warning)" };
}

export function StatusMark(props: { chatId: string; status: Indicator; size?: number }) {
  const glyph = () => attentionGlyph(props.chatId, props.status);
  return (
    <span class="sb-status" role="img" aria-label={LABELS[props.status] ?? "Idle"}>
      <Switch>
        <Match when={props.status === "working"}>
          <TerminalSpinner size={13} decorative />
        </Match>
        <Match when={props.status === "completed"}>
          <span style={{ color: statusColor("completed"), display: "inline-flex" }}>
            <Icon name="check" size={11} />
          </span>
        </Match>
        <Match when={glyph()}>
          {(g) => (
            <span style={{ color: g().color, display: "inline-flex" }}>
              <Icon name={g().icon} size={props.size ?? 13} />
            </span>
          )}
        </Match>
      </Switch>
    </span>
  );
}

export function DiffStat(props: { additions: number; deletions: number }) {
  return (
    <span class="sb-diff" aria-label={`${props.additions} lines added, ${props.deletions} removed`}>
      <Show when={props.additions > 0}>
        <span class="add">+{compactCount(props.additions)}</span>
      </Show>
      <Show when={props.deletions > 0}>
        <span class="del">−{compactCount(props.deletions)}</span>
      </Show>
    </span>
  );
}

/** Repository artwork: the GitHub owner's avatar over a monogram tile. */
export function ProjectAvatar(props: { name: string; remotes: (string | null | undefined)[]; title?: string }) {
  const [failed, setFailed] = createSignal<string | null>(null);
  const url = () => githubAvatar(props.remotes);
  return (
    <span class="sb-avatar" title={props.title} aria-hidden="true">
      <span class="sb-mono">{initials(props.name)}</span>
      <Show when={url() && failed() !== url() && url()}>
        {(src) => <img src={src()} alt="" loading="lazy" referrerPolicy="no-referrer" onError={() => setFailed(src())} />}
      </Show>
    </span>
  );
}

function RenameField(props: { chat: Chat }) {
  let input!: HTMLInputElement;
  let done = false;
  /** Enter and Escape hand focus back to the row; a click elsewhere keeps it where the user put it. */
  const finish = (commit: boolean, refocus = false) => {
    if (done) return;
    done = true;
    const id = props.chat.id;
    if (commit) renameChat(id, input.value);
    setRenaming(null);
    if (refocus)
      requestAnimationFrame(() => document.querySelector<HTMLElement>(`.sb-row[data-chat-id="${CSS.escape(id)}"] .sb-row-open`)?.focus({ preventScroll: true }));
  };
  onMount(() => {
    // Opened from the row menu, the menu's close hands focus over instead (Row's onCloseAutoFocus).
    input.focus();
    input.select();
  });
  return (
    <input
      ref={input}
      class="sb-rename"
      aria-label="Session title"
      value={titleOf(props.chat)}
      onClick={(e) => e.stopPropagation()}
      onDblClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        // Row and list keys (F2, arrows, Home/End) are the field's own while editing; app shortcuts still work.
        if (!(e.ctrlKey || e.metaKey) || ["ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          finish(true, true);
        } else if (e.key === "Escape") {
          e.preventDefault();
          finish(false, true);
        }
      }}
      onBlur={() => finish(true)}
    />
  );
}

export function Row(props: { item: RowItem; search: boolean }) {
  const chat = () => state.chats[props.item.chatId];
  return <Show when={chat()}>{(c) => <RowBody chat={c()} item={props.item} search={props.search} />}</Show>;
}

function RowBody(props: { chat: Chat; item: RowItem; search: boolean }) {
  const id = () => props.chat.id;
  const status = () => props.item.indicator;
  const archived = () => isArchived(props.chat);
  const selected = createMemo(() => {
    if (view.route.kind !== "chat" || !view.selectedChatId) return false;
    const open = state.chats[view.selectedChatId];
    return (open?.threadId ?? view.selectedChatId) === id();
  });
  const twoLine = () => props.item.twoLine;
  const nested = () => props.item.nested;
  const title = () => singleLine(titleOf(props.chat)) || "New session";
  const label = () => LABELS[status()];
  const working = () => status() === "working";
  const unseenDone = () => status() === "completed";
  const space = () => avatarSpace(props.chat);
  const showAvatar = () => !nested() && option("showProjectIcon") && !!space();
  const harness = () => (option("showHarness") ? harnessOf(props.chat) : null);
  const diff = () => diffFor(props.chat);
  const avatarBadge = () => !nested() && working() && showAvatar();
  const attentionBadge = () => twoLine() && !working() && !unseenDone() && !!label() && showAvatar();
  const age = () => {
    const at = time(props.chat.lastMessageAt) || time(props.chat.createdAt);
    const text = timeAgo(at, now());
    return nested() && text !== "now" ? `${text} ago` : text;
  };
  const statusMark = () => !!label() && !unseenDone();
  const leadsTitle = () => !nested() && statusMark() && !avatarBadge() && !attentionBadge();
  const branch = () => (option("showBranch") ? branchOf(props.chat) : "");
  const tooltip = () => {
    const s = space();
    return `${s ? spaceName(s) : "No project"} · ${deviceName(props.chat.deviceId) ?? "Unknown device"}`;
  };
  let row!: HTMLDivElement;
  let keyboardMenu = false;
  const openMenu = () => {
    keyboardMenu = true;
    const rect = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: rect.left + 24, clientY: rect.bottom - 4 }));
  };

  const avatar = (): JSX.Element => (
    <Show
      when={showAvatar() && space()}
      fallback={
        <Show when={!nested() && harness()}>
          {(h) => (
            <span class="sb-avatar" style={{ display: "flex", "align-items": "center", "justify-content": "center", color: harnessTint(h()) ?? "color-mix(in srgb, var(--text-muted) 80%, transparent)" }}>
              <Icon name={harnessIcon(h())} size={14} />
            </span>
          )}
        </Show>
      }
    >
      {(s) => (
        <span class="sb-avatar-wrap">
          <ProjectAvatar name={spaceName(s())} remotes={projectRemotes(s())} title={tooltip()} />
          <Show when={avatarBadge()}>
            <span class="sb-badge" role="img" aria-label="Working"><TerminalSpinner size={10} decorative /></span>
          </Show>
          <Show when={attentionBadge()}>
            <span class="sb-badge">
              <StatusMark chatId={id()} status={status()} size={10} />
            </span>
          </Show>
        </span>
      )}
    </Show>
  );

  return (
    <ContextMenu modal={false}>
      <ContextMenu.Trigger
        as="div"
        ref={row}
        class="sb-row"
        style={{ height: `${props.item.size - props.item.top}px` }}
        data-selected={selected() ? "true" : undefined}
        data-two-line={twoLine() ? "true" : undefined}
        data-unread={unseenDone() && !archived() ? "true" : undefined}
        data-archived={archived() ? "true" : undefined}
        data-dragging={dragging() === id() ? "true" : undefined}
        data-chat-id={id()}
        draggable={!props.search && !archived() && renaming() !== id()}
        onDragStart={(e: DragEvent) => startDrag(e, id())}
        onDragEnd={endDrag}
        {...dropHandlers(() => ({ kind: "row", chatId: id(), zone: props.item.zone }))}
      >
        <button
          type="button"
          class="sb-row-open sb-focus"
          aria-current={selected() ? "page" : undefined}
          aria-label={`${title()}${label() ? `, ${label() === "Input" ? "needs input" : label()!.toLowerCase()}` : ""}${archived() ? ", archived" : ""}`}
          aria-keyshortcuts="F2 Shift+F10"
          onClick={() => selectChat(id())}
          onDblClick={() => !props.search && setRenaming(id())}
          onKeyDown={(e) => {
            if (e.key === "F2") {
              e.preventDefault();
              setRenaming(id());
            } else if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
              e.preventDefault();
              openMenu();
            }
          }}
        >
          <Show when={nested()}>
            <span class="sb-row-lead">
              <Show
                when={statusMark()}
                fallback={
                  <span class="sb-branch" title={branchOf(props.chat) || undefined}>
                    <Icon name="git-state-branch" size={12} />
                  </span>
                }
              >
                <StatusMark chatId={id()} status={status()} />
              </Show>
            </span>
          </Show>
          {avatar()}
          <span class="sb-row-body">
            <span class="sb-row-line">
              <Show when={leadsTitle()}>
                <StatusMark chatId={id()} status={status()} />
              </Show>
              <Show when={renaming() === id()} fallback={<span class="sb-row-title">{title()}</span>}>
                <RenameField chat={props.chat} />
              </Show>
            </span>
            <Show when={twoLine()}>
              <span class="sb-row-meta">
                <span class="sb-row-loc">
                  {locationLabel(props.chat)}
                  <Show when={branch()}>{(b) => <> · {b()}</>}</Show>
                </span>
                <Show when={diff()}>{(d) => <DiffStat additions={d().additions} deletions={d().deletions} />}</Show>
              </span>
            </Show>
          </span>
        </button>
        <span class="sb-row-trail">
          <Switch fallback={<span class="sb-row-time">{age()}</span>}>
            <Match when={!twoLine() && diff()}>{(d) => <DiffStat additions={d().additions} deletions={d().deletions} />}</Match>
            <Match when={twoLine() && attentionBadge() && label()}>
              {(text) => (
                <span class="sb-row-attn" style={{ color: status() === "awaitingInput" ? "var(--warning)" : statusColor(status()) }}>
                  {text() === "Input" ? "Needs input" : text()}
                </span>
              )}
            </Match>
          </Switch>
          <button
            type="button"
            class="sb-row-archive"
            aria-label={archived() ? `Unarchive ${title()}` : `Archive ${title()}`}
            title={archived() ? "Unarchive session" : "Archive session"}
            onClick={(e) => {
              e.stopPropagation();
              setArchived(id(), !archived());
            }}
          >
            <Icon name={archived() ? "archive-up-minimalistic" : "archive-minimalistic"} size={13} />
          </button>
        </span>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content
          class="menu"
          aria-label={`Actions for ${title()}`}
          onCloseAutoFocus={(e: Event) => {
            // The trigger is not focusable: hand focus to the rename field, or back to the row when
            // the menu came from the keyboard.
            e.preventDefault();
            const fromKeyboard = keyboardMenu;
            keyboardMenu = false;
            const field = renaming() === id() ? row.querySelector<HTMLInputElement>(".sb-rename") : null;
            if (field) {
              field.focus({ preventScroll: true });
              field.select();
            } else if (fromKeyboard) row.querySelector<HTMLElement>(".sb-row-open")?.focus({ preventScroll: true });
          }}
        >
          <MenuEntries entries={untrack(() => chatEntries(id()))} />
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu>
  );
}
