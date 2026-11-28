import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { AGENTS, INTENSITIES, MACHINE, cloudBill, costPerHour, machineMinutes, agentOf } from "./pricing";

const data = (name: string) =>
	JSON.parse(readFileSync(new URL(`../../../../../crates/usage/data/${name}`, import.meta.url), "utf8")) as Record<string, Record<string, number>>;
const catalogue = { ...data("model-prices.json"), ...data("provider-prices.json") };
const billing = readFileSync(new URL("../../../../../crates/protocol/src/cloud_billing.rs", import.meta.url), "utf8");

describe("subscription vs API pricing", () => {
	it("uses the token rates Zuse ships in its price catalogue", () => {
		for (const agent of AGENTS) {
			for (const model of agent.models) {
				const entry = catalogue[model.id];
				expect(entry, model.id).toBeDefined();
				expect(model.input).toBeCloseTo(entry.input_cost_per_token, 12);
				expect(model.output).toBeCloseTo(entry.output_cost_per_token, 12);
				expect(model.cacheRead).toBeCloseTo(entry.cache_read_input_token_cost, 12);
				expect(model.cacheWrite).toBeCloseTo(entry.cache_creation_input_token_cost ?? entry.input_cost_per_token, 12);
			}
		}
	});

	it("matches the machine-minute rate in the billing contract", () => {
		expect(billing).toContain("RATE_MICROS: i64 = 20_000_000");
		expect(billing).toContain("RATE_MACHINE_MINUTES: i64 = 33_600");
		expect(billing).toContain("DEFAULT_RATE: u32 = 4");
		expect(MACHINE.maxMinutes).toBe((MACHINE.maxPrice / MACHINE.ratePrice) * MACHINE.rateMinutes);
		expect(MACHINE.trialMinutes).toBe(MACHINE.cloudMinutes / 4);
		expect(machineMinutes(140, MACHINE.standardRate)).toBe(MACHINE.cloudMinutes);
	});

	it("prices a heavy month of three Opus agents near seven thousand dollars", () => {
		const opus = agentOf("claude").models[0];
		const month = costPerHour(opus, INTENSITIES.heavy.workload) * 3 * 8 * 22;
		expect(month).toBeGreaterThan(6_500);
		expect(month).toBeLessThan(7_500);
	});

	it("bills cloud time at one rate past the allowance", () => {
		expect(cloudBill(10_000, false).total).toBe(20);
		expect(cloudBill(67_200, false).total).toBeCloseTo(40, 6);
		expect(cloudBill(10_000, true)).toMatchObject({ plan: "Cloud Max", total: 50 });
	});
});
