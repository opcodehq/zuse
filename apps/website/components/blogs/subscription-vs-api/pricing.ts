export type AgentId = "claude" | "codex" | "grok" | "kimi" | "glm";

export interface ModelRate {
	id: string;
	label: string;
	input: number;
	output: number;
	cacheRead: number;
	cacheWrite: number;
}

export interface Plan {
	id: string;
	label: string;
	price: number;
}

export interface Agent {
	id: AgentId;
	label: string;
	models: ModelRate[];
	plans: Plan[];
}

const perMillion = (input: number, output: number, cacheRead: number, cacheWrite = input) => ({
	input: input / 1e6,
	output: output / 1e6,
	cacheRead: cacheRead / 1e6,
	cacheWrite: cacheWrite / 1e6,
});

export const PRICES_CHECKED = "November 28, 2026";

export const AGENTS: Agent[] = [
	{
		id: "claude",
		label: "Claude",
		models: [
			{ id: "claude-opus-4-8", label: "Opus 4.8", ...perMillion(5, 25, 0.5, 6.25) },
			{ id: "claude-sonnet-4-6", label: "Sonnet 4.6", ...perMillion(3, 15, 0.3, 3.75) },
			{ id: "claude-fable-5", label: "Fable 5", ...perMillion(10, 50, 1, 12.5) },
		],
		plans: [
			{ id: "pro", label: "Pro", price: 20 },
			{ id: "max", label: "Max", price: 100 },
			{ id: "ultra", label: "Ultra", price: 200 },
		],
	},
	{
		id: "codex",
		label: "ChatGPT",
		models: [
			{ id: "gpt-5.5", label: "GPT-5.5", ...perMillion(5, 30, 0.5) },
			{ id: "gpt-5.3-codex", label: "GPT-5.3 Codex", ...perMillion(1.75, 14, 0.175) },
		],
		plans: [
			{ id: "plus", label: "Plus", price: 20 },
			{ id: "pro-100", label: "Pro 100", price: 100 },
			{ id: "pro-200", label: "Pro 200", price: 200 },
			{ id: "pro-500", label: "Pro 500", price: 500 },
		],
	},
	{
		id: "grok",
		label: "Grok",
		models: [
			{ id: "xai/grok-4.3", label: "Grok 4.3", ...perMillion(1.25, 2.5, 0.2) },
			{ id: "xai/grok-code-fast-1", label: "Grok Code Fast 1", ...perMillion(0.2, 1.5, 0.02) },
		],
		plans: [
			{ id: "plus", label: "Plus", price: 8 },
			{ id: "premium", label: "Premium", price: 30 },
			{ id: "super", label: "Super", price: 100 },
			{ id: "ultra", label: "Ultra", price: 200 },
		],
	},
	{
		id: "kimi",
		label: "Kimi Code",
		models: [{ id: "moonshot/kimi-k3.1-pro", label: "Kimi K3.1 Pro", ...perMillion(2, 10, 0.2, 2) }],
		plans: [
			{ id: "plus", label: "Plus", price: 19 },
			{ id: "pro", label: "Pro", price: 39 },
			{ id: "max", label: "Max", price: 99 },
			{ id: "ultra", label: "Ultra", price: 199 },
		],
	},
	{
		id: "glm",
		label: "GLM Coding Plan",
		models: [{ id: "zai/glm-5.3", label: "GLM-5.3", ...perMillion(1.4, 4.4, 0.26, 1.4) }],
		plans: [
			{ id: "lite", label: "Lite", price: 18 },
			{ id: "pro", label: "Pro", price: 80 },
			{ id: "max", label: "Max", price: 168 },
		],
	},
];

export const agentOf = (id: AgentId) => AGENTS.find((agent) => agent.id === id) ?? AGENTS[0];

export interface Workload {
	requestsPerHour: number;
	contextTokens: number;
	outputTokens: number;
	cacheRead: number;
}

export type IntensityId = "light" | "steady" | "heavy";

export const CACHE_READ = 0.96;
export const CACHE_WRITE = 0.02;

export const INTENSITIES: Record<IntensityId, { label: string; workload: Workload }> = {
	light: { label: "Light", workload: { requestsPerHour: 20, contextTokens: 50_000, outputTokens: 600, cacheRead: CACHE_READ } },
	steady: { label: "Steady", workload: { requestsPerHour: 60, contextTokens: 80_000, outputTokens: 800, cacheRead: CACHE_READ } },
	heavy: { label: "Heavy", workload: { requestsPerHour: 120, contextTokens: 120_000, outputTokens: 1_000, cacheRead: CACHE_READ } },
};

export function tokensPerHour(workload: Workload) {
	const input = workload.requestsPerHour * workload.contextTokens;
	const cacheRead = input * workload.cacheRead;
	const cacheWrite = input * Math.min(CACHE_WRITE, 1 - workload.cacheRead);
	return {
		input,
		cacheRead,
		cacheWrite,
		uncached: input - cacheRead - cacheWrite,
		output: workload.requestsPerHour * workload.outputTokens,
	};
}

export function costPerHour(rate: ModelRate, workload: Workload) {
	const tokens = tokensPerHour(workload);
	return (
		tokens.cacheRead * rate.cacheRead +
		tokens.cacheWrite * rate.cacheWrite +
		tokens.uncached * rate.input +
		tokens.output * rate.output
	);
}

export const MACHINE = {
	ratePrice: 20,
	rateMinutes: 33_600,
	standardRate: 4,
	largeRate: 8,
	trialPrice: 1,
	trialMinutes: 8_400,
	cloudPrice: 20,
	cloudMinutes: 33_600,
	maxPrice: 50,
	maxMinutes: 84_000,
	sleepAfterMinutes: 10,
	defaultCap: 25,
};

export const machineMinutes = (awakeHours: number, rate: number) => Math.round(awakeHours * 60 * rate);

export const machinePrice = (minutes: number) => (minutes * MACHINE.ratePrice) / MACHINE.rateMinutes;

export function cloudBill(minutes: number, large: boolean) {
	const cloud = { plan: "Cloud", price: MACHINE.cloudPrice, extra: machinePrice(Math.max(0, minutes - MACHINE.cloudMinutes)) };
	const max = { plan: "Cloud Max", price: MACHINE.maxPrice, extra: machinePrice(Math.max(0, minutes - MACHINE.maxMinutes)) };
	const best = large || max.price + max.extra < cloud.price + cloud.extra ? max : cloud;
	return { ...best, total: best.price + best.extra };
}

export const dollars = (value: number) =>
	value >= 100
		? `$${Math.round(value).toLocaleString("en-US")}`
		: `$${value.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;

export const compact = (value: number) =>
	value >= 1e6 ? `${(value / 1e6).toFixed(value >= 1e7 ? 0 : 1)}M` : value >= 1e3 ? `${Math.round(value / 1e3)}k` : `${Math.round(value)}`;
