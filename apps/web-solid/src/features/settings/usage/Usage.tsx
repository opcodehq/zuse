// Usage dashboard: locally recorded tokens and estimated cost, plan limits, breakdowns and sessions
// (crates/viewport/src/usage_dashboard/view.rs).
import { Select as KSelect } from "@kobalte/core/select";
import { createEffect, createMemo, createSignal, For, type JSX, on, onMount, Show } from "solid-js";
import { state } from "@/core/store";
import { navigate } from "@/core/view";
import { Icon } from "@/ui/Icon";
import { GlyphSpinner } from "@/ui/loaders";
import { t } from "../i18n";
import { Chart, COST_SERIES, SERIES } from "./Chart";
import { delta, money, share, tokens } from "./format";
import {
  checkOwner,
  clearDay,
  currentProject,
  LIMITS,
  loadSessions,
  open,
  type Overview,
  type Period,
  PROVIDERS,
  refresh,
  type SessionSort,
  setOffset,
  setPeriod,
  setProvider,
  setQuery,
  setScope,
  setSort,
  setUsage,
  toggleSessions,
  total,
  usage,
  type UsageGroup,
  viewDay,
} from "./state";

const close = () => navigate({ kind: "chat" });

function Seg(props: { label: string; children: JSX.Element }) {
  return (
    <div class="us-seg" role="group" aria-label={props.label}>
      {props.children}
    </div>
  );
}

function SegButton(props: { pressed: boolean; disabled?: boolean; onClick: () => void; children: JSX.Element; label?: string }) {
  return (
    <button type="button" class="us-btn" aria-pressed={props.pressed} aria-label={props.label} disabled={props.disabled} onClick={() => props.onClick()}>
      {props.children}
    </button>
  );
}

export function Usage() {
  let root: HTMLDivElement | undefined;
  onMount(() => {
    open();
    root?.focus({ preventScroll: true });
  });
  createEffect(on(() => state.epoch, () => void refresh(false), { defer: true }));
  createEffect(on(() => [state.localDeviceId, (state.account as { user?: { id?: string } } | null)?.user?.id], () => checkOwner(), { defer: true }));
  // The project scope follows the selected chat; losing the project falls back to all projects.
  createEffect(
    on(
      () => currentProject()?.id,
      (id, previous) => {
        if (!usage.projectScope || id === previous) return;
        if (!id) setScope(false);
        else void refresh(false);
      },
      { defer: true },
    ),
  );
  const project = () => currentProject();
  const report = () => usage.report;

  return (
    <div
      ref={root}
      class="us-root"
      tabIndex={-1}
      role="region"
      aria-label={t("usage-usage_dashboard_usage", "Usage")}
      onKeyDown={(e) => {
        if (e.key !== "Escape" || e.defaultPrevented || document.querySelector('[role="listbox"]')) return;
        e.preventDefault();
        close();
      }}
    >
      <header class="us-head">
        <h1>{t("usage-usage_dashboard_usage", "Usage")}</h1>
        <button type="button" class="st-icon-action" data-size="md" aria-label="Close usage" title="Close usage" onClick={close}>
          <Icon name="close" size={16} />
        </button>
      </header>
      <div class="us-controls">
        <span class="us-controls-scope">{usage.projectScope ? (project()?.label ?? "This project") : t("usage-usage_dashboard_all_projects", "All projects")}</span>
        <div class="us-controls-right">
          <Seg label="Scope">
            <SegButton pressed={!usage.projectScope} onClick={() => setScope(false)}>
              All projects
            </SegButton>
            <SegButton pressed={usage.projectScope} disabled={!project()} label={project() ? `Current project: ${project()!.label}` : "Current project (open a project chat first)"} onClick={() => setScope(true)}>
              Current
            </SegButton>
          </Seg>
          <Seg label="Period">
            <For each={[7, 30, 90] as Period[]}>
              {(days) => (
                <SegButton pressed={usage.period === days && !usage.range} label={`Last ${days} days`} onClick={() => setPeriod(days)}>
                  {days}D
                </SegButton>
              )}
            </For>
          </Seg>
          <button
            type="button"
            class="us-btn us-icon-btn"
            aria-label="Refresh usage"
            title="Refresh usage"
            data-busy={usage.reportLoading ? "true" : undefined}
            disabled={usage.reportLoading}
            onClick={() => void refresh(true)}
          >
            <Show when={usage.reportLoading} fallback={<Icon name="refresh" size={14} />}>
              <GlyphSpinner cell={1.75} decorative />
            </Show>
          </button>
        </div>
      </div>
      <Show when={usage.reportError && report()}>
        <div class="us-notice" role="status">
          Showing saved usage. Refresh failed: {usage.reportError}
        </div>
      </Show>
      <Show
        when={report()}
        fallback={
          <Show when={usage.reportLoading} fallback={<Empty />}>
            <Skeleton />
          </Show>
        }
      >
        {(r) => (
          <div class="us-scroll">
            <div class="us-body">
              <Daily report={r()} />
              <Metrics report={r()} />
              <Limits />
              <Breakdown report={r()} />
              <Sessions report={r()} />
              <div class="us-status-line">
                <span>{r().sources.filter((s) => s.detected).length} sources detected</span>
                <span aria-live="polite">
                  {usage.reportLoading ? "Updating…" : `Updated ${new Date(r().generatedAt).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`}
                </span>
              </div>
            </div>
          </div>
        )}
      </Show>
    </div>
  );
}

