// New project: actions and existing projects, then the create and clone forms, as on desktop.
import { createEffect, createMemo, Match, on, onCleanup, onMount, Show, Switch } from "solid-js";
import { state } from "@/core/store";
import type { Device } from "@/core/protocol";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { CreationBadge } from "./CreationNotice";
import { creationAt } from "./creations";
import { FolderBrowser } from "./FolderBrowser";
import { deviceGlyph, deviceLabel, spaceName } from "./hosts";
import { GroupLabel, Highlight, KeyHint, KeyCaps, Radio, Row, VirtualList } from "./Palette";
import { repositoryLabel } from "./paths";
import {
  ACTIONS,
  type ActionRow,
  activateFormRow,
  actionRows,
  back,
  canSubmit,
  closeSetup,
  cloneUrl,
  destination,
  endSetup,
  type FormRow,
  formGroup,
  formRows,
  githubReady,
  loadGithub,
  pickAction,
  primary,
  searching,
  setName,
  setQuery,
  setSetup,
  setup,
  startSetup,
  TEMPLATES,
} from "./setup";

export function NewProject() {
  onMount(() => {
    if (!setup.started) startSetup();
  });
  onCleanup(endSetup);
  return (
    <div
      class="pj-route"
      onPointerDown={(event) => {
        // Pressing outside the card dismisses it, as desktop's palette does.
        if (event.target === event.currentTarget) closeSetup();
      }}
    >
      <Show when={setup.browser} fallback={<SetupCard />}>
        <FolderBrowser />
      </Show>
    </div>
  );
}

type Entry =
  | { kind: "label"; text: string; status?: string | null }
  | { kind: "summary" }
  | { kind: "error"; text: string }
  | { kind: "action"; row: ActionRow; index: number }
  | { kind: "form"; row: FormRow; index: number }
  | { kind: "note"; text: string }
  | { kind: "skeleton"; index: number };

const step = (from: number | null, count: number, delta: number) => {
  if (!count) return 0;
  if (from === null) return delta >= 0 ? 0 : count - 1;
  return (((from + delta) % count) + count) % count;
};

