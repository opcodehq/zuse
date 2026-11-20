// Recorded usage: one controller for the dashboard (crates/viewport/src/usage_dashboard/mod.rs). The engine
// scans local agent logs; this keeps the latest overview, a page of sessions and live plan allowances.
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { view } from "@/core/view";
import { createStore, reconcile } from "solid-js";
import type { AccountsSnapshot } from "../data";
import { message } from "../data";

export const OVERVIEW = "UsageOverview";
export const SESSIONS = "UsageSessions";

export type CostStatus = "known" | "partial" | "unknown";
export interface Summary {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  reasoningTokens: number;
  costUsd: number | null;
  costStatus: CostStatus;
  recordCount: number;
  possibleDuplicateCount: number;
}
export interface UsageGroup extends Summary {
  key: string;
  label: string;
  startedAt: string | null;
  endedAt: string | null;
  sourceIds: string[];
}
export interface SourceStatus {
  id: string;
  label: string;
  detected: boolean;
  recordCount: number;
  paths: string[];
  warning: string | null;
}
export interface Overview {
  bucket: string;
  generatedAt: string;
  summary: Summary;
  sessionCount: number;
  previousSummary: Summary | null;
  previousSessionCount: number | null;
  groups: UsageGroup[];
  bySource: UsageGroup[];
  byModel: UsageGroup[];
  byProject: UsageGroup[];
  previousBySource: UsageGroup[];
  previousByModel: UsageGroup[];
  previousByProject: UsageGroup[];
  sources: SourceStatus[];
}
export interface SessionsPage {
  rows: UsageGroup[];
  total: number;
  nextOffset: number | null;
}
export type SessionSort = "tokens" | "cost" | "last-active";
export type Period = 7 | 30 | 90;

export const total = (s: Summary) => s.inputTokens + s.outputTokens + s.cacheReadTokens + s.cacheCreationTokens + s.reasoningTokens;

/** Usage sources the session filter offers, with their marks. */
export const PROVIDERS = [
  { id: "claude", name: "Claude Code", mark: "claude-mark" },
  { id: "codex", name: "Codex", mark: "openai-mark" },
  { id: "grok", name: "Grok", mark: "grok-mark" },
  { id: "gemini", name: "Gemini", mark: "gemini-mark" },
  { id: "kiro", name: "Kiro", mark: "kiro-mark" },
] as const;
/** Plan limit cards: usage source and the harness whose active account holds the allowance. */
export const LIMITS = [
  { harness: "claude-code", name: "Claude Code", mark: "claude-mark" },
  { harness: "codex", name: "Codex", mark: "openai-mark" },
  { harness: "grok", name: "Grok", mark: "grok-mark" },
  { harness: "gemini", name: "Gemini", mark: "gemini-mark" },
  { harness: "kiro", name: "Kiro", mark: "kiro-mark" },
] as const;

interface UsageState {
  projectScope: boolean;
  period: Period;
  range: { since: string; until: string; label: string } | null;
  report: Overview | null;
  reportError: string | null;
  reportLoading: boolean;
  page: SessionsPage | null;
  pageError: string | null;
  pageLoading: boolean;
  limits: AccountsSnapshot | null;
  limitsError: string | null;
  sessionsOpen: boolean;
  query: string;
  provider: string | null;
  sort: SessionSort;
  offset: number;
  cost: boolean;
  contributor: 0 | 1 | 2;
  expanded: boolean;
}

export const [usage, setUsage] = createStore<UsageState>({
  projectScope: false,
  period: 7,
  range: null,
  report: null,
  reportError: null,
  reportLoading: false,
  page: null,
  pageError: null,
  pageLoading: false,
  limits: null,
  limitsError: null,
  sessionsOpen: false,
  query: "",
  provider: null,
  sort: "tokens",
  offset: 0,
  cost: true,
  contributor: 0,
  expanded: false,
});