function Empty() {
  return (
    <div class="us-empty">
      <span>{usage.reportError ?? "No usage has been recorded yet."}</span>
      <button type="button" class="st-action" data-tone="outlined" onClick={() => void refresh(true)}>
        Refresh usage
      </button>
    </div>
  );
}

function Skeleton() {
  return (
    <div class="us-body" aria-busy="true" aria-label="Loading usage">
      <div class="st-skeleton" style={{ height: "288px" }} />
      <div class="us-metrics">
        <For each={[0, 1, 2, 3]}>{() => <div class="st-skeleton" style={{ height: "96px" }} />}</For>
      </div>
      <div class="st-skeleton" style={{ height: "208px" }} />
    </div>
  );
}

const measure = (g: UsageGroup) => (usage.cost ? (g.costUsd ?? 0) : total(g));

function Daily(props: { report: Overview }) {
  const [hovered, setHovered] = createSignal<number | null>(null);
  const days = createMemo(() => props.report.groups.slice(-90));
  const sources = createMemo(() =>
    [...props.report.bySource].sort((a, b) => measure(b) - measure(a)).slice(0, 5),
  );
  const denominator = () => Math.max(1, sources().reduce((sum, g) => sum + measure(g), 0));
  const hoveredGroup = () => (hovered() === null ? null : days()[hovered()!]);
  return (
    <section class="us-frame" aria-labelledby="usage-daily">
      <div class="us-frame-head">
        <h2 id="usage-daily">
          {t("usage-usage_dashboard_daily_usage", "Daily usage")}
          <Show when={usage.range}>
            {(range) => (
              <button type="button" class="us-btn" onClick={clearDay} aria-label={`Clear day ${range().label}`}>
                {range().label} · Clear
              </button>
            )}
          </Show>
        </h2>
        <Seg label="Measure">
          <SegButton pressed={!usage.cost} onClick={() => setUsage("cost", false)}>
            Tokens
          </SegButton>
          <SegButton pressed={usage.cost} onClick={() => setUsage("cost", true)}>
            Cost
          </SegButton>
        </Seg>
      </div>
      <div style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
        <div class="us-card">
          <div class="us-summary">
            <div class="us-lead">
              <div class="us-eyebrow">
                <i aria-hidden="true" />
                {usage.cost ? "ESTIMATED API COST" : "PROCESSED TOKENS"}
              </div>
              <div class="us-total">{usage.cost ? money(props.report.summary.costUsd) : tokens(total(props.report.summary))}</div>
              <div class="us-hint">
                {usage.cost
                  ? "Estimated from local usage at published provider rates. Actual billing may vary."
                  : `${props.report.summary.recordCount} locally observed usage records in this period.`}
              </div>
            </div>
            <div class="us-sources">
              <div class="us-sources-head">
                <span>{usage.cost ? "COST BY SOURCE" : "TOKENS BY SOURCE"}</span>
                <span>
                  {sources().length} active {sources().length === 1 ? "source" : "sources"}
                </span>
              </div>
              <div class="us-source-grid">
                <For each={sources()}>
                  {(source) => (
                    <div class="us-source">
                      <div class="us-source-line">
                        <span>{source.label}</span>
                        <span>{usage.cost ? money(source.costUsd) : tokens(total(source))}</span>
                      </div>
                      <div class="us-bar" role="presentation">
                        <i style={{ width: `${Math.min(1, Math.max(0, measure(source) / denominator())) * 100}%` }} />
                      </div>
                    </div>
                  )}
                </For>
              </div>
            </div>
          </div>
        </div>
        <div class="us-card us-plot">
          <Show
            when={days().length}
            fallback={<div style={{ height: "256px", display: "grid", "place-items": "center", color: "var(--text-muted)" }}>{t("usage-usage_dashboard_no_usage_in_this_period", "No usage in this period")}</div>}
          >
            <div class="us-legend" aria-hidden="true">
              <For each={usage.cost ? [...COST_SERIES] : [...SERIES]}>
                {(s) => (
                  <span>
                    <i style={{ background: s.color }} />
                    {s.name}
                  </span>
                )}
              </For>
            </div>
            <Chart groups={days()} cost={usage.cost} hovered={hovered()} onHover={setHovered} onViewDay={(i) => days()[i] && viewDay(days()[i]!)} />
            <div class="us-foot" aria-live="polite">
              <Show when={hoveredGroup()} fallback={<span>{t("usage-usage_dashboard_hover_the_chart_for_a_daily_breakdown", "Hover the chart for a daily breakdown")}</span>}>
                {(g) => (
                  <>
                    <strong>{g().label}</strong>
                    <span class="us-foot-values">
                      {tokens(total(g()))} · {money(g().costUsd)}
                    </span>
                    <button type="button" class="us-btn" style={{ "font-size": "11px", "font-weight": 500 }} disabled={!g().startedAt} onClick={() => viewDay(g())}>
                      View day
                    </button>
                  </>
                )}
              </Show>
            </div>
          </Show>
        </div>
      </div>
    </section>
  );
}

