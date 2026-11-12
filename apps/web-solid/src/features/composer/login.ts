// Provider sign-in from the composer tray: start the agent's login, open its page, poll until it settles.
import { createSignal } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { navigate } from "@/core/view";

/** Display names for agents with a login of their own (crates/ui/src/settings/accounts.rs PROVIDERS). */
export const PROVIDER_NAMES: Record<string, string> = {
  "claude-code": "Claude Code", codex: "Codex", cursor: "Cursor", antigravity: "Antigravity", grok: "Grok",
  devin: "Devin", opencode: "OpenCode", pi: "Pi", hermes: "Hermes",
};
export const signsIn = (harness: string | undefined) => !!harness && harness in PROVIDER_NAMES;
export const providerName = (harness: string | undefined) => (harness && PROVIDER_NAMES[harness]) || "your provider";

export interface Login {
  chatId: string;
  harness: string;
  attempt: number;
  step: "starting" | "waiting" | "failed";
  message?: string;
  url?: string;
  loginId?: string;
  /** The transcript error this sign-in resolves. */
  errorId?: string;
  target: string | null;
}

const [login, setLogin] = createSignal<Login | null>(null);
export { login };

let attempts = 0;
let pollTimer: ReturnType<typeof setTimeout> | undefined;
const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));
const withTarget = (target: string | null, params: Record<string, unknown>) => (target ? { ...params, targetDeviceId: target } : params);

function openPage(url: string) {
  window.open(url, "_blank", "noopener,noreferrer");
}

interface Start { loginId: string; url: string; mode: string }
interface Poll { status: "pending" | "done" | "error"; message?: string; url?: string }

/** Begin a sign-in for the chat's agent. `onDone` runs once the provider reports success. */
export async function startLogin(chatId: string, harness: string, errorId: string | undefined, onDone: (errorId?: string) => void) {
  cancelLogin();
  const attempt = ++attempts;
  const chat = state.chats[chatId];
  const target = chat && chat.deviceId !== state.localDeviceId ? chat.deviceId : null;
  setLogin({ chatId, harness, attempt, step: "starting", errorId, target });
  const current = () => login()?.attempt === attempt;
  let start: Start;
  try {
    start = await rpc.call<Start>(M.START_AGENT_LOGIN, withTarget(target, { harness }));
  } catch (error) {
    if (current()) setLogin((l) => l && { ...l, step: "failed", message: `Couldn't start the sign-in: ${errorText(error)}` });
    return;
  }
  if (!current()) return;
  if (start.mode === "paste-code") {
    // The paste-code step lives with the provider accounts in Settings.
    setLogin(null);
    navigate({ kind: "settings", section: "providers" });
    return;
  }
  if (start.url) openPage(start.url);
  setLogin((l) => l && { ...l, step: "waiting", url: start.url || undefined, loginId: start.loginId });
  const poll = async () => {
    if (!current()) return;
    let reply: Poll;
    try {
      reply = await rpc.call<Poll>(M.POLL_AGENT_LOGIN, withTarget(target, { loginId: start.loginId }));
    } catch (error) {
      if (current()) setLogin((l) => l && { ...l, step: "failed", message: errorText(error) });
      return;
    }
    if (!current()) return;
    if (reply.status === "done") {
      const done = login();
      setLogin(null);
      onDone(done?.errorId);
      return;
    }
    if (reply.status === "error") {
      setLogin((l) => l && { ...l, step: "failed", message: reply.message || "The sign-in failed." });
      return;
    }
    if (reply.url && reply.url !== login()?.url) {
      openPage(reply.url);
      setLogin((l) => l && { ...l, url: reply.url });
    }
    setLogin((l) => l && { ...l, message: reply.message });
    pollTimer = setTimeout(() => void poll(), 1500);
  };
  pollTimer = setTimeout(() => void poll(), 1500);
}

export function cancelLogin() {
  clearTimeout(pollTimer);
  const current = login();
  setLogin(null);
  if (current?.loginId && current.step === "waiting")
    void rpc.call(M.CANCEL_AGENT_LOGIN, withTarget(current.target, { loginId: current.loginId })).catch(() => {});
}

export function reopenLoginPage() {
  const url = login()?.url;
  if (url) openPage(url);
}
