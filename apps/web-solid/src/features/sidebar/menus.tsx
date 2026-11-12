// Menu contents for rows, project headings, sections and view options. Kobalte's menu parts are
// shared by dropdown and context menus, so one renderer serves both.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { For, type JSX, Match, Show, Switch, untrack } from "solid-js";
import { state } from "@/core/store";
import { navigate } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { forkChat, openProjectArchive, setArchived } from "./actions";
import { isArchived, setDialog, setRenaming } from "./overlay";
import { projectKey } from "./model";
import { changeSection, pinnedIds, sections, setPinned } from "./pins";
import { option, organization, setOption, setOrganization } from "./prefs";
import { copyText } from "./util";

export type Entry =
  | { kind: "item"; label: string; icon?: string; danger?: boolean; disabled?: boolean; checked?: boolean; keepOpen?: boolean; onSelect: () => void }
  | { kind: "sub"; label: string; icon?: string; entries: () => Entry[] }
  | { kind: "custom"; label: string; icon?: string; meta?: string; content: () => JSX.Element }
  | { kind: "separator" }
  | { kind: "heading"; label: string };

export function MenuEntries(props: { entries: Entry[] }) {
  return (
    <For each={props.entries}>
      {(entry) => (
        <Switch>
          <Match when={entry.kind === "separator"}>
            <DropdownMenu.Separator class="menu-sep sb-sep" />
          </Match>
          <Match when={entry.kind === "heading" && entry}>
            {(heading) => <div class="menu-label">{heading().label}</div>}
          </Match>
          <Match when={entry.kind === "item" && entry.checked === undefined && entry}>
            {(item) => (
              <DropdownMenu.Item
                class="menu-item"
                data-danger={item().danger ? "true" : undefined}
                disabled={item().disabled}
                closeOnSelect={!item().keepOpen}
                onSelect={() => item().onSelect()}
              >
                <Show when={item().icon}>{(name) => <Icon name={name()} size={14} />}</Show>
                <span>{item().label}</span>
              </DropdownMenu.Item>
            )}
          </Match>
          <Match when={entry.kind === "item" && entry.checked !== undefined && entry}>
            {(item) => (
              <DropdownMenu.CheckboxItem
                class="menu-item"
                checked={!!item().checked}
                disabled={item().disabled}
                closeOnSelect={!item().keepOpen}
                onChange={() => item().onSelect()}
              >
                <Show when={item().icon}>{(name) => <Icon name={name()} size={14} />}</Show>
                <span>{item().label}</span>
                <DropdownMenu.ItemIndicator class="sb-menu-check">
                  <Icon name="check" size={14} />
                </DropdownMenu.ItemIndicator>
              </DropdownMenu.CheckboxItem>
            )}
          </Match>
          <Match when={entry.kind === "sub" && entry}>
            {(sub) => (
              <DropdownMenu.Sub overlap gutter={4}>
                <DropdownMenu.SubTrigger class="menu-item">
                  <Show when={sub().icon}>{(name) => <Icon name={name()} size={14} />}</Show>
                  <span>{sub().label}</span>
                  <span class="sb-menu-sub"><Icon name="alt-arrow-right" size={14} /></span>
                </DropdownMenu.SubTrigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.SubContent class="menu">
                    <MenuEntries entries={untrack(sub().entries)} />
                  </DropdownMenu.SubContent>
                </DropdownMenu.Portal>
              </DropdownMenu.Sub>
            )}
          </Match>
          <Match when={entry.kind === "custom" && entry}>
            {(custom) => (
              <DropdownMenu.Sub overlap gutter={4}>
                <DropdownMenu.SubTrigger class="menu-item">
                  <Show when={custom().icon}>{(name) => <Icon name={name()} size={14} />}</Show>
                  <span style={{ flex: 1, "min-width": 0 }}>{custom().label}</span>
                  <Show when={custom().meta}>{(meta) => <span class="sb-usage-meta">{meta()}</span>}</Show>
                  <span class="sb-menu-sub"><Icon name="alt-arrow-right" size={14} /></span>
                </DropdownMenu.SubTrigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.SubContent class="menu sb-detail">{custom().content()}</DropdownMenu.SubContent>
                </DropdownMenu.Portal>
              </DropdownMenu.Sub>
            )}
          </Match>
        </Switch>
      )}
    </For>
  );
}

