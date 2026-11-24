import { chatRequest } from "@/core/chat";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";

export interface UsageResumeStatus {
  enabled: boolean;
  pending: { resetsAt: number | null; enabled: boolean } | null;
}

/** The timer belongs to the engine. This only refreshes its presentation. */
export function useUsageResume(chat: Accessor<string | null>) {
  const [status, setStatus] = createSignal<UsageResumeStatus | null>(null);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  let generation = 0;
  const params = () => chat() ? chatRequest(chat()!) : {};
  const load = async () => {
    const current = generation;
    if (state.connection.kind !== "connected" || busy()) return;
    try {
      const value = await rpc.call<UsageResumeStatus>(M.GET_USAGE_RESUME, params());
      if (current === generation) setStatus(value);
    } catch { /* The connection strip handles read failures; mutations show errors. */ }
  };
  createEffect(() => [chat(), state.epoch, state.connection.kind, chat() ? state.sessions[chat()!]?.status : null] as const, () => {
    generation++;
    setStatus(null);
    setError(null);
    void load();
  });
  const timer = setInterval(() => void load(), 2000);
  onCleanup(() => { generation++; clearInterval(timer); });
  const mutate = async (method: typeof M.SET_USAGE_RESUME | typeof M.RESUME_USAGE_LIMIT, enabled?: boolean) => {
    if (busy() || state.connection.kind !== "connected") return;
    const current = ++generation;
    setBusy(true);
    setError(null);
    try {
      const value = await rpc.call<UsageResumeStatus>(method, { ...params(), ...(enabled === undefined ? {} : { enabled }) });
      if (current === generation) setStatus(value);
    } catch (cause) {
      if (current === generation) setError(cause instanceof Error ? cause.message : String(cause));
    } finally { setBusy(false); }
  };
  return { status, busy, error, setEnabled: (enabled: boolean) => mutate(M.SET_USAGE_RESUME, enabled), resume: () => mutate(M.RESUME_USAGE_LIMIT) };
}