function Metrics(props: { report: Overview }) {
  const rows = () => {
    const s = props.report.summary;
    const p = props.report.previousSummary;
    return [
      {
        name: "Cost",
        value: money(s.costUsd),
        hint: s.costStatus === "partial" ? "Some models are unpriced" : s.costStatus === "unknown" ? "Pricing unavailable" : "Estimated from local usage",
        change: delta(s.costUsd, p?.costUsd),
      },
      { name: "Tokens", value: tokens(total(s)), hint: `${tokens(s.inputTokens)} in · ${tokens(s.outputTokens)} out`, change: delta(total(s), p ? total(p) : null) },
      {
        name: "Cache reads",
        value: tokens(s.cacheReadTokens),
        hint: `${Math.round((s.cacheReadTokens / Math.max(1, s.inputTokens + s.cacheReadTokens)) * 100)}% of observed input`,
        change: delta(s.cacheReadTokens, p?.cacheReadTokens),
      },
      {
        name: "Active sessions",
        value: String(props.report.sessionCount),
        hint: `${s.recordCount} usage records`,
        change: delta(props.report.sessionCount, props.report.previousSessionCount),
      },
    ];
  };
  return (
    <section class="us-frame" aria-label="Summary">
      <div class="us-metrics">
        <For each={rows()}>
          {(row) => (
            <div class="us-card us-metric">
              <div class="us-metric-head">
                <span>{row.name}</span>
                <Show when={row.change}>
                  <span>{row.change}</span>
                </Show>
              </div>
              <div class="us-metric-value">{row.value}</div>
              <div class="us-metric-hint" title={row.hint}>
                {row.hint}
              </div>
            </div>
          )}
        </For>
      </div>
    </section>
  );
}

