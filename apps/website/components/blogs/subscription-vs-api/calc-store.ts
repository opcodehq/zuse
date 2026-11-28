"use client";

import * as React from "react";
import { AGENTS, INTENSITIES, agentOf, costPerHour, type AgentId, type IntensityId, type Workload } from "./pricing";

export interface AgentChoice {
	count: number;
	model: string;
	plan: string;
}

export interface Calc {
	intensity: IntensityId | "custom";
	workload: Workload;
	agents: Record<AgentId, AgentChoice>;
	hoursPerDay: number;
	days: number;
	local: boolean;
	large: boolean;
	shared: boolean;
}

const choice = (id: AgentId, count: number, plan: string): AgentChoice => ({ count, model: agentOf(id).models[0].id, plan });

export const DEFAULT_CALC: Calc = {
	intensity: "steady",
	workload: INTENSITIES.steady.workload,
	agents: {
		claude: choice("claude", 2, "ultra"),
		codex: choice("codex", 0, "pro-200"),
		grok: choice("grok", 0, "super"),
		kimi: choice("kimi", 0, "max"),
		glm: choice("glm", 0, "pro"),
	},
	hoursPerDay: 6,
	days: 22,
	local: false,
	large: false,
	shared: false,
};

export const HEAVY_MONTH: Partial<Calc> = {
	intensity: "heavy",
	workload: INTENSITIES.heavy.workload,
	agents: { ...DEFAULT_CALC.agents, claude: choice("claude", 3, "ultra") },
	hoursPerDay: 8,
	days: 22,
};

let current = DEFAULT_CALC;
const listeners = new Set<() => void>();

export function update(change: Partial<Calc> | ((calc: Calc) => Partial<Calc>)) {
	current = { ...current, ...(typeof change === "function" ? change(current) : change) };
	listeners.forEach((listener) => listener());
}

export function useCalc() {
	return React.useSyncExternalStore(
		(listener) => {
			listeners.add(listener);
			return () => listeners.delete(listener);
		},
		() => current,
		() => DEFAULT_CALC,
	);
}

export function totals(calc: Calc) {
	const rows = AGENTS.map((agent) => {
		const picked = calc.agents[agent.id];
		const model = agent.models.find((entry) => entry.id === picked.model) ?? agent.models[0];
		const plan = agent.plans.find((entry) => entry.id === picked.plan) ?? agent.plans[0];
		const hourly = costPerHour(model, calc.workload);
		return { agent, model, plan, count: picked.count, hourly, api: hourly * picked.count * calc.hoursPerDay * calc.days };
	});
	const active = rows.filter((row) => row.count > 0);
	const agentCount = active.reduce((sum, row) => sum + row.count, 0);
	return {
		rows,
		active,
		agentCount,
		api: active.reduce((sum, row) => sum + row.api, 0),
		plans: active.reduce((sum, row) => sum + row.plan.price, 0),
	};
}