function SetupCard() {
  let input: HTMLInputElement | undefined;
  const choose = () => setup.mode === "choose";
  const confirm = () => setup.mode === "clone" && setup.cloneConfirm;
  const headerValue = () => (setup.mode === "create" || confirm() ? setup.name : setup.query);
  const placeholder = () =>
    setup.mode === "create"
      ? "Project name"
      : confirm()
        ? "Folder name"
        : setup.mode === "clone"
          ? "Search your repositories or paste a Git URL…"
          : "Find a project or action…";

  const actions = createMemo(actionRows);
  const rows = createMemo(formRows);

  const entries = createMemo<Entry[]>(() => {
    const out: Entry[] = [];
    if (choose()) {
      let group = "";
      actions().forEach((row, index) => {
        const heading = row.kind === "project" ? "Projects" : row.kind === "computer" ? "Computer" : "Start";
        if (heading !== group) out.push({ kind: "label", text: heading });
        group = heading;
        out.push({ kind: "action", row, index });
      });
      if (!out.length) out.push({ kind: "note", text: "Nothing matches" });
      return out;
    }
    if (!searching()) {
      out.push({ kind: "label", text: confirm() ? "Repository" : "Project" }, { kind: "summary" });
      if (setup.error) out.push({ kind: "error", text: setup.error });
    }
    let group: string | null = null;
    rows().forEach((row, index) => {
      const heading = formGroup(row);
      if (heading && heading !== group) out.push({ kind: "label", text: heading });
      group = heading;
      out.push({ kind: "form", row, index });
    });
    if (searching() && !rows().some((r) => r.kind === "repo" || r.kind === "url")) {
      if (setup.repositories.state === "loading") for (let i = 0; i < 4; i++) out.push({ kind: "skeleton", index: i });
      else out.push({ kind: "note", text: searchNote() });
      if (setup.error) out.push({ kind: "error", text: setup.error });
    }
    return out;
  });

  const activeEntry = createMemo(() => {
    const target = choose() ? setup.active : setup.navigating ? setup.active : -1;
    if (target < 0) return undefined;
    const at = entries().findIndex((e) => (e.kind === "action" || e.kind === "form") && e.index === target);
    return at >= 0 ? at : undefined;
  });

  createEffect(
    on(
      () => [setup.mode, setup.cloneConfirm] as const,
      () => queueMicrotask(() => input?.focus()),
    ),
  );

  const onKeyDown = (event: KeyboardEvent) => {
    let key = event.key;
    if (event.ctrlKey && key === "n") key = "ArrowDown";
    if (event.ctrlKey && key === "p") key = "ArrowUp";
    if (key === "Escape") {
      event.preventDefault();
      closeSetup();
      return;
    }
    if (key === "Backspace" && !choose() && headerValue() === "") {
      event.preventDefault();
      back();
      return;
    }
    const delta = key === "ArrowDown" ? 1 : key === "ArrowUp" ? -1 : 0;
    if (choose()) {
      const list = actions();
      if (delta) {
        event.preventDefault();
        setSetup("active", step(setup.active, list.length, delta));
      } else if (key === "Enter") {
        event.preventDefault();
        const row = list[setup.active];
        if (row) pickAction(row);
      }
      return;
    }
    const list = rows();
    if (delta) {
      event.preventDefault();
      setSetup({ active: step(setup.navigating ? setup.active : null, list.length, delta), navigating: list.length > 0 });
    } else if (key === "Enter") {
      event.preventDefault();
      const row = list[setup.active];
      if (row && setup.navigating && !(event.metaKey || event.ctrlKey)) activateFormRow(row);
      else primary();
    }
  };

  const onInput = (value: string) => {
    if (setup.mode === "create" || confirm()) setName(value);
    else setQuery(value);
  };

  return (
    <section class="pj-card" aria-label="New project" onKeyDown={(e) => e.target !== input && e.key === "Escape" && closeSetup()}>
      <div class="pj-header">
        <Show
          when={!choose()}
          fallback={
            <span class="pj-lead">
              <Icon name="magnifer" size={16} />
            </span>
          }
        >
          <button type="button" class="pj-lead pj-back" aria-label="Back" title="Back" onClick={() => back()}>
            <Icon name="arrow-left" size={16} />
          </button>
        </Show>
        <input
          ref={input}
          class="pj-input"
          value={headerValue()}
          placeholder={placeholder()}
          aria-label={placeholder()}
          role="combobox"
          aria-expanded="true"
          aria-controls="pj-setup-list"
          aria-activedescendant={activeEntry() !== undefined ? `pj-setup-${activeEntry()}` : undefined}
          autocomplete="off"
          spellcheck={false}
          onInput={(e) => onInput(e.currentTarget.value)}
          onKeyDown={onKeyDown}
        />
      </div>
      <div class="pj-panel">
        <VirtualList
          id="pj-setup-list"
          label={choose() ? "Projects and actions" : setup.mode === "create" ? "Project options" : "Repositories"}
          items={entries()}
          active={activeEntry()}
          estimate={(e) => (e.kind === "label" ? 26 : e.kind === "summary" ? 44 : e.kind === "note" ? 60 : 32)}
          itemKey={(e, i) => (e.kind === "action" || e.kind === "form" ? `${e.kind}-${e.index}-${rowKey(e)}` : `${e.kind}-${i}`)}
        >
          {(entry, index) => <EntryView entry={entry} id={`pj-setup-${index}`} />}
        </VirtualList>
      </div>
      <Show when={choose()} fallback={<FormFooter />}>
        <footer class="pj-footer">
          <span class="pj-hints">
            <KeyHint keys={["up", "down"]} label="Navigate" />
            <KeyHint keys={["enter"]} label="Open" />
          </span>
          <KeyHint keys={["Esc"]} label="Close" />
        </footer>
      </Show>
    </section>
  );
}

function rowKey(entry: Entry): string {
  if (entry.kind === "action") {
    const row = entry.row;
    return row.kind === "project" ? row.space.id : row.kind === "computer" ? row.device.id : row.kind;
  }
  if (entry.kind === "form") {
    const row = entry.row;
    switch (row.kind) {
      case "template":
        return row.template;
      case "computer":
        return row.device.id;
      case "repo":
        return row.repo.nameWithOwner;
      case "url":
        return "url";
      case "more":
        return `more-${row.page}`;
      default:
        return row.kind;
    }
  }
  return "";
}

function searchNote(): string {
  const repos = setup.repositories;
  if (repos.state === "error") return repos.message;
  const github = setup.github;
  if (github.state === "loading") return "Checking GitHub on this computer…";
  if (github.state === "ready") {
    if (!github.value.installed) return "Install the GitHub CLI to browse your repositories, or paste a Git URL.";
    if (!github.value.authenticated) return "Run gh auth login to browse your repositories, or paste a Git URL.";
    return "No repositories match. Paste a Git URL to clone anything else.";
  }
  return "GitHub is unavailable here. Paste a Git URL to clone.";
}

