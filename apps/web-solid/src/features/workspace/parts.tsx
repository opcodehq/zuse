// Small presentational pieces shared by workspace surfaces.
import { type JSX, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { Spinner } from "@/ui/primitives";
import { fileIconUrl } from "./fileIcons";
import type { ChangeRequestState, GitFileState } from "./types";

export function DiffStat(props: { additions: number; deletions: number }) {
  return (
    <span class="ws-diffstat" aria-label={`${props.additions} additions, ${props.deletions} deletions`}>
      <span class="ws-add">+{props.additions}</span>
      <span class="ws-del">−{props.deletions}</span>
    </span>
  );
}

export function PrStateIcon(props: { state: ChangeRequestState | string; draft?: boolean; size?: number }) {
  const name = () => {
    const s = props.state.toLowerCase();
    if (s === "merged") return "git-state-merged";
    if (s === "closed") return "git-state-closed";
    return props.draft ? "git-state-draft" : "git-state-open";
  };
  const tone = () => {
    const s = props.state.toLowerCase();
    return s === "merged" ? "merged" : s === "closed" ? "danger" : props.draft ? "muted" : "success";
  };
  return <Icon name={name()} size={props.size ?? 14} class="ws-pr-icon" data-tone={tone()} label={props.draft ? `Draft, ${props.state}` : props.state} />;
}

export function FileIcon(props: { name: string; directory?: boolean; open?: boolean; size?: number }) {
  return (
    <Show
      when={!props.directory}
      fallback={<Icon name={props.open ? "folder-open" : "folder"} size={props.size ?? 14} class="ws-folder-icon" />}
    >
      <img class="ws-file-icon" src={fileIconUrl(props.name)} width={props.size ?? 14} height={props.size ?? 14} alt="" aria-hidden="true" draggable={false} />
    </Show>
  );
}

const LETTERS: Record<GitFileState, string> = {
  added: "A",
  modified: "M",
  deleted: "D",
  renamed: "R",
  copied: "C",
  unmerged: "!",
  untracked: "U",
  typeChanged: "T",
  unchanged: "",
};

export function statusLetter(state: GitFileState) {
  return LETTERS[state] ?? "";
}

/** Tone for a Git state: additions green, deletions red, conflicts warn, the rest modified. */
export function statusTone(state: GitFileState | string) {
  switch (state) {
    case "added":
    case "untracked":
    case "A":
    case "U":
      return "add";
    case "deleted":
    case "D":
      return "del";
    case "unmerged":
    case "!":
      return "warn";
    default:
      return "mod";
  }
}

export function PanelHeader(props: { title: string; children?: JSX.Element; icon?: string }) {
  return (
    <div class="ws-panel-header">
      <Show when={props.icon}>{(name) => <Icon name={name()} size={14} class="ws-muted" />}</Show>
      <h2 class="ws-panel-title">{props.title}</h2>
      <div class="ws-grow" />
      {props.children}
    </div>
  );
}

export function Empty(props: { icon?: string; title: string; detail?: string; children?: JSX.Element }) {
  return (
    <div class="ws-empty">
      <Show when={props.icon}>{(name) => <Icon name={name()} size={20} class="ws-faint" />}</Show>
      <div class="ws-empty-title">{props.title}</div>
      <Show when={props.detail}>
        <div class="ws-empty-detail">{props.detail}</div>
      </Show>
      {props.children}
    </div>
  );
}

export function Loading(props: { label: string }) {
  return (
    <div class="ws-empty">
      <Spinner label={props.label} />
      <div class="ws-empty-detail">{props.label}</div>
    </div>
  );
}

export function Banner(props: { tone?: "danger" | "warning" | "info"; children: JSX.Element; actions?: JSX.Element }) {
  return (
    <div class="ws-banner" data-tone={props.tone ?? "info"} role={props.tone === "danger" ? "alert" : "status"}>
      <Icon name={props.tone === "info" || !props.tone ? "info-circle" : "danger-triangle"} size={14} />
      <div class="ws-grow">{props.children}</div>
      <Show when={props.actions}>
        <div class="ws-banner-actions">{props.actions}</div>
      </Show>
    </div>
  );
}
