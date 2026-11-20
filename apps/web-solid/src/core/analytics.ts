import { createEffect, createRoot, untrack } from "solid-js";
import { ActiveTimeTracker, analyticsAccountId, createAnonymousAnalyticsId, type AnalyticsEventName, type AnalyticsProperties, sanitizeAnalyticsProperties } from "@zuse/analytics";
import { sanitizeAnalyticsEnvelope } from "@zuse/analytics/envelope";
import { state } from "./store";
import { view } from "./view";
import { rpc } from "./connection";
import { M } from "./methods";

declare const __APP_VERSION__: string;

const CONSENT = "zuse.web.analytics.enabled.v1";
const IDENTITY = "zuse.web.analytics.identity.v1";
const KEY = (import.meta.env.VITE_POSTHOG_KEY ?? "").trim();
const CONFIGURED = !!KEY && (import.meta.env.PROD || import.meta.env.VITE_POSTHOG_ENABLE_DEV === "1");
type Sdk = typeof import("posthog-js/dist/module.slim")["default"];
let sdk: Sdk | null = null;
let initialized = false;
let enabled = false;
let generation = 0;
let account: string | null = null;
let anonymous = "";
let cleanup: (() => void) | null = null;
let currentScreen = "chat";
let lastOpenedChat: string | null = null;
function reportChatOpened() {
  const id = (view.route.kind === "chat" || view.route.kind === "zappers") ? view.selectedChatId : null;
  if (!id) { lastOpenedChat = null; return; }
  if (!enabled || state.connection.kind !== "connected" || !state.workspaceSynced || lastOpenedChat === id) return;
  lastOpenedChat = id;
  void rpc.call(M.RECORD_CHAT_OPENED, { chatId: id, surface: "web" }).catch(() => {});
}

export function analyticsEnabled(): boolean {
  try { return localStorage.getItem(CONSENT) !== "false"; } catch { return false; }
}
function privacySignal(): boolean {
  return navigator.doNotTrack === "1" || (navigator as Navigator & { globalPrivacyControl?: boolean }).globalPrivacyControl === true;
}
function anonymousId(): string {
  const saved = localStorage.getItem(IDENTITY);
  if (saved && /^anonymous_[a-f0-9-]{36}$/.test(saved)) return saved;
  const id = createAnonymousAnalyticsId();
  localStorage.setItem(IDENTITY, id);
  return id;
}
export function captureAnalytics(event: AnalyticsEventName, properties: AnalyticsProperties = {}) {
  if (!enabled || !sdk) return;
  sdk.capture(event, { ...sanitizeAnalyticsProperties(event, {
    surface: "web", app_version: __APP_VERSION__, identity_kind: account ? "account" : "anonymous",
    authenticated: !!account, ...properties,
  }), distinct_id: account ? analyticsAccountId(account) : anonymous }, { transport: "sendBeacon" });
}

export async function setAnalyticsEnabled(next: boolean): Promise<void> {
  enabled = false;
  const version = ++generation;
  cleanup?.();
  cleanup = null;
  sdk?.opt_out_capturing();
  // Persist before any capture; storage errors fail closed.
  localStorage.setItem(CONSENT, String(next));
  if (!next || !CONFIGURED || privacySignal()) return;
  anonymous = anonymousId();
  const module = await import("posthog-js/dist/module.slim");
  if (version !== generation) return;
  sdk = module.default;
  if (!initialized) sdk.init(KEY, {
    api_host: import.meta.env.VITE_POSTHOG_HOST || "https://us.i.posthog.com",
    persistence: "memory", person_profiles: "never",
    autocapture: false, capture_pageview: false, capture_pageleave: false,
    capture_exceptions: false, disable_session_recording: true, disable_surveys: true,
    advanced_disable_feature_flags: true, advanced_disable_feature_flags_on_first_load: true,
    disable_external_dependency_loading: true, disable_web_experiments: true,
    request_batching: false, opt_out_capturing_by_default: true,
    bootstrap: { distinctID: account ? analyticsAccountId(account) : anonymous },
    before_send: (event) => enabled ? sanitizeAnalyticsEnvelope(event) : null,
  });
  initialized = true;
  enabled = true;
  sdk.opt_in_capturing({ captureEventName: false });
  captureAnalytics("app opened", { launch_type: "standard" });
  captureAnalytics("screen viewed", { screen: currentScreen });
  cleanup = activity();
  reportChatOpened();
}

function activity(): () => void {
  const tracker = new ActiveTimeTracker({ onInterval: ({ activeSeconds }) => captureAnalytics("app active interval", { active_seconds: activeSeconds }) });
  let foreground = false;
  const sync = () => {
    const next = !document.hidden && document.hasFocus();
    if (foreground === next) return;
    foreground = next;
    if (next) tracker.foreground();
    else { tracker.background(); captureAnalytics("app backgrounded", { active_seconds: 0 }); }
  };
  const interact = () => { if (foreground) tracker.interact(); };
  const events = ["pointerdown", "keydown", "scroll"] as const;
  events.forEach((event) => window.addEventListener(event, interact, { passive: true, capture: true }));
  window.addEventListener("focus", sync);
  window.addEventListener("blur", sync);
  document.addEventListener("visibilitychange", sync);
  const tick = window.setInterval(() => tracker.tick(), 10_000);
  sync();
  return () => {
    window.clearInterval(tick);
    events.forEach((event) => window.removeEventListener(event, interact, true));
    window.removeEventListener("focus", sync);
    window.removeEventListener("blur", sync);
    document.removeEventListener("visibilitychange", sync);
  };
}

export function startAnalytics(): void {
  // Track closed route names, never paths, selected entities, titles, or URLs.
  createRoot(() => {
    createEffect(() => [view.route.kind, view.selectedChatId, state.connection.kind, state.workspaceSynced], () => untrack(reportChatOpened));
    createEffect(() => view.route.kind, route => {
      currentScreen = route === "newProject" || route === "newZapper" ? "other" : route === "archive" ? "archives" : route === "zappers" ? "chat" : route;
      captureAnalytics("screen viewed", { screen: currentScreen });
    });
    createEffect(() => {
      const user = state.account?.user as { id?: unknown } | undefined;
      return typeof user?.id === "string" ? user.id : null;
    }, next => {
      if (account === next) return;
      const previous = account;
      account = next;
      if (previous && !next) {
        try { localStorage.removeItem(IDENTITY); anonymous = anonymousId(); } catch { enabled = false; }
      }
      // Each capture supplies our pseudonymous id without creating a person profile.
    });
  });
  void setAnalyticsEnabled(analyticsEnabled()).catch(() => {});
  window.addEventListener("storage", (event) => {
    if (event.key === CONSENT) void setAnalyticsEnabled(analyticsEnabled()).catch(() => {});
  });
}
