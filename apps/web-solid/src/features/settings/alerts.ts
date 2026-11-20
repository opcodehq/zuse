// App-wide effects of Settings: session sounds, browser notifications, agent update notices and reduced motion.
import { state } from "@/core/store";
import { openChat } from "@/core/view";
import { toast } from "@/ui/primitives";
import { createEffect, createRoot, createSignal } from "solid-js";
import { data } from "./data";
import { cliName } from "./harness";
import { pref } from "./prefs";
import { playAttention, playCompletion, playRequest } from "./sound";

const STALE_MS = 45_000;
type Indicator = "none" | "working" | "awaitingInput" | "errored";
interface Baseline {
  indicator: Indicator;
  lastCompletedTurn: string | null;
  fresh: boolean;
}

function baseline(session: Record<string, unknown>, now: number): Baseline {
  const updated = Date.parse(String(session.updatedAt ?? ""));
  const age = Number.isFinite(updated) ? now - updated : Infinity;
  const status = session.status;
  const indicator: Indicator =
    status === "errored" ? "errored" : status === "working" || status === "awaitingInput" ? (age > STALE_MS ? "none" : status) : "none";
  return { indicator, lastCompletedTurn: (session.lastCompletedTurn as string | null | undefined) ?? null, fresh: age <= STALE_MS };
}

type Cue = "attention" | "request" | "done";
function cueSince(next: Baseline, prev: Baseline): Cue | null {
  if (next.indicator === "errored" && prev.indicator !== "errored") return "attention";
  if (next.indicator === "awaitingInput" && prev.indicator !== "awaitingInput") return "request";
  if (next.fresh && next.lastCompletedTurn && next.lastCompletedTurn !== prev.lastCompletedTurn) return "done";
  return null;
}

const COPY: Record<Cue, string> = { attention: "Run failed", request: "Needs your input", done: "Finished" };
const RANK: Record<Cue, number> = { attention: 3, request: 2, done: 1 };

function notify(cue: Cue, chatId: string, browser: boolean, background: boolean, chatTitle?: string | null) {
  if (!browser || typeof Notification === "undefined" || Notification.permission !== "granted") return;
  if (background && document.visibilityState === "visible" && document.hasFocus()) return;
  const title = chatTitle?.trim() || "Untitled chat";
  try {
    const note = new Notification(title, { body: COPY[cue], tag: `zuse-${chatId}`, silent: true });
    note.onclick = () => {
      window.focus();
      openChat(chatId);
      note.close();
    };
  } catch {
    // Some browsers only allow notifications from a service worker.
  }
}

const supported = typeof Notification !== "undefined";
const [permission, setPermission] = createSignal<NotificationPermission | "unsupported">(supported ? Notification.permission : "unsupported");
export const notificationPermission = permission;

/** Must run from a user gesture; browsers ignore unprompted requests. */
export async function requestNotificationPermission() {
  if (!supported) return "unsupported" as const;
  const result = await Notification.requestPermission().catch(() => Notification.permission);
  setPermission(result);
  return result;
}

const ATTENTION_GAP_MS = 30_000;
let lastAttention = 0;

const media = window.matchMedia("(prefers-reduced-motion: reduce)");
const [systemReduced, setSystemReduced] = createSignal(media.matches);
media.addEventListener("change", (event) => setSystemReduced(event.matches));
export const systemReducedMotion = systemReduced;

createRoot(() => {
  const baselines = new Map<string, Baseline>();
  let epoch = -1;
  createEffect(() => ({
    epoch: state.epoch,
    sessions: Object.values(state.sessions).map(session => ({ id: session.chatId, next: baseline(session as unknown as Record<string, unknown>, Date.now()), child: !!state.chats[session.chatId]?.parentChatId })),
    sounds: pref("sessionSounds"), completion: pref("completionSound"), preset: pref("completionPreset"), input: pref("inputSound"), attention: pref("attentionSound"),
    browser: pref("browserNotifications"), background: pref("notifyInBackgroundOnly"),
    titles: Object.fromEntries(Object.values(state.chats).map(chat => [chat.id, chat.title])),
  }), (value) => {
    // A reconnect replays every row; treat it as a new baseline, never as fresh events.
    const replay = epoch !== value.epoch;
    epoch = value.epoch;
    let strongest: Cue | null = null;
    const seen = new Set<string>();
    for (const session of value.sessions) {
      const chatId = session.id;
      seen.add(chatId);
      const next = session.next;
      const prev = baselines.get(chatId);
      baselines.set(chatId, next);
      if (replay || !prev) continue;
      let cue = cueSince(next, prev);
      if (cue === "done" && session.child) cue = null;
      if (!cue) continue;
      notify(cue, chatId, value.browser, value.background, value.titles[chatId]);
      if (!strongest || RANK[cue] > RANK[strongest]) strongest = cue;
    }
    for (const chatId of [...baselines.keys()]) if (!seen.has(chatId)) baselines.delete(chatId);
    // One cue per snapshot, however many chats changed together.
    if (!strongest || !value.sounds) return;
    if (strongest === "done" && value.completion) playCompletion(value.preset);
    if (strongest === "request" && value.input) playRequest();
    if (strongest === "attention" && value.attention && Date.now() - lastAttention >= ATTENTION_GAP_MS) {
      lastAttention = Date.now();
      playAttention();
    }
  });

  const announced = new Set<string>();
  createEffect(() => data.updates, (rows) => {
    for (const row of rows ?? []) {
      const key = `${row.harness}@${row.latestVersion ?? ""}`;
      if (row.phase !== "available" || announced.has(key)) continue;
      announced.add(key);
      if (pref("providerUpdateNotifications")) {
        const name = (state.harnesses.find((h) => h.id === row.harness)?.name as string | undefined) ?? cliName(row.harness);
        toast(`${name} ${row.latestVersion ? `v${row.latestVersion}` : "update"} is available. Update it in Settings → Providers.`);
      }
    }
  });

  createEffect(() => [pref("reduceMotion"), systemReduced()] as const, ([choice, system]) => {
    if (choice === "on" || (choice === "system" && system)) document.documentElement.dataset.reduceMotion = "on";
    else delete document.documentElement.dataset.reduceMotion;
  });
});
