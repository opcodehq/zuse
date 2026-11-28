import { describe, expect, it } from "vitest";
import { QUEUE, matchOf, sensitivePart, verdictOf } from "./model";

describe("sensitivePart", () => {
	it("matches the engine's sensitive path rule", () => {
		for (const path of [".env", "apps/api/.env.local", "~/.ssh/config", "home/.aws/config", ".netrc", ".pgpass", "Credentials", "credentials.json", "id_ed25519", "id_rsa.pub", "certs/server.PEM", "tls.key", "store.p12", "cert.pfx"]) {
			expect(sensitivePart(path), path).not.toBeNull();
		}
	});

	it("leaves ordinary paths alone", () => {
		for (const path of ["src/env.ts", ".envrc", "docs/credential-guide.md", "id_rsa_backup", "keys/readme.md", "src/billing/invoice.ts"]) {
			expect(sensitivePart(path), path).toBeNull();
		}
	});
});

describe("verdictOf", () => {
	it("keeps reads running at every level that exists", () => {
		expect(verdictOf("plan", "read", false)).toBe("runs");
		expect(verdictOf("ask", "read", false)).toBe("runs");
		expect(verdictOf("full", "read", false)).toBe("runs");
	});

	it("offers Approve for me only to agents with their own reviewer", () => {
		expect(verdictOf("auto", "shell", false)).toBe("unavailable");
		expect(verdictOf("auto", "shell", true)).toBe("reviewer");
	});

	it("blocks writes in plan mode and runs them in full access", () => {
		expect(verdictOf("plan", "edit", false)).toBe("blocked");
		expect(verdictOf("ask", "edit", false)).toBe("asks");
		expect(verdictOf("full", "git", false)).toBe("runs");
	});
});

describe("matchOf", () => {
	it("matches the exact resource only and never a forced prompt", () => {
		const [edit, test, env, watch, again] = QUEUE;
		const rules = [
			{ tool: test.tool, resource: test.resource, scope: "project" as const },
			{ tool: edit.tool, resource: edit.resource, scope: "chat" as const },
			{ tool: env.tool, resource: env.resource, scope: "project" as const },
		];
		expect(matchOf(watch, rules)).toBeNull();
		expect(matchOf(again, rules)?.scope).toBe("chat");
		expect(matchOf(env, rules)).toBeNull();
	});
});