/** A dropdown on any trigger element, filled with entries. */
export function EntriesMenu(props: {
  label: string;
  entries: () => Entry[];
  trigger: (triggerProps: Record<string, unknown>) => JSX.Element;
  placement?: "bottom-start" | "bottom-end" | "top-start" | "top-end" | "right-start";
  onOpenChange?: (open: boolean) => void;
  class?: string;
}) {
  return (
    <DropdownMenu placement={props.placement ?? "bottom-end"} gutter={4} modal={false} onOpenChange={(open) => props.onOpenChange?.(open)}>
      <DropdownMenu.Trigger as={(triggerProps: Record<string, unknown>) => props.trigger(triggerProps)} aria-label={props.label} />
      <DropdownMenu.Portal>
        <DropdownMenu.Content class={`menu ${props.class ?? ""}`} aria-label={props.label}>
          <MenuEntries entries={untrack(props.entries)} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

// ---- chat rows (crates/ui shell.rs render_overlays, chat menu) ----

export function chatEntries(chatId: string): Entry[] {
  const chat = state.chats[chatId];
  if (!chat) return [];
  const archived = isArchived(chat);
  const pinned = pinnedIds().includes(chatId);
  const all = sections();
  const current = all.find((s) => s.sessionIds.includes(chatId));
  const entries: Entry[] = [{ kind: "item", label: "Rename", icon: "pen", onSelect: () => setRenaming(chatId) }];
  if (!archived) {
    entries.push({ kind: "item", label: pinned ? "Unpin" : "Pin", icon: "pin", onSelect: () => setPinned(chatId, !pinned) });
    entries.push({
      kind: "sub",
      label: "Move to section",
      icon: "list",
      entries: () => [
        ...all.map(
          (section): Entry => ({
            kind: "item",
            label: section.name,
            checked: section.id === current?.id,
            onSelect: () => changeSection({ action: "assign", sessionId: chatId, sectionId: section.id }),
          }),
        ),
        ...(all.length ? [{ kind: "separator" } as Entry] : []),
        { kind: "item", label: "New section…", icon: "plus", onSelect: () => setDialog({ kind: "section", sectionId: null, assign: chatId }) },
        ...(current
          ? [{ kind: "item", label: "Remove from section", icon: "close", onSelect: () => changeSection({ action: "assign", sessionId: chatId, sectionId: null }) } as Entry]
          : []),
      ],
    });
    entries.push({ kind: "item", label: "Fork to new thread", icon: "fork", onSelect: () => void forkChat(chatId) });
  }
  entries.push({
    kind: "item",
    label: archived ? "Unarchive" : "Archive",
    icon: archived ? "archive-up-minimalistic" : "archive-minimalistic",
    onSelect: () => setArchived(chatId, !archived),
  });
  const sessionId = typeof chat.harnessSessionId === "string" && chat.harnessSessionId.trim() ? chat.harnessSessionId : null;
  entries.push({
    kind: "sub",
    label: "Copy",
    icon: "copy",
    entries: () => [
      ...(chat.cwd ? [{ kind: "item", label: "Path", icon: "copy", onSelect: () => void copyText(chat.cwd!, "Path") } as Entry] : []),
      { kind: "item", label: "Chat ID", icon: "copy", onSelect: () => void copyText(chat.id, "Chat ID") },
      ...(sessionId ? [{ kind: "item", label: "Harness session ID", icon: "copy", onSelect: () => void copyText(sessionId, "Harness session ID") } as Entry] : []),
    ],
  });
  entries.push({ kind: "separator" });
  entries.push({ kind: "item", label: "Delete…", icon: "trash-bin-minimalistic", danger: true, onSelect: () => setDialog({ kind: "deleteChat", chatId }) });
  return entries;
}

// ---- project headings (shell/spaces.rs render_space_overlays) ----

export function projectEntries(spaceId: string): Entry[] {
  const space = state.spaces[spaceId];
  if (!space) return [];
  return [
    { kind: "item", label: "Rename…", icon: "pen", onSelect: () => setDialog({ kind: "renameProject", spaceId }) },
    { kind: "item", label: "Archived chats", icon: "dither-archive", onSelect: () => openProjectArchive(projectKey(space)) },
    { kind: "item", label: "Usage", icon: "dashboard-speed", onSelect: () => navigate({ kind: "usage" }) },
    { kind: "item", label: "Remove…", icon: "trash-bin-minimalistic", danger: true, onSelect: () => setDialog({ kind: "removeProject", spaceId }) },
  ];
}

// ---- custom sections (shell/sidebar_sections.rs) ----

export function sectionEntries(sectionId: string): Entry[] {
  const section = sections().find((s) => s.id === sectionId);
  if (!section) return [];
  const live = section.sessionIds.filter((id) => {
    const chat = state.chats[id];
    return chat && !isArchived(chat);
  });
  return [
    { kind: "item", label: "Edit section", icon: "pen", onSelect: () => setDialog({ kind: "section", sectionId }) },
    { kind: "item", label: "Archive all", icon: "archive-minimalistic", disabled: !live.length, onSelect: () => live.forEach((id) => setArchived(id, true)) },
    { kind: "item", label: "Delete", icon: "trash-bin-minimalistic", danger: true, onSelect: () => changeSection({ action: "delete", id: sectionId }) },
  ];
}

// ---- view options (shell/spaces.rs render_sidebar_view_menu) ----

export function viewEntries(): Entry[] {
  // Getters keep rows mounted while toggles that leave the menu open change.
  const toggle = (label: string, icon: string, key: "showBranch" | "showHarness" | "showProjectIcon" | "showLocation" | "showProjectFilter"): Entry => ({
    kind: "item",
    label,
    icon,
    get checked() {
      return option(key);
    },
    keepOpen: true,
    onSelect: () => setOption(key, !option(key)),
  });
  return [
    { kind: "heading", label: "View" },
    { kind: "item", label: "Projects", icon: "folder", get checked() { return organization() === "projects"; }, onSelect: () => setOrganization("projects") },
    { kind: "item", label: "Threads", icon: "list", get checked() { return organization() === "threads"; }, onSelect: () => setOrganization("threads") },
    {
      kind: "sub",
      label: "Sort",
      icon: "sort-vertical",
      entries: () => [
        { kind: "item", label: "Last updated", icon: "clock-circle", get checked() { return option("sort") === "updated"; }, onSelect: () => setOption("sort", "updated") },
        { kind: "item", label: "Created", icon: "calendar", get checked() { return option("sort") === "created"; }, onSelect: () => setOption("sort", "created") },
      ],
    },
    {
      kind: "sub",
      label: "Show",
      icon: "eye",
      entries: () => [
        toggle("Branch", "git-branch", "showBranch"),
        toggle("Harness", "bot", "showHarness"),
        toggle("Project icon", "project-default", "showProjectIcon"),
        toggle("Location", "remote-server", "showLocation"),
        toggle("Project filter", "folder", "showProjectFilter"),
      ],
    },
    {
      kind: "item",
      label: "Group by device",
      icon: "monitor",
      get checked() {
        return organization() === "devices";
      },
      keepOpen: true,
      onSelect: () => (organization() === "devices" ? setOption("byDevice", false) : setOrganization("devices")),
    },
    { kind: "item", label: "Compact", icon: "fold-vertical", get checked() { return option("compact"); }, keepOpen: true, onSelect: () => setOption("compact", !option("compact")) },
    { kind: "separator" },
    { kind: "item", label: "Create section", icon: "plus", onSelect: () => setDialog({ kind: "section", sectionId: null }) },
  ];
}