/** The project the dashboard can narrow to: the selected chat's, or the one a new chat would use. */
export function currentProject() {
  const chat = view.selectedChatId ? state.chats[view.selectedChatId] : undefined;
  const space =
    (chat?.spaceId ? state.spaces[chat.spaceId] : undefined) ??
    (view.draftProjectPath ? Object.values(state.spaces).find((s) => s.path === view.draftProjectPath) : undefined);
  if (!space) return null;
  return { id: space.id, label: space.name?.trim() || space.path.split(/[/\\]/).filter(Boolean).pop() || space.path, deviceId: space.deviceId };
}

const target = () => {
  const project = usage.projectScope ? currentProject() : null;
  const device = project?.deviceId;
  return device && device !== state.localDeviceId ? { targetDeviceId: device } : {};
};

const cache = new Map<string, Overview>();
let owner = "";
const reportKey = () => JSON.stringify([usage.period, usage.projectScope ? currentProject()?.id : null, usage.range?.since, usage.range?.until]);

function request(offsetOverride?: number) {
  const day = new Date();
  day.setHours(0, 0, 0, 0);
  day.setDate(day.getDate() - (usage.period - 1));
  return {
    since: usage.range?.since ?? day.toISOString(),
    until: usage.range?.until ?? null,
    projectId: usage.projectScope ? (currentProject()?.id ?? null) : null,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    query: usage.query.trim(),
    providerId: usage.provider,
    sort: usage.sort,
    offset: offsetOverride ?? usage.offset,
    limit: 10,
    ...target(),
  };
}

const unavailable = () => state.connection.kind !== "connected";
let reportSeq = 0;
let pageSeq = 0;
let limitsSeq = 0;

/** A different computer or account means different logs: forget what was cached. */
export function checkOwner() {
  const user = (state.account as { user?: { id?: string } } | null)?.user?.id ?? "";
  const next = `${state.localDeviceId ?? ""}|${user}`;
  if (next === owner) return;
  owner = next;
  cache.clear();
  setUsage(draft => { Object.assign(draft, { report: null, page: null, limits: null, reportError: null, pageError: null, limitsError: null }); });
}

export async function refresh(force = false) {
  checkOwner();
  const key = reportKey();
  const cached = cache.get(key);
  if (cached) setUsage(draft => { { const next = cached; if (draft["report"] == null) draft["report"] = next; else reconcile(next)(draft["report"]); } });
  const seq = ++reportSeq;
  setUsage(draft => { Object.assign(draft, { reportError: null }); });
  // The engine only probes allowances when forced; a first look must, or every card reads unavailable.
  void loadLimits(force || !usage.limits);
  queueMicrotask(() => { if (usage.sessionsOpen) void loadSessions(); });
  if (unavailable()) {
    setUsage(draft => { Object.assign(draft, { reportError: "Computer unavailable. Reconnect and refresh usage." }); });
    return;
  }
  setUsage(draft => { Object.assign(draft, { reportLoading: true }); });
  try {
    const report = await rpc.call<Overview>(OVERVIEW, { ...request(), forceRefresh: force });
    if (seq !== reportSeq) return;
    if (!report || !Array.isArray(report.groups)) throw new Error("Usage report could not be read. Refresh to try again.");
    cache.set(key, report);
    setUsage(draft => { { const next = report; if (draft["report"] == null) draft["report"] = next; else reconcile(next)(draft["report"]); } });
  } catch (error) {
    if (seq === reportSeq) setUsage(draft => { Object.assign(draft, { reportError: message(error) }); });
  } finally {
    if (seq === reportSeq) setUsage(draft => { Object.assign(draft, { reportLoading: false }); });
  }
}

