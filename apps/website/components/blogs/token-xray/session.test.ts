import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { ALL_OFF, ALL_ON, FINAL_REPORT, MEASURED, PRICED_AS, RATE, REQUESTS, SESSION_MODEL, session } from "./session";

const read = (path: string) => readFileSync(new URL(`../../../../../${path}`, import.meta.url), "utf8");
const prices = JSON.parse(read("crates/usage/data/model-prices.json")) as Record<string, Record<string, number>>;
const providerPrices = JSON.parse(read("crates/usage/data/provider-prices.json")) as Record<string, unknown>;
const benchmark = JSON.parse(read("internal-docs/benchmarks/harness-followup-2026-10-28.json")) as {
	runs: { arm: string; model: string; case: string; repeat: number; success: boolean; requests: { usage: Record<string, number> }[] }[];
};

describe("token x-ray session", () => {
	it("replays a real native run from the benchmark suite", () => {
		const run = benchmark.runs.find((entry) => entry.arm === "native" && entry.model === SESSION_MODEL && entry.case === "boundary-fix" && entry.repeat === 0);
		expect(run?.success).toBe(true);
		expect(run?.requests.map((request) => request.usage)).toEqual(
			REQUESTS.map((request) => ({
				input_tokens: request.uncached,
				cache_creation_input_tokens: request.cacheWrite,
				cache_read_input_tokens: request.cacheRead,
				output_tokens: request.output,
			})),
		);
	});

	it("prices with the bundled table, which does not list the session's model", () => {
		const entry = prices[PRICED_AS.id];
		expect(RATE.input).toBeCloseTo(entry.input_cost_per_token, 12);
		expect(RATE.output).toBeCloseTo(entry.output_cost_per_token, 12);
		expect(RATE.cacheWrite).toBeCloseTo(entry.cache_creation_input_token_cost, 12);
		expect(RATE.cacheRead).toBeCloseTo(entry.cache_read_input_token_cost, 12);
		const keys = [...Object.keys(prices), ...Object.keys(providerPrices)].map((key) => key.toLowerCase());
		expect(keys.some((key) => key.includes(SESSION_MODEL) || SESSION_MODEL.includes(key))).toBe(false);
	});

	it("matches the final-report and guidance numbers in the harness docs", () => {
		const doc = read("internal-docs/harness-tool-efficiency.md");
		expect(doc).toContain(`${FINAL_REPORT.before} to ${FINAL_REPORT.after} tokens`);
		expect(read("crates/harness/src/native_tools/schema.rs")).toContain("Routine final status should fit within five lines");
	});

	it("keeps every measured token in exactly one bin", () => {
		const counted = Object.values(MEASURED).reduce((total, slice) => total + slice.read + slice.write + slice.fresh + slice.output, 0);
		const provider = REQUESTS.reduce((total, request) => total + request.uncached + request.cacheWrite + request.cacheRead + request.output, 0);
		expect(counted).toBe(provider);
		expect(session(ALL_ON).tokens).toBe(provider);
		expect(Object.values(MEASURED).every((slice) => slice.read >= 0 && slice.write >= 0)).toBe(true);
	});

	it("costs 3.35 cents with every fix and 18.65 cents with none", () => {
		expect(session(ALL_ON).cost * 100).toBeCloseTo(3.35, 2);
		expect(session(ALL_OFF).cost * 100).toBeCloseTo(18.65, 2);
		expect(session({ ...ALL_ON, prefix: false }).cost * 100).toBeCloseTo(13.14, 2);
		expect(session(ALL_OFF).requests).toBe(REQUESTS.length + 2);
	});
});
