import { describe, expect, it } from "vitest";
import { ASK, DENIED, PHASES, stepOf } from "./cockpit-model";
import { DIFF, count } from "./phone-data";

describe("cockpit model", () => {
	it("stops once to ask, between work and the finished turn", () => {
		expect(PHASES.filter((phase) => phase.lamp === "needs")).toHaveLength(1);
		expect(PHASES[ASK].lamp).toBe("needs");
		expect(PHASES.at(-1)?.lamp).toBe("done");
	});

	it("only grows the console log as the turn goes on", () => {
		const logs = PHASES.map((phase) => phase.log);
		expect(logs).toEqual([...logs].sort((a, b) => a - b));
	});

	it("ends a denied turn without running the command", () => {
		expect(stepOf(ASK + 1, "deny")).toBe(DENIED);
		expect(stepOf(ASK + 1, "allow").id).toBe("run");
	});

	it("reports the same totals as the diff the phone opens", () => {
		const added = DIFF.reduce((sum, file) => sum + count(file, "add"), 0);
		const removed = DIFF.reduce((sum, file) => sum + count(file, "del"), 0);
		expect(PHASES.at(-1)?.readout).toContain(`+${added} −${removed} in ${DIFF.length} files`);
	});
});
