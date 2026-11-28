import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { SCROLL_1500, TYPING, WORKLOADS } from "./bench";

interface Side {
	launch: number;
	memory: number;
	cpu: number;
	lagP50: number;
	lagP95: number;
	open: number | null;
	scrollSlow: number | null;
	scrollMax: number | null;
}
interface Row {
	id: string;
	zuse: Side;
	zuseFixedCpu: number | null;
	t3: Side;
}

const results = JSON.parse(
	readFileSync(join(__dirname, "../../../../../internal-docs/benchmarks/desktop-2026-11-14/results.json"), "utf8"),
) as { rows: Row[] };
const row = (id: string) => results.rows.find((entry) => entry.id === id)!;

describe("desktop benchmark figures", () => {
	it("match the recorded medians", () => {
		for (const workload of WORKLOADS) {
			const measured = row(workload.id);
			expect(workload.native.ready).toBeCloseTo(measured.zuse.launch / 1000, 2);
			expect(workload.electron.ready).toBeCloseTo(measured.t3.launch / 1000, 2);
			expect(workload.native.memory).toBe(Math.round(measured.zuse.memory));
			expect(workload.electron.memory).toBe(Math.round(measured.t3.memory));
			expect(workload.native.cpu).toBeCloseTo(measured.zuseFixedCpu ?? measured.zuse.cpu, 1);
			expect(workload.electron.cpu).toBeCloseTo(measured.t3.cpu, 1);
		}
	});

	it("keep typing lag and the 1,500-message scroll as measured", () => {
		const smoke = row("smoke");
		expect(TYPING.native.p50).toBe(Math.round(smoke.zuse.lagP50));
		expect(TYPING.native.p95).toBe(Math.round(smoke.zuse.lagP95));
		expect(TYPING.electron.p50).toBe(Math.round(smoke.t3.lagP50));
		expect(TYPING.electron.p95).toBe(Math.round(smoke.t3.lagP95));
		const chat = row("mid1500");
		expect(SCROLL_1500.native.open).toBe(Math.round(chat.zuse.open!));
		expect(SCROLL_1500.electron.open).toBe(Math.round(chat.t3.open!));
		expect(SCROLL_1500.native.longest).toBe(Math.round(chat.zuse.scrollMax!));
		expect(SCROLL_1500.electron.longest).toBe(Math.round(chat.t3.scrollMax!));
		expect(SCROLL_1500.native.slow).toBe(chat.zuse.scrollSlow);
		expect(SCROLL_1500.electron.slow).toBe(chat.t3.scrollSlow);
	});
});