function Limits() {
  return (
    <section class="us-frame" aria-labelledby="usage-limits">
      <div class="us-frame-head">
        <h2 id="usage-limits">{t("usage-usage_dashboard_plan_limits", "Plan limits")}</h2>
        <span class="st-desc">{t("usage-usage_dashboard_live_provider_allowance", "Live provider allowance")}</span>
      </div>
      <div class="us-limits">
        <For each={LIMITS}>
          {(provider) => {
            const account = () => usage.limits?.accounts.find((a) => a.harness === provider.harness && a.active);
            const primary = () => account()?.usageWindows?.[0];
            const fraction = () => Math.min(1, Math.max(0, primary()?.usedFraction ?? 0));
            const filled = () => Math.ceil(fraction() * 38);
            return (
              <div class="us-card us-limit">
                <span class="us-limit-mark" aria-hidden="true">
                  <Icon name={provider.mark} size={16} />
                </span>
                <div class="us-limit-info">
                  <div class="us-limit-head">
                    <div style={{ "min-width": 0 }}>
                      <div class="us-limit-name">{provider.name}</div>
                      <div class="us-limit-sub">{account()?.planLabel ?? primary()?.label ?? "Usage limits"}</div>
                    </div>
                    <div>
                      <div class="us-limit-left">{primary() ? `${Math.round((1 - fraction()) * 100)}% left` : "—"}</div>
                      <div class="us-limit-sub" style={{ "text-align": "right" }}>
                        {primary()?.resetsAt
                          ? new Date(primary()!.resetsAt!).toLocaleString(undefined, { month: "short", day: "numeric", hour: "numeric", minute: "2-digit" })
                          : primary()
                            ? "No reset"
                            : "Usage unavailable"}
                      </div>
                    </div>
                  </div>
                  <Show
                    when={primary()}
                    fallback={
                      <Show when={account()?.usageError ?? usage.limitsError}>
                        {(error) => <div class="us-limit-sub" style={{ "margin-top": "4px" }}>{error()}</div>}
                      </Show>
                    }
                  >
                    <div
                      class="us-segments"
                      data-high={fraction() >= 0.8 ? "true" : undefined}
                      role="meter"
                      aria-label={`${provider.name} ${primary()!.label} usage`}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-valuenow={Math.round(fraction() * 100)}
                    >
                      <For each={Array.from({ length: 38 }, (_, i) => i)}>{(i) => <i data-on={i < filled() ? "true" : undefined} />}</For>
                    </div>
                  </Show>
                </div>
              </div>
            );
          }}
        </For>
      </div>
    </section>
  );
}

