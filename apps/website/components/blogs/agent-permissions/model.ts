export type Level = "plan" | "ask" | "auto" | "full";
export type Verdict = "runs" | "asks" | "reviewer" | "blocked" | "unavailable";
export type Reach = "read" | "edit" | "shell" | "git" | "network" | "computer";

export const LEVELS: { id: Level; label: string }[] = [
	{ id: "plan", label: "Plan mode" },
	{ id: "ask", label: "Ask for approval" },
	{ id: "auto", label: "Approve for me" },
	{ id: "full", label: "Full access" },
];

export const REACHES: { id: Reach; label: string }[] = [
	{ id: "read", label: "Read files" },
	{ id: "edit", label: "Edit files" },
	{ id: "shell", label: "Shell commands" },
	{ id: "git", label: "Git" },
	{ id: "network", label: "External tools" },
	{ id: "computer", label: "Your computer" },
];

export const VERDICT_LABEL: Record<Verdict, string> = {
	runs: "Runs",
	asks: "Asks you",
	reviewer: "Reviewer decides",
	blocked: "Blocked",
	unavailable: "Not available",
};

const NOTES: Record<Level, Record<Reach, string>> = {
	plan: {
		read: "Inspection runs. A sensitive path still asks.",
		edit: "Denied. The agent proposes instead of changing files.",
		shell: "Denied: in Zuse's agent only inspection tools run.",
		git: "Git runs through the shell, so it stops here too.",
		network: "Denied.",
		computer: "Plan mode can't call desktop tools at all.",
	},
	ask: {
		read: "Runs without a card. A sensitive path asks.",
		edit: "A card per change, or a saved rule for that exact file.",
		shell: "A card per command. A saved rule matches that exact command.",
		git: "A commit or a push is a shell command with its own card.",
		network: "Every external tool call asks.",
		computer: "Every call asks, screenshots included.",
	},
	auto: {
		read: "Runs.",
		edit: "The agent's own reviewer judges it, and can still ask you.",
		shell: "The reviewer judges each command. Escalations still ask.",
		git: "Judged like any other command.",
		network: "Judged by the reviewer.",
		computer: "Zuse wires desktop control into its own agent, which doesn't offer this level. Other agents use their own setup.",
	},
	full: {
		read: "Runs. A sensitive path still asks.",
		edit: "Runs. A write to a sensitive path still asks.",
		shell: "Runs with no card. This is not a sandbox.",
		git: "Runs, push included.",
		network: "Runs.",
		computer: "Ordinary prompts are skipped once you've trusted the desktop tool.",
	},
};

export function verdictOf(level: Level, reach: Reach, reviewer: boolean): Verdict {
	if (level === "auto" && (!reviewer || reach === "computer")) return "unavailable";
	if (reach === "read") return "runs";
	if (level === "plan") return "blocked";
	if (level === "ask") return "asks";
	if (level === "auto") return "reviewer";
	return "runs";
}

export function noteOf(level: Level, reach: Reach, reviewer: boolean) {
	if (level === "auto" && !reviewer) return "Not available for this agent: only agents that ship their own automatic reviewer offer it.";
	return NOTES[level][reach];
}

const KEYS = ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];
const EXTENSIONS = [".pem", ".key", ".p12", ".pfx"];
const FOLDERS = [".aws", ".ssh", ".netrc", ".pgpass"];

export function sensitivePart(path: string): string | null {
	for (const name of path.split(/[\\/]+/).filter(Boolean)) {
		const lower = name.toLowerCase();
		if (name === ".env" || name.startsWith(".env.")) return name;
		if (FOLDERS.includes(name)) return name;
		if (lower === "credentials" || lower.startsWith("credentials.")) return name;
		if (KEYS.some((key) => name === key || name === key + ".pub")) return name;
		if (EXTENSIONS.some((extension) => lower.endsWith(extension))) return name;
	}
	return null;
}

export type Scope = "chat" | "project";

export interface Request {
	id: string;
	tool: "edit" | "shell" | "read";
	verb: string;
	resource: string;
	forced?: string;
}

export const QUEUE: Request[] = [
	{ id: "edit", tool: "edit", verb: "Edit file", resource: "src/billing/invoice.ts" },
	{ id: "test", tool: "shell", verb: "Run command", resource: "bun test billing" },
	{ id: "env", tool: "read", verb: "Read file", resource: "apps/api/.env.local", forced: "A sensitive path always asks, and can't be saved as a rule." },
	{ id: "watch", tool: "shell", verb: "Run command", resource: "bun test billing --watch" },
	{ id: "edit-again", tool: "edit", verb: "Edit file", resource: "src/billing/invoice.ts" },
	{ id: "push", tool: "shell", verb: "Run command", resource: "git push origin billing-fix" },
];

export interface Rule {
	tool: Request["tool"];
	resource: string;
	scope: Scope;
}

export function matchOf(request: Request, rules: Rule[]): Rule | null {
	if (request.forced) return null;
	const same = (rule: Rule) => rule.tool === request.tool && rule.resource === request.resource;
	return rules.find((rule) => rule.scope === "chat" && same(rule)) ?? rules.find((rule) => rule.scope === "project" && same(rule)) ?? null;
}
