import { isMarkdownPreferred } from "fumadocs-core/negotiation";

type Env = {
	ASSETS: { fetch(input: Request | URL | string): Promise<Response> };
};

const llmUserAgent =
	/(?:ChatGPT-User|GPTBot|Claude-User|ClaudeBot|Perplexity-User|PerplexityBot|Google-Extended|cohere-ai)/i;

const redirects: Record<string, string> = {
	"/": "/start",
	"/providers/gemini": "/providers/acp#gemini-cli",
	"/remote-access-redesign": "/remote",
	"/reference/bundled-skill": "/composer/skills-mcp",
	"/projects/file-includes": "/projects/worktrees",
	"/serve/status-json": "/serve/command-reference",
	"/mobile/offline-notifications": "/mobile",
	"/how-to/custom-boxd-snapshot": "/how-to/choose-cloud-provider",
	"/api-reference/webhook-signatures": "/api-reference",
};

const retiredPrefixes = [
	"/api-reference/workspaces/",
	"/api-reference/messages/",
	"/api-reference/webhooks/",
	"/api-reference/attachments/",
	"/api-reference/projects/",
	"/api-reference/providers/",
];

/** The exported Markdown file behind a request, or undefined to serve the page itself. */
function markdownAsset(request: Request, pathname: string) {
	const path = pathname.replace(/\/+$/u, "") || "/";
	if (path.endsWith(".md")) return `/api/markdown${path}`;
	if (path.startsWith("/api/markdown/")) return `${path}.md`;
	if (/\.[a-z0-9]+$/iu.test(path) || path.startsWith("/api/")) return;

	const userAgent = request.headers.get("user-agent") ?? "";
	if (!isMarkdownPreferred(request) && !llmUserAgent.test(userAgent)) return;
	return `/api/markdown${path === "/" ? "/start" : path}.md`;
}

export default {
	async fetch(request: Request, env: Env): Promise<Response> {
		const url = new URL(request.url);
		const redirect =
			redirects[url.pathname] ??
			(retiredPrefixes.some((prefix) => url.pathname.startsWith(prefix))
				? "/api-reference"
				: undefined);
		if (redirect) return Response.redirect(new URL(redirect, url).toString(), 308);

		const asset = markdownAsset(request, url.pathname);
		if (!asset) return env.ASSETS.fetch(request);

		const response = await env.ASSETS.fetch(new URL(asset, url));
		if (!response.ok) return env.ASSETS.fetch(request);

		return new Response(response.body, {
			headers: {
				"Content-Type": "text/markdown; charset=utf-8",
				"Cache-Control": "public, max-age=0, s-maxage=3600",
				"X-Content-Type-Options": "nosniff",
				Vary: "Accept, User-Agent",
			},
		});
	},
};
