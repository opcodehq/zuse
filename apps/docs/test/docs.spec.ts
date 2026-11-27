import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

type SearchResult = {
	type: "page" | "heading" | "text";
	url: string;
};

test("how-to guide deep links render", async ({ page }) => {
	await page.goto("/how-to/local-parallel-worktrees");
	await expect(
		page.getByRole("heading", { name: "Run tasks in parallel with worktrees" }),
	).toBeVisible();
});

test("browser and orchestration guides render as linked documentation", async ({
	page,
}) => {
	await page.goto("/workspace/browser");
	await expect(
		page.getByRole("heading", { name: "In-app browser" }),
	).toBeVisible();
	await page.getByRole("link", { name: "Browser debugging" }).first().click();
	await expect(page).toHaveURL(/\/workspace\/browser-debugging/u);

	await page.goto("/composer/orchestration");
	await expect(
		page.getByRole("heading", { name: "Agent orchestration" }),
	).toBeVisible();
	await expect(
		page.getByRole("link", { name: "Run agents in parallel" }).first(),
	).toBeVisible();
	await page
		.getByRole("link", { name: "Scripting and agent tools" })
		.first()
		.click();
	await expect(page).toHaveURL(/\/serve\/agent-cli/u);
	await expect(
		page.getByRole("heading", { name: "Scripting and agent tools" }),
	).toBeVisible();
	await expect(
		page.getByText("zuse pair --json", { exact: false }).first(),
	).toBeVisible();
});

test("product links use the canonical website domain", async ({
	page,
	isMobile,
}) => {
	test.skip(isMobile, "Desktop sidebar links");
	await page.goto("/start/install");

	const sidebar = page.locator("#nd-sidebar");
	await expect(
		sidebar.getByRole("link", { name: "Open zuse.sh" }),
	).toHaveAttribute("href", "https://zuse.sh");
	await expect(
		sidebar.getByRole("link", { name: "Download Zuse" }),
	).toHaveAttribute("href", "https://zuse.sh/download");
});

test("documentation and how-to guides use separate navigation sections", async ({
	page,
	isMobile,
}) => {
	test.skip(isMobile, "Desktop sidebar section switcher");
	await page.goto("/start/first-chat");

	const sidebar = page.locator("#nd-sidebar");
	const sectionSwitcher = sidebar.locator('button[aria-haspopup="dialog"]');
	await expect(sectionSwitcher).toContainText("Documentation");
	await expect(sidebar.getByRole("link", { name: "Welcome" })).toBeVisible();
	await expect(
		sidebar.getByRole("link", { name: "Parallel worktrees" }),
	).toHaveCount(0);

	await sectionSwitcher.click();
	await page.getByRole("link", { name: /How-to guides/ }).click();
	await expect(page).toHaveURL(/\/how-to$/u);
	await expect(sectionSwitcher).toContainText("How-to guides");
	await expect(
		sidebar.getByRole("link", { name: "Parallel worktrees" }),
	).toBeVisible();
	await expect(sidebar.getByRole("link", { name: "Welcome" })).toHaveCount(0);
});

test("documentation search supports the keyboard", async ({
	page,
	isMobile,
}) => {
	test.skip(isMobile, "Mobile uses the navigation-sheet search trigger");
	await page.goto("/start/first-chat");
	await page.keyboard.press("Control+k");
	const search = page.getByRole("textbox", { name: "Search" });
	await search.fill("zuse.json");
	const result = page
		.getByRole("button", { name: /Share actions with zuse\.json/ })
		.first();
	await expect(result).toBeVisible();
	await result.click();
	await expect(page).toHaveURL(/\/projects\/repository-settings/u);
});

test("documentation search ships a static page and heading index", async ({
	request,
}) => {
	const index = await request.get("/api/search");
	expect(index.ok()).toBeTruthy();
	expect(index.headers()["content-type"]).toContain("application/json");
	const body = await index.text();
	expect(body).toContain("/projects/repository-settings");
	expect(body).toContain("/serve/agent-cli");
});

test("navigation uses flat labeled groups and a filled active row", async ({
	page,
	isMobile,
}) => {
	test.skip(isMobile, "Desktop sidebar state");
	await page.goto("/concepts/local-first");

	const sidebar = page.locator("#nd-sidebar");
	await expect(sidebar.getByText("Concepts", { exact: true })).toBeVisible();
	await expect(sidebar.getByRole("button", { name: "Concepts" })).toHaveCount(
		0,
	);

	const activeLink = sidebar.locator("a[data-active='true']");
	await expect(activeLink).toHaveText("Local-first data");
	const activeBackground = await activeLink.evaluate(
		(element) => getComputedStyle(element).backgroundColor,
	);
	expect(activeBackground).not.toBe("rgba(0, 0, 0, 0)");

	await page.keyboard.press("Control+k");
	await page.getByRole("textbox", { name: "Search" }).fill("worktree");
	const selectedResult = page.locator(
		"[role='dialog'] button[aria-selected='true']",
	);
	await expect(selectedResult).toBeVisible();
	await expect(selectedResult).toHaveCSS("box-shadow", "none");
});

