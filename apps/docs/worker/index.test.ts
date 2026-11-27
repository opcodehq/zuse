import { describe, expect, it } from "vitest";
import worker from "./index";

const assets = {
	async fetch(input: Request | URL | string) {
		const path = new URL(input instanceof Request ? input.url : input).pathname;
		if (path === "/api/markdown/missing.md")
			return new Response("Not found", { status: 404 });
		if (path.startsWith("/api/markdown/")) return new Response(`markdown ${path}`);
		return new Response(`asset ${path}`);
	},
};

const get = (path: string, headers: Record<string, string> = {}) =>
	worker.fetch(new Request(`https://docs.zuse.sh${path}`, { headers }), {
		ASSETS: assets,
	});

describe("docs worker", () => {
	it("redirects the root and retired pages permanently", async () => {
		const root = await get("/");
		expect(root.status).toBe(308);
		expect(root.headers.get("location")).toBe("https://docs.zuse.sh/start");

		const gemini = await get("/providers/gemini");
		expect(gemini.headers.get("location")).toBe(
			"https://docs.zuse.sh/providers/acp#gemini-cli",
		);
	});

	it("serves page Markdown at <path>.md and the legacy route", async () => {
		const page = await get("/start/install.md");
		expect(await page.text()).toBe("markdown /api/markdown/start/install.md");
		expect(page.headers.get("content-type")).toBe("text/markdown; charset=utf-8");

		const legacy = await get("/api/markdown/start/install");
		expect(await legacy.text()).toBe("markdown /api/markdown/start/install.md");
	});

	it("negotiates Markdown for agents and Accept headers", async () => {
		const accept = await get("/start/install", { accept: "text/markdown" });
		expect(await accept.text()).toBe("markdown /api/markdown/start/install.md");
		expect(accept.headers.get("vary")).toBe("Accept, User-Agent");

		const agent = await get("/workspace/browser", { "user-agent": "ClaudeBot/1.0" });
		expect(await agent.text()).toBe("markdown /api/markdown/workspace/browser.md");
	});

	it("serves HTML to browsers", async () => {
		const page = await get("/start/install", {
			accept: "text/html,application/xhtml+xml,*/*;q=0.8",
		});
		expect(await page.text()).toBe("asset /start/install");
	});

	it("falls back to the page when no Markdown exists", async () => {
		const page = await get("/missing.md");
		expect(await page.text()).toBe("asset /missing.md");
	});
});
