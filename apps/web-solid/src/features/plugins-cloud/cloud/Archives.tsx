// Archived cloud chats with recovery and cleanup progress (crates/viewport/src/cloud_archive_cards.rs).
import { createMemo, createSignal, For, onMount, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { Dialog } from "@/ui/primitives";
import { account, owner, signedIn } from "../account";
import { Action, SearchField } from "../ui";
import {
  archiveStatus,
  cleanupFailed,
  cleanupPending,
  cloud,
  deleteFailed,
  deleteWorkspace,
  isArchived,
  isDeleting,
  loadWorkspaces,
  openHistory,
  restoreWorkspace,
  retryCleanup,
  workspaceBusy,
  workspaceLabel,
} from "./state";
import type { ArchiveStatus, Workspace } from "./types";

export const archiveVisible = (row: Workspace) => isArchived(row) || isDeleting(row) || (row.state === "deleted" && (cleanupPending(row) || cleanupFailed(row)));

const DELETION: Record<string, string> = {
  "delete-waiting-for-execution": "Stopping background work…",
  "delete-provider-cleanup": "Removing cloud workspace…",
  "delete-purging-data": "Removing saved history and backups…",
  "delete-clearing-metadata": "Finishing deletion…",
};
const ATTENTION = new Set(["archive-metadata-invalid", "archive-source-missing", "archive-source-shared", "cloud_archive_operation_fenced"]);

function status(row: Workspace): { label: string; description: string; attention: boolean } {
  if (row.state === "deleted") {
    return {
      label: "Cloud deleted",
      description: cleanupFailed(row) ? "Cloud data is deleted. Local cleanup still needs attention." : "Cloud data is deleted. Finishing local cleanup.",
      attention: cleanupFailed(row),
    };
  }
  if (isDeleting(row)) {
    return {
      label: deleteFailed(row) ? "Deletion needs attention" : "Deleting",
      description: deleteFailed(row) ? "Deletion could not finish. Retry to continue cleanup." : DELETION[row.statusCode] ?? "Deleting cloud thread…",
      attention: deleteFailed(row),
    };
  }
  if (row.statusCode.includes("failed") || row.statusCode.includes("incomplete") || ATTENTION.has(row.statusCode)) {
    return { label: "Archive needs attention", description: "Archive preparation could not finish. Refresh to check its current state.", attention: true };
  }
  if (row.state !== "archived") return { label: "Archiving", description: "Saving this chat before it moves to the archive.", attention: false };
  return { label: "Archived", description: "Saved history is available.", attention: false };
}

function recoveryTitle(value: ArchiveStatus) {
  if (value.errorCode) return "Recovery needs attention";
  switch (value.phase) {
    case "uploading":
      return "Uploading backup";
    case "verifying":
      return "Verifying backup";
    case "retiring":
      return "Retiring compute";
    case "restoring":
      return "Restoring files";
    default:
      return value.backupVerified ? "Backup verified" : "Backup not yet verified";
  }
}

function matches(row: Workspace, query: string) {
  const needle = query.trim().toLowerCase();
  if (!archiveVisible(row)) return false;
  if (!needle) return true;
  const recovery = archiveStatus(row);
  return `${workspaceLabel(row)} ${row.branch} ${status(row).label} ${cleanupFailed(row) ? "cleanup" : ""} ${recovery ? recoveryTitle(recovery) : ""}`.toLowerCase().includes(needle);
}

export function Archives() {
  const [query, setQuery] = createSignal("");
  const [confirm, setConfirm] = createSignal<string | null>(null);
  const ready = () => signedIn() && !owner()?.organizationId;
  onMount(() => {
    if (ready() && !cloud.workspaces.loading) void loadWorkspaces();
  });
  const rows = createMemo(() => cloud.workspaces.rows.filter((row) => matches(row, query())));
  const busy = workspaceBusy;
  const enabled = () => ready() && !cloud.workspaces.loading && !busy() && !cloud.history.loading;
  const gate = () => {
    if (account() === null) return "Loading your account…";
    if (!signedIn()) return "Sign in to view your Cloud archives.";
    if (owner()?.organizationId) return "Switch to your personal account to view Cloud archives.";
    return null;
  };
  const total = () => cloud.workspaces.rows.filter(archiveVisible).length;
  return (
    <section class="pc-frame" aria-label="Cloud archives">
      <header class="pc-frame-head">
        <div class="pc-frame-title pc-grow">
          <h3>{ready() && total() > 0 ? `Cloud archives · ${total()}` : "Cloud archives"}</h3>
          <p class="pc-frame-sub">Saved conversations and cleanup progress. Restored chats stay paused.</p>
        </div>
        <Show when={ready() && total() > 3}>
          <SearchField value={query()} onInput={setQuery} placeholder="Search archives" label="Search Cloud archives" width={180} />
        </Show>
        <Action disabled={!enabled()} onClick={() => void loadWorkspaces()}>
          {cloud.workspaces.loading ? "Refreshing…" : "Refresh"}
        </Action>
      </header>
      <div class="pc-archive-list">
        <Show
          when={!gate()}
          fallback={<p class="pc-row-description pc-pad">{gate()}</p>}
        >
          <Show when={cloud.workspaces.error}>
            {(error) => (
              <div class="pc-inline-alert" role="alert">
                {error()}
              </div>
            )}
          </Show>
          <Show when={rows().length === 0}>
            <p class="pc-row-description pc-pad">
              {cloud.workspaces.loading
                ? "Loading Cloud archives…"
                : cloud.workspaces.error
                  ? "Refresh to load your Cloud archives."
                  : query().trim()
                    ? "No Cloud archives match your search."
                    : "No archived Cloud chats."}
            </p>
          </Show>
          <For each={rows()}>{(row) => <Card row={row} enabled={enabled()} onDelete={() => setConfirm(row.workspaceId)} />}</For>
        </Show>
      </div>
      <Dialog
        open={confirm() !== null}
        onOpenChange={(next) => !next && setConfirm(null)}
        title="Delete Cloud chat?"
        description="This permanently removes all conversations, Cloud files and recovery backups for this chat. This cannot be undone."
        footer={
          <>
            <Action onClick={() => setConfirm(null)}>Cancel</Action>
            <button
              type="button"
              class="btn"
              data-variant="danger"
              onClick={() => {
                const id = confirm();
                setConfirm(null);
                if (id) void deleteWorkspace(id);
              }}
            >
              Delete permanently
            </button>
          </>
        }
      />
    </section>
  );
}

function Card(props: { row: Workspace; enabled: boolean; onDelete: () => void }) {
  const info = () => status(props.row);
  const pending = () => cloud.workspaces.pending[props.row.workspaceId];
  const deleted = () => props.row.state === "deleted";
  const deleting = () => isDeleting(props.row);
  const opening = () => cloud.history.loading && cloud.history.workspaceId === props.row.workspaceId;
  const recovery = () => archiveStatus(props.row);
  const cleanup = () => props.row.localMirrorCleanup;
  const rowError = () => (cloud.workspaces.rowError?.id === props.row.workspaceId ? cloud.workspaces.rowError.message : null);
  return (
    <article class="pc-archive-card" aria-label={workspaceLabel(props.row)}>
      <div class="pc-archive-head">
        <span class="pc-archive-icon" aria-hidden="true">
          <Icon name="cloud" size={16} />
        </span>
        <div class="pc-row-copy">
          <div class="pc-archive-title pc-truncate">{workspaceLabel(props.row)}</div>
          <Show when={props.row.branch}>
            <div class="pc-row-description pc-truncate">Branch {props.row.branch}</div>
          </Show>
        </div>
        <span class="pc-archive-status" data-attention={info().attention ? "true" : undefined}>
          {info().label}
        </span>
      </div>
      <p class="pc-archive-description">{info().description}</p>
      <Show when={recovery()}>
        {(value) => (
          <div class="pc-recovery">
            <strong data-attention={value().errorCode ? "true" : undefined}>{recoveryTitle(value())}</strong>
            <span>{value().computeRetired ? "Compute retired" : value().machineRetained ? "Machine retained" : "No retained machine"}</span>
            <Show when={value().backupVerified && recoveryTitle(value()) !== "Backup verified"}>
              <span>Backup verified</span>
            </Show>
            <Show when={value().phase === "verifying" && value().totalChunks > 0 && value().verifiedChunks <= value().totalChunks}>
              <span>
                {value().verifiedChunks} of {value().totalChunks} backup parts verified
              </span>
            </Show>
            <Show when={value().retentionDeadline}>
              {(deadline) => (
                <>
                  <span>Compute retention deadline: {new Date(deadline()).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })}</span>
                  <Show when={!value().computeRetired}>
                    <span>Retirement waits for a verified backup.</span>
                  </Show>
                </>
              )}
            </Show>
            <Show when={value().errorCode}>
              <span>Refresh to check this archive again.</span>
            </Show>
          </div>
        )}
      </Show>
      <Show when={cleanup()}>
        {(value) => (
          <>
            <Show when={value().state === "pending"}>
              <p class="pc-archive-description">Cleaning up local files…</p>
            </Show>
            <Show when={value().state === "error"}>
              <div class="pc-inline-alert" role="alert">
                {value().error ?? "Local cleanup could not finish. Retry to continue."}
              </div>
            </Show>
            <Show when={value().removed > 0 || value().preserved > 0}>
              <p class="pc-row-description">
                Local files: {value().removed} removed · {value().preserved} kept
              </p>
            </Show>
          </>
        )}
      </Show>
      <Show when={rowError()}>
        {(message) => (
          <div class="pc-inline-alert" role="alert">
            {message()}
          </div>
        )}
      </Show>
      <div class="pc-archive-actions">
        <Show when={!deleting() && !deleted()}>
          <Action disabled={!props.enabled} onClick={() => void openHistory(props.row.workspaceId, props.row.initialSessionId || null)}>
            {opening() ? "Opening…" : "View history"}
          </Action>
          <Action disabled={!props.enabled || props.row.state !== "archived"} onClick={() => void restoreWorkspace(props.row.workspaceId)}>
            {pending() === "restore" ? "Restoring…" : "Restore chat"}
          </Action>
        </Show>
        <Show when={!deleted()}>
          <Action
            class="pc-action-danger"
            disabled={!props.enabled || (deleting() && !deleteFailed(props.row))}
            onClick={() => (deleteFailed(props.row) ? void deleteWorkspace(props.row.workspaceId) : props.onDelete())}
          >
            {pending() === "delete" ? "Deleting…" : deleteFailed(props.row) ? "Retry deletion" : deleting() ? "Deleting…" : "Delete…"}
          </Action>
        </Show>
        <Show when={cleanupFailed(props.row)}>
          <Action disabled={!props.enabled} onClick={() => void retryCleanup(props.row.workspaceId)}>
            {pending() === "cleanup" ? "Retrying…" : "Retry local cleanup"}
          </Action>
        </Show>
      </div>
    </article>
  );
}