function EntryView(props: { entry: Entry; id: string }) {
  return (
    <Switch>
      <Match when={props.entry.kind === "label" && props.entry}>{(e) => <GroupLabel status={e().status}>{e().text}</GroupLabel>}</Match>
      <Match when={props.entry.kind === "summary"}>
        <Summary />
      </Match>
      <Match when={props.entry.kind === "error" && props.entry}>
        {(e) => (
          <div class="pj-error" role="alert">
            <Icon name="danger-triangle" size={13} />
            <span>{e().text}</span>
          </div>
        )}
      </Match>
      <Match when={props.entry.kind === "note" && props.entry}>{(e) => <div class="pj-note">{e().text}</div>}</Match>
      <Match when={props.entry.kind === "skeleton"}>
        <div class="pj-skeleton" aria-hidden="true">
          <span />
          <span />
        </div>
      </Match>
      <Match when={props.entry.kind === "action" && props.entry}>{(e) => <ActionRowView row={e().row} index={e().index} id={props.id} />}</Match>
      <Match when={props.entry.kind === "form" && props.entry}>{(e) => <FormRowView row={e().row} index={e().index} id={props.id} />}</Match>
    </Switch>
  );
}

function ActionRowView(props: { row: ActionRow; index: number; id: string }) {
  const highlighted = () => setup.active === props.index;
  const hover = () => setup.active !== props.index && setSetup("active", props.index);
  return (
    <Switch>
      <Match when={props.row.kind === "project" && props.row}>
        {(row) => {
          const op = () => creationAt(row().space.deviceId, row().space.path);
          return (
            <div class="pj-row-wrap">
              <Row
                id={props.id}
                glyph="project-default"
                highlighted={highlighted()}
                onPointerMove={hover}
                onClick={() => pickAction(row())}
                aria-label={`Open ${spaceName(row().space)}`}
              >
                <span class="pj-title pj-title-fixed">
                  <Highlight text={spaceName(row().space)} query={setup.query} />
                </span>
                <span class="pj-detail">{row().space.path}</span>
                <Show when={op()}>{(o) => <CreationBadge op={o()} />}</Show>
              </Row>
              <Show when={op()}>{(o) => <CreationBadge op={o()} action />}</Show>
            </div>
          );
        }}
      </Match>
      <Match when={props.row.kind === "computer" && (props.row as { device: Device }).device}>
        {(device) => (
          <Row
            id={props.id}
            glyph={deviceGlyph(device().platform)}
            highlighted={highlighted()}
            onPointerMove={hover}
            onClick={() => pickAction(props.row)}
            aria-checked={setup.deviceId === device().id}
          >
            <span class="pj-title pj-grow">{deviceLabel(device().id)}</span>
            <Radio on={setup.deviceId === device().id} />
          </Row>
        )}
      </Match>
      <Match when={props.row.kind !== "project" && props.row.kind !== "computer" && ACTIONS[props.row.kind as keyof typeof ACTIONS]}>
        {(action) => (
          <Row id={props.id} glyph={action().glyph} highlighted={highlighted()} onPointerMove={hover} onClick={() => pickAction(props.row)}>
            <span class="pj-title pj-title-fixed">
              <Highlight text={action().title} query={setup.query} />
            </span>
            <span class="pj-detail">{action().hint}</span>
          </Row>
        )}
      </Match>
    </Switch>
  );
}

