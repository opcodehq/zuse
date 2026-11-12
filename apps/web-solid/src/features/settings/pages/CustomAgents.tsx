// ACP agents: custom agents launched over the Agent Client Protocol, added from the public catalog or by hand
// (crates/viewport/src/acp_rows.rs, acp_catalog.rs, acp_command.rs).
import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { state } from "@/core/store";
import { Icon } from "@/ui/Icon";
import { Menu } from "@/ui/primitives";
import { type CustomAcpAgent, data, type RegistryAgent } from "../data";
import { descriptorEnabled, type HarnessDescriptor, harnessMark, harnessTint, providerName } from "../harness";
import { t } from "../i18n";
import { custom, definition, duplicateAgent, fieldsFor, loadCatalog, matches, offer, probe, removeAgent, saveAgent, type AgentFields } from "../custom";
import { effectiveEnabled, setEnabled } from "../providers";
import { ActionButton, Badge, Card, ErrorStrip, Group, InlineSpinner, Mark, SettingsDialog, Switch } from "../widgets";

type Form = { agent: CustomAcpAgent | null; existing: boolean };

export function CustomAgents() {
  const [catalogOpen, setCatalogOpen] = createSignal(false);
  const [form, setForm] = createSignal<Form | null>(null);
  const [removing, setRemoving] = createSignal<CustomAcpAgent | null>(null);
  const loaded = () => !!data.custom;
  const blocked = () => !!custom.busy || state.connection.kind !== "connected";

  const openCatalog = () => {
    setCatalogOpen(true);
    void loadCatalog();
  };

  return (
    <Group
      title={t("settings-acp_agents_title", "ACP agents")}
      action={
        <ActionButton icon="plus" disabled={blocked() || !loaded()} onClick={openCatalog}>
          {t("common-add", "Add")}
        </ActionButton>
      }
    >
      <Card>
        <Show
          when={loaded()}
          fallback={
            <Show when={data.customError} fallback={<div class="st-empty">Loading custom agents…</div>}>
              <div class="st-empty">{data.customError}</div>
            </Show>
          }
        >
          <For each={data.custom?.agents ?? []} fallback={<div class="st-empty">{t("settings-acp_agents_empty", "No ACP agents yet")}</div>}>
            {(agent) => (
              <AgentRow
                agent={agent}
                blocked={blocked()}
                onEdit={() => setForm({ agent, existing: true })}
                onRemove={() => setRemoving(agent)}
              />
            )}
          </For>
        </Show>
        <Show when={custom.busy}>
          <div class="st-row st-desc" role="status">
            <InlineSpinner label={custom.busy!} />
            {custom.busy}
          </div>
        </Show>
      </Card>
      <Show when={custom.error && !form()}>{(text) => <ErrorStrip>{text()}</ErrorStrip>}</Show>
      <Catalog
        open={catalogOpen()}
        onClose={() => setCatalogOpen(false)}
        onCustom={(agent) => {
          setCatalogOpen(false);
          setForm({ agent, existing: false });
        }}
      />
      <AgentForm form={form()} onClose={() => setForm(null)} />
      <SettingsDialog
        open={!!removing()}
        onClose={() => setRemoving(null)}
        title={`Remove ${removing()?.name ?? "agent"}?`}
        description="Zuse stops offering this agent. Existing chats keep their transcripts."
        footer={
          <>
            <ActionButton tone="quiet" onClick={() => setRemoving(null)}>
              Cancel
            </ActionButton>
            <ActionButton
              tone="danger"
              onClick={() => {
                const agent = removing();
                setRemoving(null);
                if (agent) removeAgent(agent.id);
              }}
            >
              Remove
            </ActionButton>
          </>
        }
      />
    </Group>
  );
}

