// The connection-loss cue: desktop's Attention also covers a degraded connection. Session chimes and banners
// belong to Settings (features/settings/alerts.ts); this reads the same preference keys so one switch governs both.
import { createEffect, createRoot, on } from "solid-js";
import { state } from "@/core/store";
import { pref } from "@/features/settings/prefs";
import { playSound } from "./sounds";

const CONNECTION_GRACE_MS = 10_000;
const ATTENTION_GAP_MS = 30_000;
let lastAttention = 0;

function warn() {
  if (pref("sessionSounds") && pref("attentionSound") && Date.now() - lastAttention >= ATTENTION_GAP_MS) {
    lastAttention = Date.now();
    playSound("attention");
  }
  if (!pref("browserNotifications") || typeof Notification === "undefined" || Notification.permission !== "granted") return;
  if (pref("notifyInBackgroundOnly") && document.visibilityState === "visible" && document.hasFocus()) return;
  try {
    const banner = new Notification("Connection unavailable", {
      body: navigator.onLine ? "Zuse is trying to reconnect." : "This device is offline.",
      tag: "zuse-connection",
      silent: true,
    });
    banner.onclick = () => {
      window.focus();
      banner.close();
    };
  } catch {
    // Some browsers only allow banners from a service worker; the sound still covers it.
  }
}

/** A blip inside the grace period stays silent; a reconnect never sounds. */
export function startNotifications() {
  return createRoot((dispose) => {
    let lostTimer: ReturnType<typeof setTimeout> | undefined;
    createEffect(
      on(
        () => state.connection.kind,
        (kind, previous) => {
          if (kind === "connected") {
            clearTimeout(lostTimer);
            lostTimer = undefined;
            return;
          }
          if (previous !== "connected" || kind !== "reconnecting" || lostTimer) return;
          lostTimer = setTimeout(() => {
            lostTimer = undefined;
            if (state.connection.kind === "reconnecting") warn();
          }, CONNECTION_GRACE_MS);
        },
      ),
    );
    return () => {
      clearTimeout(lostTimer);
      dispose();
    };
  });
}
