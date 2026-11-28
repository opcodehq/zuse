import { INTENSITIES, MACHINE, agentOf, costPerHour, dollars, machinePrice } from "./pricing";

const opus = agentOf("claude").models[0];
const plan = agentOf("claude").plans.find((entry) => entry.id === "ultra") ?? agentOf("claude").plans[0];
const HOURS_PER_DAY = 8;
const DAYS = 22;
const AGENTS = 3;

export const heroRate = costPerHour(opus, INTENSITIES.heavy.workload);

export function heroState(agents: number, hours: number) {
	const apiCost = heroRate * agents * hours;
	const minutes = Math.round(MACHINE.standardRate * 60 * agents * hours);
	const cloud = Math.max(MACHINE.cloudPrice, machinePrice(minutes));
	const day = Math.min(DAYS, Math.floor(hours / HOURS_PER_DAY) + 1);
	return {
		apiCost,
		minutes,
		cloud,
		day,
		taxi: dollars(apiCost),
		clock: minutes.toLocaleString("en-US"),
		readout: `Day ${day} · ${dollars(apiCost)} vs ${dollars(plan.price)} + ${dollars(cloud)}`,
		valuetext: `${agents} ${agents === 1 ? "agent" : "agents"} running, day ${day} of ${DAYS}: ${dollars(apiCost)} at API rates, ${dollars(plan.price)} flat plan, ${dollars(cloud)} of cloud machine time`,
	};
}

export const HERO_MONTH = { hours: HOURS_PER_DAY * DAYS, days: DAYS, agents: AGENTS, plan: plan.price, hoursPerDay: HOURS_PER_DAY };

const end = heroState(AGENTS, HOURS_PER_DAY * DAYS);

export const HERO = {
	agents: AGENTS,
	taxi: end.taxi,
	clock: end.clock,
	plan: `${dollars(plan.price)}/mo`,
	readout: end.readout,
	caption: `Three ${opus.label} agents at a heavy pace (120 requests an hour, 120k tokens of context each) for ${HOURS_PER_DAY} hours a day, ${DAYS} days. The taxi meter charges every token at API rates and ends the month at ${end.taxi}. The flat-rate meter is the ${dollars(plan.price)} plan, and its needle never moves. The time clock stamps Zuse machine minutes, 4 per awake minute of a standard cloud computer, only while one is awake: ${end.clock} minutes, ${dollars(end.cloud)}.`,
};
