import { createSignal } from "solid-js";

// Set only by the server’s isolated browser-controller bootstrap.
export const [hostedBrowser, setHostedBrowser] = createSignal(false);

// Owner access: a #token= link becomes an HttpOnly cookie, then the link is cleared.

/** Exchange a #token= fragment for the session cookie. Returns false when the token was rejected. */
export async function unlockFromLink(location: Location = window.location): Promise<boolean> {
  const token = location.hash.startsWith("#token=") ? decodeURIComponent(location.hash.slice(7)) : null;
  if (!token) return true;
  history.replaceState(null, "", location.pathname + location.search);
  const response = await fetch("/api/session", {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ token }),
  });
  return response.ok;
}

export type SessionCheck = "owner" | "signedOut" | "noOwnerSessions" | "unreachable";

/** Whether this origin offers an owner session, and if so whether the cookie is valid. */
export async function checkSession(): Promise<SessionCheck> {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), 3000);
  try {
    const response = await fetch("/api/session/check", {
      method: "POST",
      credentials: "same-origin",
      signal: controller.signal,
    });
    if (response.ok) {
      const body = await response.json().catch(() => null);
      setHostedBrowser(body?.controllerReady === true);
      return "owner";
    }
    if (response.status === 401 || response.status === 403) return "signedOut";
    // Listeners without owner sessions (paired-only, relay) answer 404/405.
    if (response.status === 404 || response.status === 405) return "noOwnerSessions";
    return "unreachable";
  } catch {
    return "unreachable";
  } finally {
    clearTimeout(timer);
  }
}

export async function signOut() {
  await fetch("/api/session", { method: "DELETE", credentials: "same-origin" });
}

/** Remove the short-lived authorization code before analytics or UI startup. */
export function takeHostedCallback(): { code: string; state: string } | null {
  const url = new URL(location.href);
  if (url.pathname !== "/app/" || !url.searchParams.has("code") || !url.searchParams.has("state")) return null;
  const code = url.searchParams.get("code")!;
  const state = url.searchParams.get("state")!;
  url.searchParams.delete("code");
  url.searchParams.delete("state");
  history.replaceState(null, "", url.pathname + url.search + url.hash);
  return { code, state };
}