function FormRowView(props: { row: FormRow; index: number; id: string }) {
  const highlighted = () => setup.navigating && setup.active === props.index;
  const hover = () => {
    if (setup.active !== props.index || !setup.navigating) setSetup({ active: props.index, navigating: true });
  };
  const common = () => ({ id: props.id, highlighted: highlighted(), onPointerMove: hover, onClick: () => activateFormRow(props.row) });
  return (
    <Switch>
      <Match when={props.row.kind === "template" && TEMPLATES.find((t) => t.template === (props.row as { template: string }).template)}>
        {(t) => (
          <Row {...common()} glyph={t().glyph} aria-checked={setup.template === t().template}>
            <span class="pj-title pj-title-fixed">{t().label}</span>
            <span class="pj-detail">{t().hint}</span>
            <Radio on={setup.template === t().template} />
          </Row>
        )}
      </Match>
      <Match when={props.row.kind === "publish"}>
        <Row {...common()} glyph="site-github" aria-checked={setup.publish && githubReady()} aria-disabled={!githubReady()}>
          <span class="pj-title pj-title-fixed" data-muted={githubReady() ? undefined : "true"}>
            Create private GitHub repository
          </span>
          <span class="pj-detail">{publishDetail()}</span>
          <span class="toggle" aria-hidden="true" data-on={setup.publish && githubReady() ? "true" : undefined} data-disabled={githubReady() ? undefined : "true"} />
        </Row>
      </Match>
      <Match when={props.row.kind === "location"}>
        <Row {...common()} glyph="folder" aria-label={`Location ${setup.parent || "not chosen"}. Change`}>
          <span class="pj-title pj-grow">{setup.parent.trim() || "Choose a folder"}</span>
          <span class="pj-trailing">Change</span>
        </Row>
      </Match>
      <Match when={props.row.kind === "computer" && (props.row as { device: Device }).device}>
        {(device) => (
          <Row {...common()} glyph={deviceGlyph(device().platform)} aria-checked={setup.deviceId === device().id}>
            <span class="pj-title pj-grow">{deviceLabel(device().id)}</span>
            <Radio on={setup.deviceId === device().id} />
          </Row>
        )}
      </Match>
      <Match when={props.row.kind === "repo" && (props.row as { repo: { nameWithOwner: string; private: boolean } }).repo}>
        {(repo) => {
          const parts = () => {
            const [owner, name] = repo().nameWithOwner.includes("/") ? repo().nameWithOwner.split("/", 2) : ["", repo().nameWithOwner];
            return { owner: owner ?? "", name: name ?? "" };
          };
          return (
            <Row {...common()} glyph="site-github" aria-label={`${repo().nameWithOwner}${repo().private ? ", private" : ""}`}>
              <span class="pj-title pj-title-fixed">
                <Highlight text={parts().name} query={setup.query} />
              </span>
              <span class="pj-detail">{parts().owner}</span>
              <Show when={repo().private}>
                <span class="pj-trailing">Private</span>
              </Show>
            </Row>
          );
        }}
      </Match>
      <Match when={props.row.kind === "url" && (props.row as { url: string }).url}>
        {(url) => (
          <Row {...common()} glyph="global">
            <span class="pj-title pj-title-fixed">Clone from URL</span>
            <span class="pj-detail">{url()}</span>
          </Row>
        )}
      </Match>
      <Match when={props.row.kind === "more"}>
        <Row {...common()} glyph="refresh">
          <span class="pj-title pj-grow" data-muted="true">
            Load more repositories
          </span>
        </Row>
      </Match>
    </Switch>
  );
}

function publishDetail(): string {
  if (githubReady()) return "Pushes the first commit";
  const github = setup.github;
  if (github.state === "loading") return "Checking GitHub…";
  if (github.state === "ready") return github.value.installed ? "Run gh auth login to enable" : "Install the GitHub CLI to enable";
  return "GitHub is unavailable here";
}

function Summary() {
  const confirm = () => setup.mode === "clone" && setup.cloneConfirm;
  const onDevice = () => (setup.deviceId && setup.deviceId !== state.localDeviceId ? ` on ${deviceLabel(setup.deviceId)}` : "");
  const glyph = () => (confirm() ? (cloneUrl().includes("github.com") ? "site-github" : "global") : "folder-add");
  const title = () => (confirm() ? repositoryLabel(cloneUrl()) : setup.name.trim() || "New project");
  const detail = () => {
    const path = destination();
    if (confirm()) return path ? `Clones into ${path}${onDevice()}` : "Name the folder to clone into";
    if (path) return `Creates ${path}${onDevice()}`;
    return setup.parent.trim() ? `Goes in ${setup.parent.trim()}${onDevice()}` : "Choose a location below";
  };
  return (
    <div class="pj-summary">
      <Icon name={glyph()} size={16} class="pj-glyph" />
      <div class="pj-summary-text">
        <span class="pj-summary-title">{title()}</span>
        <span class="pj-summary-detail">{detail()}</span>
      </div>
    </div>
  );
}

function FormFooter() {
  const highlighted = () => (setup.navigating ? formRows()[setup.active] : undefined);
  const enterLabel = () => {
    const row = highlighted();
    if (row?.kind === "publish") return "Toggle";
    if (row?.kind === "location") return "Change";
    if (row?.kind === "more") return "Load more";
    if (row) return "Select";
    if (searching()) return "Continue";
    return setup.mode === "clone" ? "Clone" : "Create";
  };
  const action = () => (setup.busy ? "Starting…" : searching() ? "Continue" : setup.mode === "clone" ? "Clone" : "Create");
  return (
    <footer class="pj-footer">
      <span class="pj-hints">
        <KeyHint keys={["up", "down"]} label="Navigate" />
        <KeyHint keys={["enter"]} label={enterLabel()} />
        <KeyHint keys={["⌫"]} label="Back" />
      </span>
      <span class="pj-footer-end">
        <Show when={setup.repositories.state === "error" && searching()}>
          <button type="button" class="pj-text-btn" onClick={() => void loadGithub(1)}>
            Retry
          </button>
        </Show>
        <button type="button" class="pj-submit" disabled={!canSubmit()} aria-label={action()} onClick={() => primary()}>
          <Show when={setup.busy}>
            <GlyphSpinner label="Starting" cell={1.75} box={12} />
          </Show>
          {action()}
          <KeyCaps keys={searching() ? ["enter"] : ["mod", "enter"]} />
        </button>
      </span>
    </footer>
  );
}

