import { describe, expect, it } from "vitest";
import { CLOUD_MINUTES, CLOUD_PRICE, floorOf, memoryAt, monthOf, saving } from "./model";

describe("idle agent model", () => {
	it("returns the measured samples exactly", () => {
		expect(memoryAt(1, "native")).toBe(4.23);
		expect(memoryAt(20, "native")).toBe(8.89);
		expect(memoryAt(20, "reference")).toBe(42.97);
	});

	it("interpolates between samples", () => {
		expect(memoryAt(15, "native")).toBeCloseTo((6.53 + 8.89) / 2, 5);
		expect(memoryAt(3, "native")).toBeGreaterThan(4.45);
		expect(memoryAt(3, "native")).toBeLessThan(5.2);
	});

	it("uses the measured close value with no agents open", () => {
		expect(memoryAt(0, "native")).toBe(4.74);
		expect(memoryAt(0, "reference")).toBeNull();
	});

	it("matches the published 79% at 20 agents", () => {
		expect(Math.round(saving(20) * 100)).toBe(79);
	});

	it("splits the floor from the per-agent cost", () => {
		const native = floorOf("native");
		expect(native.first).toBe(4.23);
		expect(native.perAgent * 1024).toBeCloseTo(251, 0);
	});

	it("prices machine minutes at the plan rate", () => {
		const always = monthOf({ workHours: 24, sessions: 0, days: 30 });
		expect(always.machineMinutes).toBe(24 * 60 * 30 * 4);
		expect(always.cost).toBeCloseTo((172_800 * CLOUD_PRICE) / CLOUD_MINUTES, 5);
		const working = monthOf({ workHours: 5, sessions: 2, days: 22 });
		expect(working.machineMinutes).toBe((5 * 60 + 20) * 22 * 4);
		expect(working.machineMinutes).toBeLessThan(CLOUD_MINUTES);
	});
});
