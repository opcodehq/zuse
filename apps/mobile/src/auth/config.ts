import {
	PRODUCTION_API_URL,
	STAGING_API_URL,
	WORKOS_PUBLIC_CLIENT_ID,
	WORKOS_STAGING_PUBLIC_CLIENT_ID,
} from "@zuse/contracts";

/**
 * Cloud-auth configuration, read from Expo public env vars. Set these in the
 * app's `.env` / EAS secrets:
 *   EXPO_PUBLIC_WORKOS_CLIENT_ID   — the same WorkOS client the desktop uses
 *   EXPO_PUBLIC_ZUSE_API_URL     — the deployed api base URL
 */
export const WORKOS_API = "https://api.workos.com";

/** Path of the WorkOS sign-in redirect, e.g. `zuse://auth?code=…`. */
export const AUTH_REDIRECT_PATH = "auth";

/**
 * The browser auth session consumes its own redirect. Android also delivers it
 * to the app as a deep link, which must not navigate.
 */
export const isAuthRedirectUrl = (url: string): boolean => {
	try {
		const parsed = new URL(url);
		if (parsed.protocol !== "zuse:" && parsed.protocol !== "zuse-dev:")
			return false;
		return (
			parsed.hostname === AUTH_REDIRECT_PATH ||
			(parsed.hostname === "" &&
				parsed.pathname.replace(/^\/+|\/+$/g, "") === AUTH_REDIRECT_PATH)
		);
	} catch {
		return false;
	}
};

export const defaultWorkosClientId = (development: boolean): string =>
	development ? WORKOS_STAGING_PUBLIC_CLIENT_ID : WORKOS_PUBLIC_CLIENT_ID;

export const workosClientId = (): string =>
	process.env.EXPO_PUBLIC_WORKOS_CLIENT_ID ??
	defaultWorkosClientId(typeof __DEV__ !== "undefined" && __DEV__);

export const defaultApiBaseUrl = (development: boolean): string =>
	development ? STAGING_API_URL : PRODUCTION_API_URL;

export const apiBaseUrl = (): string =>
	(
		process.env.EXPO_PUBLIC_ZUSE_API_URL ??
		defaultApiBaseUrl(typeof __DEV__ !== "undefined" && __DEV__)
	).replace(/\/$/, "");

/** App deep-link scheme injected by the active Expo/EAS build profile. */
export const APP_SCHEME = process.env.EXPO_PUBLIC_APP_SCHEME ?? "zuse";