function AgentRow(props: { agent: CustomAcpAgent; blocked: boolean; onEdit: () => void; onRemove: () => void }) {
  const runtime = createMemo(() => (state.harnesses as unknown as HarnessDescriptor[]).find((h) => h.id === props.agent.id));
  const enabled = () => {
    const d = runtime();
    return d ? effectiveEnabled(d) : false;
  };
  const installed = () => runtime()?.installed !== false;
  const interactive = () => !props.blocked && !!runtime() && (enabled() || installed());
  const probeState = () => custom.probes[props.agent.id];
  const status = (): [string | undefined, string] => {
    if (!runtime()) return [undefined, probeState()?.kind === "testing" ? "Setting up…" : "Connection not tested"];
    if (!enabled()) return [undefined, "Disabled"];
    const p = probeState();
    if (p?.kind === "testing") return [undefined, "Testing connection…"];
    if (p?.kind === "ready") return ["ready", "Ready"];
    if (p?.kind === "error") return ["error", p.message];
    return [undefined, "Connection not tested"];
  };
  return (
    <div class="st-provider-head">
      <Mark name="bot" dim={!enabled()} />
      <div class="st-row-text" style={{ opacity: enabled() ? undefined : 0.6, "min-width": 0 }}>
        <span class="st-row-title" style={{ overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
          {props.agent.name}
        </span>
        <span class="st-status">
          <Show when={probeState()?.kind === "testing"} fallback={<span class="st-dot" data-tone={status()[0]} aria-hidden="true" />}>
            <InlineSpinner label="Testing connection" />
          </Show>
          <span title={status()[1]}>{status()[1]}</span>
        </span>
      </div>
      <Switch label={`Enable ${props.agent.name}`} checked={enabled()} disabled={!interactive()} onChange={(value) => void setEnabled(props.agent.id, value)} />
      <Menu
        placement="bottom-end"
        items={[
          { label: t("settings-acp_agents_test_connection", "Test connection"), icon: "refresh", disabled: props.blocked || probeState()?.kind === "testing", onSelect: () => void probe(props.agent.id) },
          { label: "Edit", icon: "pen", disabled: props.blocked, onSelect: props.onEdit },
          { label: t("settings-acp_agents_duplicate", "Duplicate"), icon: "copy", disabled: props.blocked, onSelect: () => duplicateAgent(props.agent) },
          { separator: true, label: "" },
          { label: "Remove", icon: "trash-bin-minimalistic", danger: true, disabled: props.blocked, onSelect: props.onRemove },
        ]}
        trigger={(triggerProps) => (
          <button {...triggerProps} type="button" class="st-icon-action" data-size="md" aria-label={`Actions for ${props.agent.name}`}>
            <Icon name="more-horizontal" size={16} />
          </button>
        )}
      />
    </div>
  );
}

function Catalog(props: { open: boolean; onClose: () => void; onCustom: (agent: CustomAcpAgent | null) => void }) {
  const [query, setQuery] = createSignal("");
  const installed = () => new Set((data.custom?.agents ?? []).map((a) => a.id));
  const entries = createMemo(() => (custom.catalog?.agents ?? []).filter((entry) => matches(entry, query())));
  return (
    <SettingsDialog
      open={props.open}
      onClose={props.onClose}
      wide
      title="Add an ACP agent"
      description="Agents from the public ACP registry. Zuse launches the agent's package for you, or you can add your own command."
      footer={
        <>
          <ActionButton tone="quiet" onClick={() => props.onCustom(null)}>
            Custom command…
          </ActionButton>
          <ActionButton onClick={props.onClose}>Done</ActionButton>
        </>
      }
    >
      <label class="st-search">
        <Icon name="magnifer" size={14} />
        <input type="search" placeholder="Search agents…" aria-label="Search agents" value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
      </label>
      <Show when={custom.catalog?.error || custom.catalogError}>
        {(text) => <div class="st-dialog-error">{t("settings-acp_agents_registry_unavailable", "The registry is unavailable")}: {text()}</div>}
      </Show>
      <Show when={custom.error}>{(text) => <div class="st-dialog-error" role="alert">{text()}</div>}</Show>
      <div class="st-catalog-list" role="list" aria-label="ACP registry" aria-busy={custom.catalogLoading}>
        <Show when={!custom.catalogLoading || custom.catalog} fallback={<div class="st-empty">Loading agents…</div>}>
          <For each={entries()} fallback={<div class="st-empty">{t("settings-acp_agents_no_matches", "No matching agents")}</div>}>
            {(entry) => <CatalogEntry entry={entry} installed={installed()} onManual={(agent) => props.onCustom(agent)} />}
          </For>
        </Show>
      </div>
    </SettingsDialog>
  );
}

function CatalogEntry(props: { entry: RegistryAgent; installed: Set<string>; onManual: (agent: CustomAcpAgent) => void }) {
  const choice = () => offer(props.entry, custom.catalog?.platform ?? "");
  const added = () => {
    const o = choice();
    return o && o.kind !== "builtIn" && props.installed.has(o.agent.id);
  };
  const builtInEnabled = () => {
    const o = choice();
    if (o?.kind !== "builtIn") return false;
    const d = (state.harnesses as unknown as HarnessDescriptor[]).find((h) => h.id === o.harness);
    return !!d && descriptorEnabled(d);
  };
  const repository = () => (props.entry.repository?.startsWith("https://") ? props.entry.repository : null);
  const [adding, setAdding] = createSignal(false);
  return (
    <div class="st-catalog-item" role="listitem">
      <span class="st-catalog-icon" aria-hidden="true">
        <Show
          when={custom.icons[props.entry.id]}
          fallback={
            <Show when={choice()?.kind === "builtIn" ? (choice() as { harness: string }).harness : null} fallback={<Icon name="bot" size={16} />}>
              {(id) => (
                <span style={{ color: harnessTint(id()), display: "inline-flex" }}>
                  <Icon name={harnessMark(id())} size={16} />
                </span>
              )}
            </Show>
          }
        >
          {(src) => <img src={src()} alt="" />}
        </Show>
      </span>
      <div class="st-row-text">
        <div style={{ display: "flex", "align-items": "baseline", gap: "6px" }}>
          <span class="st-row-title">{props.entry.name}</span>
          <Show when={props.entry.version}>
            <span class="st-version">v{props.entry.version}</span>
          </Show>
        </div>
        <Show when={props.entry.description}>
          <div class="st-desc">{props.entry.description}</div>
        </Show>
      </div>
      <Show when={repository()}>
        {(href) => (
          <a class="st-icon-action" href={href()} target="_blank" rel="noopener noreferrer" aria-label={`View source for ${props.entry.name}`} title={`View source for ${props.entry.name}`}>
            <Icon name="arrow-up-right" size={14} />
          </a>
        )}
      </Show>
      <Show when={choice()} fallback={<Badge>Not available here</Badge>}>
        {(o) => (
          <Show
            when={o().kind !== "builtIn"}
            fallback={
              <Badge tone={builtInEnabled() ? "active" : undefined}>
                {builtInEnabled() ? "Built in" : `Built in · enable ${providerName((o() as { harness: string }).harness)} above`}
              </Badge>
            }
          >
            <Show when={!added()} fallback={<Badge tone="active">Added</Badge>}>
              <Show
                when={o().kind === "add"}
                fallback={
                  <ActionButton disabled={!!custom.busy} onClick={() => props.onManual((o() as { agent: CustomAcpAgent }).agent)}>
                    {t("settings-cloud_hosting_set_up", "Set up")}
                  </ActionButton>
                }
              >
                <ActionButton
                  disabled={!!custom.busy}
                  onClick={() => {
                    setAdding(true);
                    void saveAgent((o() as { agent: CustomAcpAgent }).agent).finally(() => setAdding(false));
                  }}
                >
                  {adding() ? "Adding…" : t("common-add", "Add")}
                </ActionButton>
              </Show>
            </Show>
          </Show>
        )}
      </Show>
    </div>
  );
}

function AgentForm(props: { form: Form | null; onClose: () => void }) {
  const [fields, setFields] = createSignal<AgentFields>(fieldsFor(null));
  const [advanced, setAdvanced] = createSignal(false);
  const [clearEnv, setClearEnv] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  createEffect(
    on(
      () => props.form,
      (form) => {
        if (!form) return;
        setFields(fieldsFor(form.agent));
        setAdvanced(false);
        setClearEnv(false);
        setError(null);
      },
    ),
  );
  const set = (key: keyof AgentFields) => (event: { currentTarget: { value: string } }) => setFields({ ...fields(), [key]: event.currentTarget.value });
  const saved = () => props.form?.agent?.env ?? {};
  const submit = async () => {
    if (custom.busy) return;
    let agent: CustomAcpAgent;
    try {
      agent = definition(fields(), clearEnv() ? null : saved());
    } catch (problem) {
      setError(problem instanceof Error ? problem.message : String(problem));
      return;
    }
    setError(null);
    const failure = await saveAgent(agent);
    if (failure) setError(failure);
    else props.onClose();
  };
  return (
    <SettingsDialog
      open={!!props.form}
      onClose={() => !custom.busy && props.onClose()}
      wide
      title={props.form?.existing ? `Edit ${props.form.agent?.name ?? "agent"}` : "Custom ACP command"}
      description="The command starts an agent that speaks the Agent Client Protocol on standard input and output. Arguments are taken literally; nothing runs through a shell."
      footer={
        <>
          <ActionButton tone="quiet" onClick={() => setAdvanced(!advanced())} aria-expanded={advanced()}>
            {advanced() ? "Hide advanced" : "Advanced"}
          </ActionButton>
          <span style={{ flex: 1 }} />
          <ActionButton tone="quiet" disabled={!!custom.busy} onClick={props.onClose}>
            Cancel
          </ActionButton>
          <ActionButton tone="solid" disabled={!!custom.busy} onClick={() => void submit()}>
            {custom.busy ?? "Save"}
          </ActionButton>
        </>
      }
    >
      <form
        class="st-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label>
          <span class="st-label">Name</span>
          <input class="st-field" placeholder="My agent" value={fields().name} onInput={set("name")} required />
        </label>
        <label>
          <span class="st-label">Executable</span>
          <input class="st-field" data-mono="true" placeholder="my-agent" value={fields().command} onInput={set("command")} spellcheck={false} required />
        </label>
        <label>
          <span class="st-label">Arguments</span>
          <input class="st-field" data-mono="true" placeholder="--acp" value={fields().args} onInput={set("args")} spellcheck={false} />
        </label>
        <label>
          <span class="st-label">Environment</span>
          <input
            class="st-field"
            data-mono="true"
            type="password"
            autocomplete="off"
            placeholder="API_KEY=… OTHER=…"
            value={fields().env}
            onInput={set("env")}
            spellcheck={false}
          />
          <Show when={Object.keys(saved()).length && !clearEnv()}>
            <span class="st-desc" style={{ "font-size": "10px" }}>
              Saved: {Object.keys(saved()).join(", ")}.{" "}
              <button type="button" class="st-link" onClick={() => setClearEnv(true)}>
                Clear variables
              </button>
            </span>
          </Show>
          <Show when={clearEnv()}>
            <span class="st-desc" style={{ "font-size": "10px" }}>
              Saved variables will be cleared.
            </span>
          </Show>
        </label>
        <Show when={advanced()}>
          <label>
            <span class="st-label">Provider ID</span>
            <input
              class="st-field"
              data-mono="true"
              placeholder="Provider ID (acp-name)"
              value={fields().id}
              onInput={set("id")}
              disabled={props.form?.existing}
              spellcheck={false}
            />
          </label>
          <label>
            <span class="st-label">Fallback model IDs</span>
            <textarea class="st-field" placeholder="Fallback model IDs — one per line" value={fields().models} onInput={set("models")} rows={3} spellcheck={false} />
          </label>
        </Show>
        <button type="submit" hidden />
      </form>
      <Show when={error()}>{(text) => <div class="st-dialog-error" role="alert">{text()}</div>}</Show>
    </SettingsDialog>
  );
}
