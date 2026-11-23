import { hostedBrowser, takeHostedCallback } from "@/core/auth";
import { onConnected, rpc, start } from "@/core/connection";
import { M } from "@/core/methods";
import { setState, state } from "@/core/store";
import "@/core/chat";
import { view } from "@/core/view";
import { startAnalytics } from "@/core/analytics";
import { App } from "@/shell/App";
import "@/shell/shell.css";
import "@/shell/theme";
import "@/ui/primitives.css";
import "@/ui/tokens.css";
import { render } from "@solidjs/web";

const callback = takeHostedCallback();
if (callback) {
  const stop = onConnected(() => {
    stop();
    if (!hostedBrowser()) return;
    void rpc.call(M.CLOUD_ACCOUNT_SIGN_IN, callback).catch(error => {
      setState(draft => { draft.account = { error: error instanceof Error ? error.message : "Sign-in failed. Try again." }; });
    });
  });
}

// Read-only handle for the e2e harness; the page can already read its own state.
(window as unknown as { __zuse: unknown }).__zuse = { state, view };

render(() => <App />, document.getElementById("root")!);
startAnalytics();
void start();