export async function loadSessions() {
  if (!usage.sessionsOpen) return;
  const seq = ++pageSeq;
  setUsage(draft => { Object.assign(draft, { pageError: null }); });
  if (unavailable()) {
    setUsage(draft => { Object.assign(draft, { pageError: "Computer unavailable. Reconnect and retry." }); });
    return;
  }
  setUsage(draft => { Object.assign(draft, { pageLoading: true }); });
  try {
    const page = await rpc.call<SessionsPage>(SESSIONS, request());
    if (seq !== pageSeq) return;
    if (!page || !Array.isArray(page.rows)) throw new Error("Saved sessions could not be read. Retry to load them.");
    setUsage(draft => { { const next = page; if (draft["page"] == null) draft["page"] = next; else reconcile(next)(draft["page"]); } });
  } catch (error) {
    if (seq === pageSeq) setUsage(draft => { Object.assign(draft, { pageError: message(error) }); });
  } finally {
    if (seq === pageSeq) setUsage(draft => { Object.assign(draft, { pageLoading: false }); });
  }
}

async function loadLimits(force: boolean) {
  const seq = ++limitsSeq;
  setUsage(draft => { Object.assign(draft, { limitsError: null }); });
  if (unavailable()) {
    setUsage(draft => { Object.assign(draft, { limitsError: "Computer unavailable." }); });
    return;
  }
  try {
    const snapshot = await rpc.call<AccountsSnapshot>(M.LIST_AGENT_ACCOUNTS, { forceUsage: force, ...target() });
    if (seq === limitsSeq) setUsage(draft => { { const next = snapshot; if (draft["limits"] == null) draft["limits"] = next; else reconcile(next, "id")(draft["limits"]); } });
  } catch (error) {
    if (seq === limitsSeq) setUsage(draft => { Object.assign(draft, { limitsError: message(error) || "Usage limits are unavailable." }); });
  }
}

let queryTimer: ReturnType<typeof setTimeout> | undefined;
export function setQuery(value: string) {
  setUsage(draft => { Object.assign(draft, { query: value, offset: 0 }); });
  clearTimeout(queryTimer);
  queryTimer = setTimeout(() => void loadSessions(), 150);
}

/** Opening resets the view the way desktop does, keeping the period. */
export function open() {
  setUsage(draft => {
    Object.assign(draft, {
      projectScope: false,
      range: null,
      cost: true,
      contributor: 0,
      expanded: false,
      sessionsOpen: false,
      provider: null,
      sort: "tokens",
      query: "",
      offset: 0,
      page: null,
    });
  });
  queueMicrotask(() => void refresh(false));
}

export function setScope(project: boolean) {
  if (usage.projectScope === project || (project && !currentProject())) return;
  setUsage(draft => { Object.assign(draft, { projectScope: project, page: null, offset: 0 }); });
  queueMicrotask(() => void refresh(false));
}

export function setPeriod(period: Period) {
  setUsage(draft => { Object.assign(draft, { period, range: null, page: null, offset: 0 }); });
  queueMicrotask(() => void refresh(false));
}

export function viewDay(group: UsageGroup) {
  if (!group.startedAt) return;
  const since = group.startedAt;
  const until = group.endedAt ?? new Date(Date.parse(since) + 86_400_000).toISOString();
  setUsage(draft => { Object.assign(draft, { range: { since, until, label: group.label }, page: null, offset: 0 }); });
  queueMicrotask(() => void refresh(false));
}

export function clearDay() {
  setUsage(draft => { Object.assign(draft, { range: null, page: null, offset: 0 }); });
  queueMicrotask(() => void refresh(false));
}

export function toggleSessions() {
  setUsage(draft => { draft["sessionsOpen"] = !usage.sessionsOpen; });
  queueMicrotask(() => { if (usage.sessionsOpen) void loadSessions(); });
}

export function setProvider(provider: string | null) {
  setUsage(draft => { Object.assign(draft, { provider, offset: 0 }); });
  queueMicrotask(() => void loadSessions());
}

export function setSort(sort: SessionSort) {
  setUsage(draft => { Object.assign(draft, { sort, offset: 0 }); });
  queueMicrotask(() => void loadSessions());
}

export function setOffset(offset: number) {
  if (offset === usage.offset || usage.pageLoading) return;
  setUsage(draft => { Object.assign(draft, { offset }); });
  queueMicrotask(() => void loadSessions());
}
