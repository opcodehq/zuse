// Zuse account sign-in from the browser, run by the engine. A remote browser confirms a device
// code; a browser on the engine's own machine can follow the loopback redirect instead.
import { createSignal, Show } from "solid-js";
import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { state } from "@/core/store";
import { area } from "./status";
import { Btn, Details, ErrorStrip } from "./ui";
import { maskedEmail } from "@/core/util";

interface AccountStatus {
  user?: { id?: string; email?: string } | null;
  login?: { url?: string; userCode?: string; expiresAt?: number } | null;
  error?: string | null;
}

export const account = () => (state.account ?? {}) as AccountStatus;

export { maskedEmail };

const onEngineMachine = () => area.mode === "owner" && ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname);
const safeUrl = (url: string | undefined) => (url?.startsWith("https://") ? url : null);

/** Starts sign-in. Resolves to an error message, or null once the engine is waiting for confirmation. */
export async function signInToAccount(): Promise<string | null> {
  // Opened during the click so pop-up blockers allow it; pointed at the sign-in page once known.
  const pending = window.open("about:blank", "_blank");
  try {
    const progress = await rpc.call<{ url?: string }>(M.CLOUD_ACCOUNT_SIGN_IN, onEngineMachine() ? { browser: true } : {});
    const url = safeUrl(progress?.url);
    if (pending && url) {
      pending.opener = null;
      pending.location.href = url;
    } else pending?.close();
    return null;
  } catch (error) {
    pending?.close();
    return error instanceof Error ? error.message : "Sign-in could not start";
  }
}

export async function signOutOfAccount(): Promise<string | null> {
  try {
    await rpc.call(M.CLOUD_ACCOUNT_SIGN_OUT);
    return null;
  } catch (error) {
    return error instanceof Error ? error.message : "Sign-out failed";
  }
}

/** The account row used by Zuse Serve setup: who is signed in, and the pending device code. */
export function AccountSignIn() {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const signedIn = () => !!account().user;
  const run = async () => {
    if (busy()) return;
    setBusy(true);
    setError(await (signedIn() ? signOutOfAccount() : signInToAccount()));
    setBusy(false);
  };
  return (
    <>
      <div class="cx-screen-row" style={{ "flex-wrap": "nowrap", gap: "10px" }}>
        <Details title="Zuse account" text={account().user?.email ? maskedEmail(account().user!.email!) : "Sign in to link this computer."} />
        <Btn onClick={run} disabled={busy() || state.connection.kind !== "connected"}>
          {busy() ? "Working…" : signedIn() ? "Sign out" : "Sign in"}
        </Btn>
      </div>
      <Show when={!signedIn() && safeUrl(account().login?.url ?? undefined)}>
        {(url) => (
          <div class="cx-screen-row" style={{ "flex-wrap": "nowrap", gap: "10px" }}>
            <div class="cx-details">
              <div class="cx-details-title">Complete sign-in</div>
              <Show when={account().login?.userCode} fallback={<div class="cx-details-text">Continue in your browser.</div>}>
                {(code) => (
                  <div class="cx-details-text">
                    Confirm this code: <span class="cx-code" aria-label={`Sign-in code ${code()}`}>{code()}</span>
                  </div>
                )}
              </Show>
            </div>
            <a class="cx-btn" data-size="md" data-tone="default" href={url()} target="_blank" rel="noopener noreferrer">Open sign-in</a>
          </div>
        )}
      </Show>
      <Show when={error() ?? account().error}>{(text) => <ErrorStrip message={text()} />}</Show>
    </>
  );
}
