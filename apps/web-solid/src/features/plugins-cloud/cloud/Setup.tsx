// Cloud Setup: image, repositories and GitHub (crates/viewport/src/cloud_repositories), plus retained image storage for Usage.
import { writeClipboard } from "@/ui/clipboard";
import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { Dialog, IconButton, toast } from "@/ui/primitives";
import { Action, Badge, Group, Note, Row } from "../ui";
import { usd } from "./money";
import {
  buildImage,
  buildRetry,
  canManageRepositories,
  clearRepoNotice,
  cloud,
  connectRepositories,
  deleteImage,
  loadImage,
  loadRepositories,
  removeRepository,
} from "./state";
import type { GithubRepository, ImageBuild } from "./types";

const identity = (value: string) => value.replace(/^github\.com\//, "").toLowerCase();
const githubConnected = () => !!cloud.repos.github?.installations.some((install) => !install.suspended);
const repositoryBusy = () => cloud.repos.listing || cloud.repos.githubLoading || cloud.repos.connecting.length > 0 || !!cloud.repos.removing;

export function Avatar(props: { name: string; url?: string | null; size: number }) {
  const [failed, setFailed] = createSignal(false);
  const fallback = () => (props.name.split("/")[0] ?? props.name).slice(0, 2).toUpperCase();
  return (
    <span class="pc-repo-avatar" aria-hidden="true" style={{ width: `${props.size}px`, height: `${props.size}px` }}>
      <Show when={props.url?.startsWith("https://") && !failed() ? props.url : null} fallback={fallback()}>
        {(src) => <img src={src()} alt="" loading="lazy" onError={() => setFailed(true)} />}
      </Show>
    </span>
  );
}

/** The Setup tab: image first, then GitHub before repositories until GitHub is connected. */
export function Setup() {
  onMount(() => {
    if (!cloud.repos.loaded) void loadRepositories();
  });
  return (
    <div class="pc-stack">
      <ImageGroup />
      <Show
        when={githubConnected()}
        fallback={
          <>
            <GithubGroup />
            <Repositories />
          </>
        }
      >
        <Repositories />
        <GithubGroup />
      </Show>
    </div>
  );
}

function Repositories() {
  const empty = () => cloud.repos.projects.length === 0;
  return (
    <Group
      title="Repositories"
      help="Choose the personal, organization, and collaborator repositories included in your private cloud image."
      action={
        <Show when={!empty()}>
          <RepositoryPicker iconOnly />
        </Show>
      }
    >
      <Show when={empty()}>
        <div class="pc-repo-empty">
          <span>{repositoryBusy() ? "Loading repositories…" : "No repositories included"}</span>
          <RepositoryPicker />
        </div>
      </Show>
      <For each={cloud.repos.projects}>
        {(project) => {
          const name = () => project.repositoryIdentity.replace(/^github\.com\//, "");
          const repo = () => cloud.repos.github?.repositories.find((r) => r.nameWithOwner.toLowerCase() === name().toLowerCase());
          return (
            <div class="pc-repo-row">
              <Avatar name={name()} url={repo()?.ownerAvatarUrl} size={24} />
              <div class="pc-row-copy">
                <div class="pc-row-title pc-truncate">{project.displayName}</div>
                <div class="pc-repo-sub pc-truncate">{project.defaultBranch} · selected for cloud image</div>
              </div>
              <button
                type="button"
                class="pc-action"
                data-icon-only="true"
                data-busy={cloud.repos.removing === project.projectId ? "true" : undefined}
                aria-label={`Remove ${project.displayName}`}
                title={`Remove ${project.displayName}`}
                disabled={repositoryBusy() || !canManageRepositories()}
                onClick={() => void removeRepository(project.projectId)}
              >
                <span class="pc-action-label">
                  <Icon name="close" size={14} />
                </span>
                <Show when={cloud.repos.removing === project.projectId}>
                  <GlyphSpinner cell={1.75} tone="current" decorative class="pc-action-spinner" />
                </Show>
              </button>
            </div>
          );
        }}
      </For>
      <Show when={cloud.repos.connecting.length > 0}>
        <Note>Adding {cloud.repos.connecting.join(", ")}…</Note>
      </Show>
      <Show when={cloud.repos.notice}>{(notice) => <Note>{notice()}</Note>}</Show>
      <Show when={cloud.repos.error}>
        {(error) => (
          <div class="pc-inline-alert" role="alert">
            <span>{error()}</span>
            <button type="button" class="pc-error-dismiss" aria-label="Dismiss" onClick={clearRepoNotice}>
              <Icon name="close" size={12} />
            </button>
          </div>
        )}
      </Show>
    </Group>
  );
}

/** Desktop's anchored repository picker: search, multi-select, then add the selection one by one. */
function RepositoryPicker(props: { iconOnly?: boolean }) {
  const [open, setOpen] = createSignal(false);
  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal<string[]>([]);
  // The header picker opens leftward from its trigger, so it may be no wider than the space to the trigger's right edge.
  const [room, setRoom] = createSignal<number | null>(null);
  let root: HTMLDivElement | undefined;
  let trigger: HTMLButtonElement | undefined;
  let search: HTMLInputElement | undefined;
  const disabled = () => repositoryBusy() || !githubConnected() || !canManageRepositories();
  const available = createMemo(() => {
    const included = new Set(cloud.repos.projects.map((p) => identity(p.repositoryIdentity)));
    return (cloud.repos.github?.repositories ?? []).filter((repo) => !included.has(repo.nameWithOwner.toLowerCase()));
  });
  const shown = createMemo(() => {
    const needle = query().trim().toLowerCase();
    if (!needle) return available();
    return available().filter((repo) => repo.nameWithOwner.toLowerCase().includes(needle) || (repo.description ?? "").toLowerCase().includes(needle));
  });
  const close = (refocus = true) => {
    setOpen(false);
    setSelected([]);
    setQuery("");
    if (refocus) trigger?.focus();
  };
  const toggle = () => {
    if (disabled() && !open()) return;
    if (open()) return close();
    setSelected([]);
    setRoom(props.iconOnly && trigger ? Math.max(240, Math.floor(trigger.getBoundingClientRect().right) - 8) : null);
    setOpen(true);
    void loadRepositories();
    queueMicrotask(() => search?.focus());
  };
  const pick = (name: string) => setSelected((list) => (list.includes(name) ? list.filter((n) => n !== name) : [...list, name]));
  const add = () => {
    const names = new Set(selected());
    const repos: GithubRepository[] = (cloud.repos.github?.repositories ?? []).filter((repo) => names.has(repo.nameWithOwner));
    if (!repos.length || repositoryBusy() || !canManageRepositories()) return;
    close();
    void connectRepositories(repos);
  };
  const outside = (event: PointerEvent) => {
    if (open() && root && !root.contains(event.target as Node)) close(false);
  };
  document.addEventListener("pointerdown", outside);
  onCleanup(() => document.removeEventListener("pointerdown", outside));
  return (
    <div class="pc-picker" ref={root} onKeyDown={(event) => event.key === "Escape" && open() && (event.stopPropagation(), close())}>
      <button
        ref={trigger}
        type="button"
        class="pc-action"
        data-icon-only={props.iconOnly ? "true" : undefined}
        aria-label="Add repository"
        title={props.iconOnly ? "Add repository" : undefined}
        aria-haspopup="dialog"
        aria-expanded={open()}
        disabled={disabled()}
        onClick={toggle}
      >
        <span class="pc-action-label">
          <Icon name="plus" size={14} />
          <Show when={!props.iconOnly}>Repository</Show>
        </span>
      </button>
      <Show when={open()}>
        <div class="pc-picker-panel" role="dialog" aria-label="Repositories" style={{ "max-width": room() ? `${room()}px` : undefined }}>
          <div class="pc-picker-search">
            <input ref={search} class="field" type="search" placeholder="Search repositories…" aria-label="Search repositories" value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
            <IconButton icon="refresh" label="Refresh GitHub repositories" disabled={repositoryBusy()} busy={repositoryBusy()} onClick={() => void loadRepositories()} />
          </div>
          <div class="pc-picker-list" role="listbox" aria-multiselectable="true" aria-label="Available repositories">
            <For
              each={shown()}
              fallback={
                <div class="pc-picker-empty">{available().length === 0 ? "Every available repository is already included." : "No repositories match this search."}</div>
              }
            >
              {(repo) => (
                <button
                  type="button"
                  role="option"
                  class="pc-picker-choice"
                  aria-label={repo.isPrivate ? `${repo.nameWithOwner}, private` : repo.nameWithOwner}
                  aria-selected={selected().includes(repo.nameWithOwner)}
                  onClick={() => pick(repo.nameWithOwner)}
                >
                  <Avatar name={repo.nameWithOwner} url={repo.ownerAvatarUrl} size={24} />
                  <span class="pc-truncate pc-grow">{repo.nameWithOwner}</span>
                  <Show when={repo.isPrivate}>
                    <Icon name="dither-lock" size={12} label="Private" />
                  </Show>
                  <Show when={selected().includes(repo.nameWithOwner)}>
                    <Icon name="check" size={14} class="pc-ok-text" />
                  </Show>
                </button>
              )}
            </For>
          </div>
          <div class="pc-picker-foot">
            <span class="pc-muted">{selected().length === 0 ? `${available().length} available` : `${selected().length} selected`}</span>
            <Action primary disabled={selected().length === 0 || repositoryBusy() || !canManageRepositories()} onClick={add}>
              {selected().length === 0 ? "Add" : `Add ${selected().length}`}
            </Action>
          </div>
        </div>
      </Show>
    </div>
  );
}

function GithubGroup() {
  const loading = () => cloud.repos.githubLoading;
  const configured = () => cloud.repos.github?.configured !== false;
  const refresh = (label: string) => (
    <Action icon="refresh" busy={loading()} onClick={() => void loadRepositories()}>
      {label}
    </Action>
  );
  return (
    <Group title="GitHub" help="Connect GitHub once to choose repositories and let cloud agents commit, push, and open pull requests as you.">
      <Show
        when={configured()}
        fallback={<Row title="GitHub App is not configured" description="Add the GitHub App credentials to the API, deploy it, then retry." action={<Badge tone="error">Unavailable</Badge>} />}
      >
        <Show when={(cloud.repos.github?.installations.length ?? 0) > 0} fallback={<Row title="Connect repositories" action={refresh("Check connection")} />}>
          <For each={cloud.repos.github?.installations ?? []}>
            {(install) => (
              <Row
                title={install.accountLogin}
                description={`${install.accountType} · ${install.repositorySelection === "selected" ? "selected repositories" : "all repositories"}`}
                action={
                  install.suspended ? (
                    <Badge tone="warn">Suspended</Badge>
                  ) : !cloud.repos.github?.user ? (
                    <Badge tone="warn">Install GitHub App</Badge>
                  ) : (
                    <Badge icon="check">Connected</Badge>
                  )
                }
              >
                <div class="pc-install-meta">
                  <Avatar name={install.accountLogin} url={install.avatarUrl} size={20} />
                  <span class="pc-row-description">{cloud.repos.github?.repositories.length ?? 0} repositories available across connected installations.</span>
                </div>
              </Row>
            )}
          </For>
          <Show when={githubConnected()}>
            <Row
              title={cloud.repos.github?.user ? `Connected as ${cloud.repos.github.user.login}` : "Connection status"}
              description="Repository changes refresh automatically when you return from GitHub. Existing cloud chats are not affected."
              action={refresh("Refresh")}
            />
          </Show>
        </Show>
      </Show>
    </Group>
  );
}

function imageStatus(state: string) {
  switch (state) {
    case "ready":
      return <Badge>Ready</Badge>;
    case "failed":
      return <Badge tone="error">Failed</Badge>;
    case "auth-broken":
      return <Badge tone="error">Authentication</Badge>;
    case "building":
      return <Badge tone="warn">Building</Badge>;
    case "outdated":
      return <Badge tone="warn">Unbuilt changes</Badge>;
    default:
      return <Badge tone="warn">Not built</Badge>;
  }
}

function ImageGroup() {
  const [history, setHistory] = createSignal(false);
  const image = () => cloud.image.value;
  const state = () => image()?.state ?? "unknown";
  const starting = () => cloud.image.building;
  const building = () => starting() || state() === "building";
  const enabled = () =>
    !cloud.image.loading && !cloud.image.deleting && !building() && state() !== "unknown" && state() !== "auth-broken" && canManageRepositories() && cloud.repos.projects.length > 0;
  const changed = () => {
    const value = image();
    const active = value?.builds.find((build) => build.active);
    const projects = cloud.repos.projects;
    if (!value || !active) return "Changed: runtime or toolchain.";
    const repos =
      active.repositories.length !== projects.length ||
      projects.some((p) => !active.repositories.some((r) => r.projectId === p.projectId) || (value.builtAt != null && p.updatedAt > value.builtAt));
    return repos ? "Changed: repositories." : "Changed: runtime or toolchain.";
  };
  const notice = () => {
    if (building()) {
      if (starting()) return { title: "Building cloud image", description: "Starting the image build…" };
      const phase = image()?.progressPhase;
      return { title: "Building cloud image", description: phase === "queued" ? "Waiting for a build worker…" : phase === "sanitizing" ? "Preparing the snapshot…" : "Preparing repositories and agents" };
    }
    const count = cloud.repos.projects.length;
    switch (state()) {
      case "not-built":
        return {
          title: "Cloud image required",
          description: count === 0 ? "Add a repository before building your first cloud image." : `${count} ${count === 1 ? "repository is" : "repositories are"} ready to include.`,
        };
      case "failed":
        return { title: "Cloud image build failed", description: `The previous image remains available. ${image()?.errorCode ?? "Open the latest build for details."}` };
      case "auth-broken":
        return { title: "Rebuild required", description: "Reconnect the affected agent. Image status will refresh automatically and show whether a rebuild is needed." };
      default:
        return { title: "Unbuilt changes", description: changed() };
    }
  };
  const label = () => (buildRetry() ? "Retry request" : ({ "not-built": "Build image", "auth-broken": "Rebuild image", failed: "Retry build" } as Record<string, string>)[state()] ?? "Update image");
  const failed = () => state() === "failed" || state() === "auth-broken";
  return (
    <Group title="Cloud image" help="Build an image for the selected provider. Existing chats keep their current environment.">
      <Show
        when={image()}
        fallback={<Row title={cloud.image.error ? "Image status unavailable" : "Checking cloud image…"} description="Checking whether this provider needs a build." />}
      >
        <Show
          when={state() === "ready" && !starting()}
          fallback={
            <div class="pc-image-alert" data-tone={failed() ? "error" : "warn"}>
              <Show when={building()} fallback={<Icon name={failed() ? "close-circle" : "danger-triangle"} size={16} class="pc-image-alert-icon" />}>
                <GlyphSpinner box={16} decorative />
              </Show>
              <div class="pc-row-copy">
                <div class="pc-row-title">{notice().title}</div>
                <div class="pc-row-description">{notice().description}</div>
              </div>
              <Show when={!building()}>
                <Show when={state() === "auth-broken"}>
                  <Badge tone="error">Authentication</Badge>
                </Show>
                <Action primary icon="refresh" disabled={!enabled()} busy={starting()} onClick={() => void buildImage(state() === "auth-broken" ? "rebuild" : "update")}>
                  {label()}
                </Action>
              </Show>
            </div>
          }
        >
          <Row
            title="Cloud image ready"
            description="Ready for new cloud chats. Agent authentication is delivered when a workspace starts."
            action={
              <>
                <Badge>Ready</Badge>
                <Action disabled={!enabled()} onClick={() => void buildImage("rebuild")}>
                  Rebuild
                </Action>
              </>
            }
          />
        </Show>
      </Show>
      <div class="pc-provider">
        <div class="pc-provider-id">
          <span class="pc-provider-radio" aria-hidden="true" />
          <Icon name="cloud-boxd" size={16} />
          <span class="pc-row-title">boxd</span>
          <Badge tone="outline">Recommended</Badge>
          <span class="pc-row-description">Billed by Zuse</span>
        </div>
        <button type="button" class="pc-provider-logs" aria-label="View boxd build logs" onClick={() => setHistory(true)}>
          <span class="pc-row-description">Zuse image</span>
          {imageStatus(state())}
          <span class="pc-row-description">Logs</span>
          <Icon name="alt-arrow-right" size={14} />
        </button>
      </div>
      <Show when={cloud.image.error}>
        {(error) => (
          <div class="pc-inline-alert" role="alert">
            <span>{error()}</span>
            <Action disabled={cloud.image.loading} onClick={() => void loadImage()}>
              Retry
            </Action>
          </div>
        )}
      </Show>
      <BuildHistory open={history()} onClose={() => setHistory(false)} />
    </Group>
  );
}

const PHASES: Record<string, string> = {
  allocate: "Starting the image builder…",
  prepare: "Preparing repositories and agents…",
  capture: "Preparing the snapshot…",
  sanitizing: "Preparing the snapshot…",
  verify: "Verifying the saved image…",
  cleanup: "Cleaning up temporary machines…",
  promote: "Making the image available…",
  "retire-images": "Removing the superseded image…",
};
const running = (state: string) => ["queued", "building", "sanitizing"].includes(state);
function duration(build: ImageBuild) {
  const seconds = Math.max(0, Math.floor((build.updatedAt - build.createdAt) / 1000));
  return seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`;
}
function logs(build: ImageBuild) {
  if (build.logText) return build.logText;
  if (build.state === "failed") return `Logs were not retained for this older build.\nError: ${build.errorCode ?? "unknown"}`;
  if (running(build.state)) return PHASES[build.progressPhase ?? ""] ?? "Waiting for the image builder…";
  return "Build completed. No output was retained.";
}

function BuildHistory(props: { open: boolean; onClose: () => void }) {
  const [previous, setPrevious] = createSignal(false);
  const [page, setPage] = createSignal(0);
  const builds = () => cloud.image.value?.builds ?? [];
  const pages = () => Math.max(1, Math.ceil((builds().length - 1) / 5));
  const current = () => Math.min(page(), pages() - 1);
  return (
    <Show when={props.open}>
      <Dialog open onOpenChange={(next) => !next && props.onClose()} title="View boxd build logs">
        <div class="pc-builds">
          <Show when={builds()[0]} fallback={<p class="pc-muted">No image builds yet.</p>}>
            {(first) => <Build build={first()} first initiallyOpen />}
          </Show>
          <Show when={builds().length > 1}>
            <button type="button" class="pc-builds-previous" aria-expanded={previous()} onClick={() => setPrevious(!previous())}>
              <Icon name={previous() ? "alt-arrow-up" : "alt-arrow-down"} size={12} />
              Previous builds {builds().length - 1}
            </button>
            <Show when={previous()}>
              <For each={builds().slice(1 + current() * 5, 6 + current() * 5)}>{(build) => <Build build={build} />}</For>
              <Show when={pages() > 1}>
                <div class="pc-builds-pages">
                  <span class="pc-muted">
                    Page {current() + 1} of {pages()}
                  </span>
                  <span class="pc-grow" />
                  <Action disabled={current() === 0} onClick={() => setPage(current() - 1)}>
                    Previous
                  </Action>
                  <Action disabled={current() + 1 >= pages()} onClick={() => setPage(current() + 1)}>
                    Next
                  </Action>
                </div>
              </Show>
            </Show>
          </Show>
        </div>
        <div class="dialog-footer">
          <Action onClick={() => props.onClose()}>Close</Action>
        </div>
      </Dialog>
    </Show>
  );
}

function Build(props: { build: ImageBuild; first?: boolean; initiallyOpen?: boolean }) {
  const [open, setOpen] = createSignal(!!props.initiallyOpen);
  const [settings, setSettings] = createSignal(false);
  const title = () => (props.build.mode === "rebuild" ? "Clean rebuild" : "Image update");
  const copy = async () => {
    if (await writeClipboard(logs(props.build))) toast("Build logs copied");
    else toast("Could not copy the build logs.", "danger");
  };
  return (
    <div class="pc-build" data-first={props.first ? "true" : undefined}>
      <button type="button" class="pc-build-head" aria-expanded={open()} onClick={() => setOpen(!open())}>
        <Show when={running(props.build.state)} fallback={<Icon name={props.build.state === "ready" ? "check-circle" : "close-circle"} size={14} />}>
          <GlyphSpinner decorative />
        </Show>
        <span class="pc-row-title pc-truncate">{title()}</span>
        <span class="pc-build-when">
          {new Date(props.build.createdAt).toLocaleString()} · {duration(props.build)}
        </span>
        <span class="pc-grow" />
        <Show when={props.build.active}>
          <Badge>Active</Badge>
        </Show>
        {props.build.state === "ready" ? <Badge>Succeeded</Badge> : props.build.state === "failed" ? <Badge tone="error">Failed</Badge> : <Badge tone="warn">In progress</Badge>}
        <Icon name={open() ? "alt-arrow-up" : "alt-arrow-down"} size={12} />
      </button>
      <Show when={open()}>
        <div class="pc-build-body">
          <div class="pc-build-log">
            <pre>{logs(props.build)}</pre>
            <IconButton icon="copy" label="Copy build logs" size="sm" class="pc-build-copy" onClick={() => void copy()} />
          </div>
          <button type="button" class="pc-link-button pc-build-settings" aria-expanded={settings()} onClick={() => setSettings(!settings())}>
            <Icon name={settings() ? "alt-arrow-down" : "alt-arrow-right"} size={12} />
            Build settings
          </button>
          <Show when={settings()}>
            <dl class="pc-info pc-build-info">
              <dt>Runtime</dt>
              <dd>{props.build.runtimeVersion}</dd>
            </dl>
            <For each={props.build.repositories}>
              {(repository) => (
                <div class="pc-build-repo">
                  <Avatar name={repository.displayName} size={24} />
                  <span class="pc-row-description pc-grow pc-truncate">{repository.displayName}</span>
                  <span class="pc-build-when">{repository.defaultBranch}</span>
                </div>
              )}
            </For>
          </Show>
        </div>
      </Show>
    </div>
  );
}

/** Usage tab: a retained image keeps billing storage until deleted. */
export function ImageStorage() {
  const [confirm, setConfirm] = createSignal<string | null>(null);
  onMount(() => {
    if (!cloud.repos.loaded) void loadRepositories();
    else void loadImage();
  });
  const storage = () => cloud.image.value?.storage ?? null;
  return (
    <Show when={storage()}>
      {(value) => {
        const deleting = () => value().state === "deleting";
        const enabled = () => !cloud.image.loading && !cloud.image.building && !cloud.image.deleting && cloud.image.value?.state !== "building" && !deleting() && canManageRepositories();
        return (
          <Group title="Image storage" help={`${usd(value().monthlyCostMicros)} per 30 days, prorated while retained.`}>
            <Row title="Retained images" description="Storage uses your $35 cloud allowance. Additional usage has the existing 5% markup and overage cap." action={<Badge tone="outline">1</Badge>} />
            <Row
              title="boxd"
              description={
                deleting()
                  ? "Image deletion pending. Existing workspaces are preserved."
                  : value().billingEnabled
                    ? "Deleting this image requires a rebuild for future workspaces. Existing workspace disks and authentication are preserved."
                    : "Storage billing has not started. Deleting this image requires a rebuild for future workspaces."
              }
              action={
                <Action disabled={!enabled()} busy={cloud.image.deleting} onClick={() => setConfirm(value().snapshotId)}>
                  Delete image
                </Action>
              }
            />
            <Row
              title="Storage usage this period"
              description="Included in your cloud usage totals."
              action={<Badge tone="outline">{cloud.billing.summary ? usd(cloud.billing.summary.storageCostMicros) : "—"}</Badge>}
            />
            <Show when={value().lastError}>{(error) => <Note error>Cleanup will retry: {error()}</Note>}</Show>
            <Show when={cloud.image.error}>
              {(error) => (
                <div class="pc-inline-alert" role="alert">
                  <span>{error()}</span>
                  <Action disabled={cloud.image.loading} onClick={() => void loadImage()}>
                    Retry
                  </Action>
                </div>
              )}
            </Show>
            <Dialog
              open={confirm() !== null}
              onOpenChange={(next) => !next && setConfirm(null)}
              title="Delete saved image?"
              description="This removes the saved cloud image and stops its storage charges. Existing workspaces, conversations and files are preserved. Build another image before starting new workspaces."
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
                      if (id) void deleteImage(id);
                    }}
                  >
                    Delete image
                  </button>
                </>
              }
            />
          </Group>
        );
      }}
    </Show>
  );
}
