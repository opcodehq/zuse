export type Lamp = "working" | "needs" | "done";

export interface Phase {
	id: string;
	lamp: Lamp;
	phone: string;
	readout: string;
	log: number;
	seconds: number;
}

export const COMMAND = "bun test api/client.test.ts";

export const PHASES: Phase[] = [
	{ id: "read", lamp: "working", phone: "Reading client.ts", readout: "working · reading client.ts", log: 3, seconds: 1.6 },
	{ id: "edit", lamp: "working", phone: "Editing client.ts", readout: "working · editing client.ts", log: 6, seconds: 1.8 },
	{ id: "ask", lamp: "needs", phone: "Waiting for approval", readout: "needs you · allow " + COMMAND.split(" ").slice(0, 2).join(" ") + "?", log: 7, seconds: 3.2 },
	{ id: "run", lamp: "working", phone: "Running bun test", readout: "working · running bun test", log: 10, seconds: 1.8 },
	{ id: "done", lamp: "done", phone: "Worked for 1m 5s", readout: "done · +20 −13 in 2 files", log: 12, seconds: 3 },
];

export const DENIED: Phase = { id: "denied", lamp: "done", phone: "Worked for 48s", readout: "done · test run denied", log: 8, seconds: 3 };

export const ASK = PHASES.findIndex((phase) => phase.id === "ask");
export const LOG_LINES = 12;

export const HERO = {
	still: ASK,
	caption:
		"The console runs the agent and shows everything it does. The handheld in the cradle shows the same turn as one line, one lamp and the two keys that matter when it stops to ask. Lift it out and nothing changes but where you are. The turn is a sample; the keys and the approval panel are the ones the phone app shows.",
};

export function stepOf(index: number, decision: "allow" | "deny" | null): Phase {
	if (decision === "deny" && index > ASK) return DENIED;
	return PHASES[Math.min(index, PHASES.length - 1)];
}