test("theme choice persists", async ({ page }) => {
	await page.goto("/remote");
	if (await page.getByRole("button", { name: "Open Sidebar" }).isVisible()) {
		await page.getByRole("button", { name: "Open Sidebar" }).click();
	}
	await page.getByRole("button", { name: "Toggle Theme" }).click();
	const theme = await page.locator("html").getAttribute("class");
	await page.reload();
	await expect(page.locator("html")).toHaveAttribute("class", theme ?? "light");
});

test("page actions expose Markdown and AI readers", async ({
	page,
	isMobile,
}) => {
	await page.goto("/serve/command-reference");
	if (isMobile) await page.locator("[data-toc-popover-trigger]").click();
	await expect(
		page.getByRole("button", { name: "Copy Markdown" }),
	).toBeVisible();
	await page.getByRole("button", { name: "Open", exact: true }).click();
	await expect(
		page.getByRole("link", { name: /View as Markdown/ }),
	).toBeVisible();
	await expect(
		page.getByRole("link", { name: /Open in ChatGPT/ }),
	).toBeVisible();
	await expect(
		page.getByRole("link", { name: /Open in Claude/ }),
	).toBeVisible();
});

test("Markdown responses and missing routes are explicit", async ({
	request,
}) => {
	const markdown = await request.get("/serve/command-reference.md");
	expect(markdown.ok()).toBeTruthy();
	expect(markdown.headers()["content-type"]).toContain("text/markdown");
	expect(await markdown.text()).toContain("zuse serve [--listen <addr>]");
	const missing = await request.get("/not-a-public-route");
	expect(missing.status()).toBe(404);
});

test("LLM requests receive Markdown without changing the page URL", async ({
	request,
}) => {
	const negotiated = await request.get("/remote", {
		headers: { Accept: "text/markdown" },
	});
	expect(negotiated.ok()).toBeTruthy();
	expect(negotiated.headers()["content-type"]).toContain("text/markdown");
	expect(negotiated.headers().vary).toContain("Accept");
	expect(await negotiated.text()).toContain("# Remote access");

	const root = await request.get("/", {
		headers: { Accept: "text/markdown" },
	});
	expect(root.ok()).toBeTruthy();
	expect(root.headers()["content-type"]).toContain("text/markdown");
	expect(await root.text()).toContain("# Build with Zuse");

	const crawler = await request.get("/serve", {
		headers: { "User-Agent": "GPTBot/1.0" },
	});
	expect(crawler.headers()["content-type"]).toContain("text/markdown");
	expect(await crawler.text()).toContain("# zuse serve");

	const index = await request.get("/llms.txt");
	expect(index.ok()).toBeTruthy();
	expect(index.headers()["content-type"]).toContain("text/markdown");
	const indexText = await index.text();
	expect(indexText).toContain("[Command reference]");
	expect(indexText).toContain(
		"https://docs.zuse.sh/serve/command-reference.md",
	);

	const indexedPage = await request.get("/serve/command-reference.md");
	expect(indexedPage.headers()["content-type"]).toContain("text/markdown");

	const fullIndex = await request.get("/llms-full.txt");
	expect(fullIndex.ok()).toBeTruthy();
	expect(await fullIndex.text()).toContain("# Add an MCP server");

	const browser = await request.get("/remote", {
		headers: { Accept: "text/html" },
	});
	expect(browser.headers()["content-type"]).toContain("text/html");
});

test("representative article has no automatic accessibility violations", async ({
	page,
}) => {
	await page.goto("/serve/command-reference");
	const results = await new AxeBuilder({ page: page as never }).analyze();
	expect(results.violations).toEqual([]);
});

test("mobile navigation contains search", async ({ page, isMobile }) => {
	test.skip(!isMobile, "Mobile-only navigation behavior");
	await page.goto("/remote");
	await page.getByRole("button", { name: "Open Sidebar" }).click();
	await expect(
		page.locator("button:has(> svg.lucide-chevrons-up-down)"),
	).toContainText("Remote and Serve");
	await page.getByRole("button", { name: "Open Sidebar" }).last().click();
	await page.getByRole("button", { name: "Open Search" }).click();
	await expect(page.getByRole("textbox", { name: "Search" })).toBeVisible();
});

test("API reference covers the control API", async ({ page }) => {
	await page.goto("/api-reference");
	await expect(
		page.getByRole("heading", { name: "Discovery file" }).first(),
	).toBeVisible();
});

test("OpenAPI document and retired provider routes are served", async ({
	request,
}) => {
	const document = await request.get("/openapi.json");
	expect(document.ok()).toBeTruthy();
	const spec = (await document.json()) as {
		openapi: string;
	};
	expect(spec.openapi).toBe("3.1.0");

	const gemini = await request.get("/providers/gemini", { maxRedirects: 0 });
	expect(gemini.status()).toBe(308);
	expect(gemini.headers().location).toMatch(/\/providers\/acp#gemini-cli$/u);
});