function Breakdown(props: { report: Overview }) {
  const tabs = ["Providers", "Models", "Projects"] as const;
  const view = createMemo(() => {
    const r = props.report;
    const [rows, previous, title] =
      usage.contributor === 1 ? [r.byModel, r.previousByModel, "Model"] : usage.contributor === 2 ? [r.byProject, r.previousByProject, "Project"] : [r.bySource, r.previousBySource, "Provider"];
    const sorted = [...rows].sort((a, b) => total(b) - total(a));
    const sum = Math.max(1, sorted.reduce((acc, g) => acc + total(g), 0));
    return {
      title,
      count: sorted.length,
      rows: (usage.expanded ? sorted : sorted.slice(0, 5)).map((g) => {
        const prior = previous.find((p) => p.key === g.key);
        const change = delta(total(g), prior ? total(prior) : null);
        return { key: g.key, label: g.label, share: `${share((total(g) / sum) * 100)}${change ? ` · ${change}` : ""}`, tokens: tokens(total(g)), cost: money(g.costUsd) };
      }),
    };
  });
  return (
    <section class="us-frame" aria-labelledby="usage-breakdown">
      <div class="us-frame-head">
        <h2 id="usage-breakdown">{t("usage-usage_dashboard_breakdown", "Breakdown")}</h2>
        <div class="us-seg" role="tablist" aria-label="Breakdown by">
          <For each={tabs}>
            {(name, i) => (
              <button
                type="button"
                role="tab"
                class="us-btn"
                aria-selected={usage.contributor === i()}
                aria-pressed={usage.contributor === i()}
                onClick={() => setUsage({ contributor: i() as 0 | 1 | 2, expanded: false })}
              >
                {name}
              </button>
            )}
          </For>
        </div>
      </div>
      <div class="us-card" style={{ overflow: "hidden" }}>
        <div class="us-table-wrap">
          <table class="us-table">
            <thead>
              <tr>
                <th scope="col">{view().title}</th>
                <th scope="col">Share</th>
                <th scope="col">Tokens</th>
                <th scope="col">Cost</th>
              </tr>
            </thead>
            <tbody>
              <For
                each={view().rows}
                fallback={
                  <tr>
                    <td colSpan={4} style={{ "text-align": "center", height: "64px", color: "var(--text-muted)", "max-width": "none" }}>
                      {t("usage-usage_dashboard_no_contributors_found", "No contributors found")}
                    </td>
                  </tr>
                }
              >
                {(row) => (
                  <tr>
                    <td title={row.label}>{row.label}</td>
                    <td>{row.share}</td>
                    <td>{row.tokens}</td>
                    <td>{row.cost}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </div>
      <Show when={view().count > 5}>
        <div style={{ display: "flex", "justify-content": "center", padding: "8px" }}>
          <button type="button" class="us-btn" aria-expanded={usage.expanded} onClick={() => setUsage("expanded", !usage.expanded)}>
            {usage.expanded ? "Show less" : `View all ${view().count}`}
          </button>
        </div>
      </Show>
    </section>
  );
}

const SORTS: { id: SessionSort; name: string }[] = [
  { id: "last-active", name: "Last active" },
  { id: "tokens", name: "Tokens" },
  { id: "cost", name: "Cost" },
];

function lastActive(g: UsageGroup) {
  const at = g.endedAt ?? g.startedAt;
  return at ? `Last active ${new Date(at).toLocaleDateString(undefined, { month: "numeric", day: "numeric", year: "numeric" })}` : "Last active unavailable";
}

function Sessions(props: { report: Overview }) {
  const totalRows = () => usage.page?.total ?? props.report.sessionCount;
  const range = () => (totalRows() === 0 ? "0" : `${usage.offset + 1}–${Math.min(usage.offset + 10, totalRows())}`);
  return (
    <section class="us-frame">
      <button type="button" class="us-sessions-toggle" aria-expanded={usage.sessionsOpen} aria-controls="usage-sessions" onClick={toggleSessions}>
        <span>
          <strong>{t("native-sessions", "Sessions")}</strong>
          <small>{props.report.sessionCount} in this period</small>
        </span>
        <Icon name={usage.sessionsOpen ? "alt-arrow-up" : "alt-arrow-down"} size={16} />
      </button>
      <Show when={usage.sessionsOpen}>
        <div id="usage-sessions">
          <div class="us-filters">
            <label class="st-search">
              <Icon name="magnifer" size={14} />
              <input type="search" placeholder="Search sessions…" aria-label="Search sessions" value={usage.query} onInput={(e) => setQuery(e.currentTarget.value)} />
            </label>
            <ProviderFilter />
          </div>
          <Show when={usage.pageError}>
            <div class="us-notice" role="alert" style={{ color: "var(--danger)", background: "color-mix(in srgb, var(--danger) 8%, transparent)" }}>
              {t("usage-usage_dashboard_could_not_load_sessions_change_a_filter_or_try_again", "Could not load sessions. Change a filter or try again.")}{" "}
              <button type="button" class="st-link" onClick={() => void loadSessions()}>
                Retry
              </button>
            </div>
          </Show>
          <div class="us-card" style={{ overflow: "hidden", margin: "0 0 0" }}>
            <div class="us-table-wrap">
              <table class="us-table" aria-busy={usage.pageLoading}>
                <thead>
                  <tr>
                    <th scope="col">{t("usage-usage_dashboard_session", "Session")}</th>
                    <For each={SORTS}>
                      {(sort) => (
                        <th scope="col" aria-sort={usage.sort === sort.id ? "descending" : "none"}>
                          <button type="button" class="us-sort" onClick={() => setSort(sort.id)}>
                            {sort.name}
                            <span data-idle={usage.sort === sort.id ? undefined : "true"} style={{ display: "inline-flex" }}>
                              <Icon name={usage.sort === sort.id ? "alt-arrow-down" : "alt-arrow-up"} size={14} />
                            </span>
                          </button>
                        </th>
                      )}
                    </For>
                  </tr>
                </thead>
                <tbody>
                  <Show
                    when={usage.page}
                    fallback={
                      <tr>
                        <td colSpan={4} style={{ "text-align": "center", height: "80px", color: "var(--text-muted)", "max-width": "none" }}>
                          {usage.pageLoading ? t("usage-usage_dashboard_loading_sessions", "Loading sessions…") : ""}
                        </td>
                      </tr>
                    }
                  >
                    <For
                      each={usage.page!.rows}
                      fallback={
                        <tr>
                          <td colSpan={4} style={{ "text-align": "center", height: "64px", color: "var(--text-muted)", "max-width": "none" }}>
                            No sessions match.
                          </td>
                        </tr>
                      }
                    >
                      {(row) => (
                        <tr>
                          <td title={row.label}>{row.label}</td>
                          <td>{lastActive(row)}</td>
                          <td>{tokens(total(row))}</td>
                          <td>{money(row.costUsd)}</td>
                        </tr>
                      )}
                    </For>
                  </Show>
                </tbody>
              </table>
            </div>
          </div>
          <div class="us-pager">
            <span>
              {range()} of {totalRows()}
            </span>
            <span style={{ display: "flex", gap: "4px" }}>
              <button type="button" class="us-pager-btn" aria-label="Previous page" disabled={usage.offset === 0 || usage.pageLoading} onClick={() => setOffset(Math.max(0, usage.offset - 10))}>
                <Icon name="alt-arrow-left" size={16} />
              </button>
              <button
                type="button"
                class="us-pager-btn"
                aria-label="Next page"
                disabled={usage.page?.nextOffset == null || usage.pageLoading}
                onClick={() => usage.page?.nextOffset != null && setOffset(usage.page.nextOffset)}
              >
                <Icon name="alt-arrow-right" size={16} />
              </button>
            </span>
          </div>
        </div>
      </Show>
    </section>
  );
}

interface ProviderChoice {
  id: string;
  name: string;
  mark?: string;
}

function ProviderFilter() {
  // Kobalte treats an empty key as no selection, so "all" stands for the unfiltered choice.
  const options: ProviderChoice[] = [{ id: "all", name: "All providers" }, ...PROVIDERS];
  const selected = () => options.find((o) => o.id === (usage.provider ?? "all")) ?? options[0]!;
  return (
    <KSelect<ProviderChoice>
      options={options}
      optionValue="id"
      optionTextValue="name"
      value={selected()}
      onChange={(option) => option && setProvider(option.id === "all" ? null : option.id)}
      gutter={4}
      placement="bottom-end"
      itemComponent={(item) => (
        <KSelect.Item item={item.item} class="st-select-item">
          <span class="st-select-check">
            <KSelect.ItemIndicator>
              <Icon name="check" size={14} />
            </KSelect.ItemIndicator>
          </span>
          <Show when={item.item.rawValue.mark}>{(mark) => <Icon name={mark()} size={14} />}</Show>
          <KSelect.ItemLabel>{item.item.rawValue.name}</KSelect.ItemLabel>
        </KSelect.Item>
      )}
    >
      <KSelect.Trigger class="st-select" aria-label="Provider" style={{ height: "28px", "min-width": "140px" }}>
        <KSelect.Value<ProviderChoice> class="st-select-value">{(s) => s.selectedOption()?.name ?? "All providers"}</KSelect.Value>
        <KSelect.Icon class="st-select-icon">
          <Icon name="alt-arrow-down" size={14} />
        </KSelect.Icon>
      </KSelect.Trigger>
      <KSelect.Portal>
        <KSelect.Content class="st-select-menu">
          <KSelect.Listbox class="st-select-list" />
        </KSelect.Content>
      </KSelect.Portal>
    </KSelect>
  );
}
