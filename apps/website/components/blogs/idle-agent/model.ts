export interface Sample {
	agents: number;
	native: number;
	reference: number;
}

export const SAMPLES: Sample[] = [
	{ agents: 1, native: 4.23, reference: 41.56 },
	{ agents: 2, native: 4.45, reference: 41.52 },
	{ agents: 5, native: 5.2, reference: 41.66 },
	{ agents: 10, native: 6.53, reference: 41.83 },
	{ agents: 20, native: 8.89, reference: 42.97 },
];

export const MOST_AGENTS = 20;
export const NATIVE_AFTER_CLOSE = 4.74;
export const QUIET_MINUTES = 10;
export const STANDARD_RATE = 4;
export const CLOUD_PRICE = 20;
export const CLOUD_MINUTES = 33_600;
export const PRICE_PER_MACHINE_MINUTE = CLOUD_PRICE / CLOUD_MINUTES;
export const DIAL_MAX = 50;

export function memoryAt(agents: number, arm: "native" | "reference") {
	if (agents <= 0) return arm === "native" ? NATIVE_AFTER_CLOSE : null;
	const count = Math.min(agents, MOST_AGENTS);
	const upper = SAMPLES.findIndex((sample) => sample.agents >= count);
	const high = SAMPLES[upper];
	if (high.agents === count || upper === 0) return high[arm];
	const low = SAMPLES[upper - 1];
	const along = (count - low.agents) / (high.agents - low.agents);
	return low[arm] + (high[arm] - low[arm]) * along;
}

export const saving = (agents: number) => 1 - (memoryAt(agents, "native") ?? 0) / (memoryAt(agents, "reference") ?? 1);

export function floorOf(arm: "native" | "reference") {
	const first = SAMPLES[0][arm];
	const last = SAMPLES[SAMPLES.length - 1];
	return { first, perAgent: (last[arm] - first) / (last.agents - SAMPLES[0].agents) };
}

export interface Month {
	workHours: number;
	sessions: number;
	days: number;
}

export function monthOf({ workHours, sessions, days }: Month) {
	const awakeMinutes = Math.min(24 * 60, workHours * 60 + sessions * QUIET_MINUTES) * days;
	const machineMinutes = awakeMinutes * STANDARD_RATE;
	const alwaysOn = 24 * 60 * days * STANDARD_RATE;
	return {
		awakeHours: awakeMinutes / 60,
		machineMinutes,
		alwaysOn,
		cost: machineMinutes * PRICE_PER_MACHINE_MINUTE,
		alwaysOnCost: alwaysOn * PRICE_PER_MACHINE_MINUTE,
		share: machineMinutes / alwaysOn,
	};
}

export const mib = (value: number) => `${value.toFixed(value < 10 ? 2 : 1)} MiB`;
export const dollars = (value: number) => `$${value < 100 ? value.toFixed(2) : Math.round(value).toLocaleString("en-US")}`;
export const count = (value: number) => Math.round(value).toLocaleString("en-US");
