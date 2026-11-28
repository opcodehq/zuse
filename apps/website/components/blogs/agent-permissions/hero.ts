import type { ModuleId } from "./geometry";

export type Flag = "down" | "half" | "up";
export const NOTCHES = ["Ask for approval", "Approve for me", "Full access"] as const;

export interface Bench {
	notch: number;
	plan: boolean;
}

export const REST: Bench = { notch: 0, plan: false };

const NAMES: Record<ModuleId, string> = { read: "reading", edit: "edits", shell: "shell and git", network: "external tools" };

export function moduleState(id: ModuleId, { notch, plan }: Bench): { lit: boolean; flag: Flag } {
	if (id === "read") return { lit: true, flag: "down" };
	if (plan) return { lit: false, flag: "down" };
	if (notch === 0) return { lit: false, flag: "up" };
	if (notch === 1) return { lit: true, flag: "half" };
	return { lit: true, flag: "down" };
}

export function benchText(bench: Bench) {
	const level = bench.plan ? "Plan mode" : NOTCHES[bench.notch];
	const rest = bench.plan ? "edits, shell and tools blocked" : bench.notch === 0 ? "edits, shell and tools ask" : bench.notch === 1 ? "the reviewer judges the rest" : "the rest runs";
	return { readout: level + " · " + rest, valuetext: level + ": reading runs, " + rest + ", secrets always ask" };
}

export const MODULE_NAMES = NAMES;

export const CAPTION = "One lamp per kind of action: lit means it runs without a card. A flag raised out of its lid means a card comes to you; at half-mast the agent's own reviewer decides and can still send one. The key picks the access level. The lever drops a lockout bar over everything but reading, which is Plan mode, a separate switch from the key. The padlocked box on the right is sensitive paths, sign-ins and leaving Plan mode: its flag never comes down. The middle notch exists only on agents that ship their own automatic reviewer.";
