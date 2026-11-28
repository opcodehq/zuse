export interface Usage {
	uncached: number;
	cacheWrite: number;
	cacheRead: number;
	output: number;
}

export interface Rate {
	input: number;
	output: number;
	cacheWrite: number;
	cacheRead: number;
}

export const SESSION_MODEL = "claude-sonnet-5-5";
export const PRICED_AS = { id: "claude-sonnet-4-6", label: "Sonnet 4.6" };

export const RATE: Rate = {
	input: 3 / 1e6,
	output: 15 / 1e6,
	cacheWrite: 3.75 / 1e6,
	cacheRead: 0.3 / 1e6,
};

export const REQUESTS: Usage[] = [
	{ uncached: 4, cacheWrite: 766, cacheRead: 4455, output: 51 },
	{ uncached: 2, cacheWrite: 444, cacheRead: 5221, output: 51 },
	{ uncached: 2, cacheWrite: 229, cacheRead: 5665, output: 339 },
	{ uncached: 4, cacheWrite: 1225, cacheRead: 5894, output: 183 },
	{ uncached: 4, cacheWrite: 851, cacheRead: 7119, output: 162 },
];

export const FINAL_REPORT = { before: 302, after: 129 };


export type BinId = "prompt" | "tools" | "conversation" | "work" | "report";
export type FixId = "prefix" | "failFast" | "grouped" | "fiveLines";
export type Fixes = Record<FixId, boolean>;

export const BINS: { id: BinId; label: string; short: string }[] = [
	{ id: "prompt", label: "Instructions and tool schemas", short: "Prompt" },
	{ id: "tools", label: "Tool output", short: "Tool output" },
	{ id: "conversation", label: "Task and earlier turns", short: "History" },
	{ id: "work", label: "Reasoning, tool calls and edits", short: "Reasoning & edits" },
	{ id: "report", label: "Final report", short: "Final report" },
];

export const FIXES: { id: FixId; label: string; short: string }[] = [
	{ id: "prefix", label: "Stable prefix", short: "Prefix" },
	{ id: "failFast", label: "Fail fast on blind writes", short: "Fail fast" },
	{ id: "grouped", label: "Edits and checks in one reply", short: "One reply" },
	{ id: "fiveLines", label: "Five-line final report", short: "Five lines" },
];

export const ALL_ON: Fixes = { prefix: true, failFast: true, grouped: true, fiveLines: true };
export const ALL_OFF: Fixes = { prefix: false, failFast: false, grouped: false, fiveLines: false };

const sum = (values: number[]) => values.reduce((total, value) => total + value, 0);
const PREFIX = REQUESTS[0].cacheRead;

interface Slice {
	read: number;
	write: number;
	fresh: number;
	output: number;
}

const empty = (): Slice => ({ read: 0, write: 0, fresh: 0, output: 0 });

function measured(): Record<BinId, Slice> {
	const bins: Record<BinId, Slice> = { prompt: empty(), tools: empty(), conversation: empty(), work: empty(), report: empty() };
	REQUESTS.forEach((request, index) => {
		bins.prompt.read += PREFIX;
		bins.conversation.read += request.cacheRead - PREFIX;
		bins.conversation.fresh += request.uncached;
		const echoed = index === 0 ? request.cacheWrite : REQUESTS[index - 1].output;
		bins.conversation.write += echoed;
		bins.tools.write += request.cacheWrite - echoed;
		if (index === REQUESTS.length - 1) bins.report.output += request.output;
		else bins.work.output += request.output;
	});
	return bins;
}

export const MEASURED = measured();

export const MEAN_REQUEST: Usage = {
	uncached: sum(REQUESTS.map((request) => request.uncached)) / REQUESTS.length,
	cacheWrite: sum(REQUESTS.map((request) => request.cacheWrite)) / REQUESTS.length,
	cacheRead: sum(REQUESTS.map((request) => request.cacheRead)) / REQUESTS.length,
	output: sum(REQUESTS.map((request) => request.output)) / REQUESTS.length,
};

export function sliceCost(slice: Slice, cached: boolean, rate: Rate = RATE) {
	const input = slice.read + slice.write + slice.fresh;
	if (!cached) return input * rate.cacheWrite + slice.output * rate.output;
	return slice.read * rate.cacheRead + slice.write * rate.cacheWrite + slice.fresh * rate.input + slice.output * rate.output;
}

export function usageCost(usage: Usage, cached = true, rate: Rate = RATE) {
	return sliceCost({ read: usage.cacheRead, write: usage.cacheWrite, fresh: usage.uncached, output: usage.output }, cached, rate);
}

export function session(fixes: Fixes) {
	const bins = structuredClone(MEASURED);
	const extra = (fixes.failFast ? 0 : 1) + (fixes.grouped ? 0 : 1);
	bins.prompt.read += extra * PREFIX;
	bins.conversation.read += extra * (MEAN_REQUEST.cacheRead - PREFIX);
	bins.conversation.fresh += extra * MEAN_REQUEST.uncached;
	bins.tools.write += extra * MEAN_REQUEST.cacheWrite;
	bins.work.output += extra * MEAN_REQUEST.output;
	if (!fixes.fiveLines) bins.report.output += FINAL_REPORT.before - FINAL_REPORT.after;
	const rows = BINS.map((bin) => {
		const slice = bins[bin.id];
		return { ...bin, slice, tokens: slice.read + slice.write + slice.fresh + slice.output, cost: sliceCost(slice, fixes.prefix) };
	});
	return {
		rows,
		requests: REQUESTS.length + extra,
		tokens: sum(rows.map((row) => row.tokens)),
		cost: sum(rows.map((row) => row.cost)),
	};
}

export const cents = (dollars: number) => `${(dollars * 100).toFixed(2)}¢`;
export const perThousand = (dollars: number) => `$${(dollars * 1000).toFixed(2)}`;
export const tokens = (count: number) => (count >= 1000 ? `${(count / 1000).toFixed(1)}k` : `${Math.round(count)}`);
