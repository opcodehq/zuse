import { describe, expect, it } from "vitest";
import { isAuthRedirectUrl } from "../../../src/auth/config";

describe("auth redirect deep links", () => {
	it.each([
		"zuse://auth?code=abc&state=xyz",
		"zuse-dev://auth?code=abc",
		"zuse:///auth?code=abc",
		"zuse-dev:///auth/",
	])("recognizes %s", (url) => {
		expect(isAuthRedirectUrl(url)).toBe(true);
	});

	it.each([
		"zuse:///",
		"zuse://computers",
		"zuse://authorize",
		"https://auth.example.com/auth",
		"/auth",
		"not a URL",
	])("ignores %s", (url) => {
		expect(isAuthRedirectUrl(url)).toBe(false);
	});
});
