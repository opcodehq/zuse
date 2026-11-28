"use client";

import * as React from "react";
import { HEAVY_MONTH, totals, update, useCalc, type Calc } from "./calc-store";
import { AGENTS, INTENSITIES, MACHINE, PRICES_CHECKED, cloudBill, compact, costPerHour, dollars, machineMinutes, tokensPerHour, type AgentId, type IntensityId } from "./pricing";

function Panel({ label, children }: { label: string; children: React.ReactNode }) {
	return (
		<div className="not-prose my-8 border border-dotted border-border bg-card/60 p-4 font-mono text-xs leading-5 text-muted-foreground sm:p-5" role="group" aria-label={label}>
			<p className="editorial-label mb-4">{label}</p>
			{children}
		</div>
	);
}

function Big({ value, caption, strong = false }: { value: string; caption: string; strong?: boolean }) {
	return (
		<div className="min-w-0">
			<p className={`text-2xl tabular-nums tracking-tight sm:text-3xl ${strong ? "text-foreground" : "text-muted-foreground"}`}>{value}</p>
			<p className="mt-1 text-[11px] uppercase tracking-[0.1em]">{caption}</p>
		</div>
	);
}

function Slider({ label, value, min, max, step, format, onChange }: { label: string; value: number; min: number; max: number; step: number; format: (value: number) => string; onChange: (value: number) => void }) {
	const id = React.useId();
	return (
		<div className="grid gap-1.5">
			<div className="flex items-baseline justify-between gap-3">
				<label htmlFor={id}>{label}</label>
				<output htmlFor={id} className="tabular-nums text-foreground">
					{format(value)}
				</output>
			</div>
			<input id={id} type="range" min={min} max={max} step={step} value={value} onChange={(event) => onChange(Number(event.target.value))} className="h-5 w-full cursor-pointer accent-[var(--foreground)]" />
		</div>
	);
}

function Segmented<T extends string>({ label, value, options, onChange }: { label: string; value: T | string; options: { id: T; label: string }[]; onChange: (value: T) => void }) {
	return (
		<div role="radiogroup" aria-label={label} className="flex flex-wrap gap-1.5">
			{options.map((option) => (
				<button
					key={option.id}
					type="button"
					role="radio"
					aria-checked={value === option.id}
					onClick={() => onChange(option.id)}
					className={`border px-2.5 py-1 transition-colors ${value === option.id ? "border-foreground bg-foreground text-background" : "border-border hover:border-foreground/50 hover:text-foreground"}`}
				>
					{option.label}
				</button>
			))}
		</div>
	);
}

const INTENSITY_OPTIONS = (Object.keys(INTENSITIES) as IntensityId[]).map((id) => ({ id, label: INTENSITIES[id].label }));

const setIntensity = (id: IntensityId) => update({ intensity: id, workload: INTENSITIES[id].workload });
const setWorkload = (change: Partial<Calc["workload"]>) => update((calc) => ({ intensity: "custom", workload: { ...calc.workload, ...change } }));
const setAgent = (id: AgentId, change: Partial<Calc["agents"][AgentId]>) => update((calc) => ({ agents: { ...calc.agents, [id]: { ...calc.agents[id], ...change } } }));

const percent = (value: number) => `${Math.round(value * 100)}%`;

export function TokenHour() {
	const calc = useCalc();
	const tokens = tokensPerHour(calc.workload);
	const opus = AGENTS[0].models[0];
	const hourly = costPerHour(opus, calc.workload);
	const parts = [
		{ label: "cache reads", value: tokens.cacheRead, cost: tokens.cacheRead * opus.cacheRead, tone: "bg-foreground/25" },
		{ label: "cache writes", value: tokens.cacheWrite, cost: tokens.cacheWrite * opus.cacheWrite, tone: "bg-foreground/55" },
		{ label: "fresh input", value: tokens.uncached, cost: tokens.uncached * opus.input, tone: "bg-foreground/75" },
		{ label: "output", value: tokens.output, cost: tokens.output * opus.output, tone: "bg-foreground" },
	];
	return (
		<Panel label="One agent-hour">
			<Segmented label="Intensity" value={calc.intensity} options={INTENSITY_OPTIONS} onChange={setIntensity} />
			<div className="mt-5 grid gap-4 sm:grid-cols-2">
				<Slider label="Requests an hour" value={calc.workload.requestsPerHour} min={5} max={200} step={5} format={(value) => `${value}`} onChange={(value) => setWorkload({ requestsPerHour: value })} />
				<Slider label="Context per request" value={calc.workload.contextTokens} min={10_000} max={200_000} step={5_000} format={compact} onChange={(value) => setWorkload({ contextTokens: value })} />
				<Slider label="Output per request" value={calc.workload.outputTokens} min={100} max={4_000} step={100} format={(value) => value.toLocaleString("en-US")} onChange={(value) => setWorkload({ outputTokens: value })} />
				<Slider label="Served from cache" value={calc.workload.cacheRead} min={0.5} max={0.98} step={0.01} format={percent} onChange={(value) => setWorkload({ cacheRead: value })} />
			</div>
			<div className="mt-6 grid grid-cols-2 gap-4">
				<Big value={compact(tokens.input + tokens.output)} caption="tokens an hour" strong />
				<Big value={dollars(hourly)} caption={`an hour on ${opus.label}`} strong />
			</div>
			<div className="mt-5 grid gap-2">
				<p>Where the dollars go</p>
				<div className="flex h-2.5 w-full overflow-hidden bg-foreground/5" aria-hidden="true">
					{parts.map((part) => (
						<div key={part.label} className={part.tone} style={{ width: `${(part.cost / hourly) * 100}%` }} />
					))}
				</div>
				<ul className="grid grid-cols-2 gap-x-4 gap-y-1 sm:grid-cols-4">
					{parts.map((part) => (
						<li key={part.label} className="flex items-center gap-2">
							<span className={`size-2 shrink-0 ${part.tone}`} aria-hidden="true" />
							<span>
								{part.label} <span className="text-foreground">{dollars(part.cost)}</span>
							</span>
						</li>
					))}
				</ul>
			</div>
		</Panel>
	);
}

export function RateBoard() {
	const calc = useCalc();
	const rows = AGENTS.flatMap((agent) => agent.models.map((model) => ({ agent, model, hourly: costPerHour(model, calc.workload) }))).sort((a, b) => b.hourly - a.hourly);
	const most = rows[0].hourly;
	return (
		<Panel label="One agent-hour, model by model">
			<Segmented label="Intensity" value={calc.intensity} options={INTENSITY_OPTIONS} onChange={setIntensity} />
			<ul className="mt-5 grid gap-2.5">
				{rows.map((row) => (
					<li key={row.model.id} className="grid grid-cols-[minmax(0,9.5rem)_1fr_4.5rem] items-center gap-3">
						<span className="truncate">
							<span className="text-foreground">{row.model.label}</span>
						</span>
						<span className="h-2 bg-foreground/5" aria-hidden="true">
							<span className="block h-full bg-foreground/70 transition-[width] duration-300 motion-reduce:transition-none" style={{ width: `${Math.max(1, (row.hourly / most) * 100)}%` }} />
						</span>
						<span className="text-right tabular-nums text-foreground">{dollars(row.hourly)}</span>
					</li>
				))}
			</ul>
			<p className="mt-4">Per agent-hour at {calc.workload.requestsPerHour} requests, {compact(calc.workload.contextTokens)} context, {percent(calc.workload.cacheRead)} from cache.</p>
		</Panel>
	);
}

const selectClass = "min-w-0 border border-border bg-background px-1.5 py-1 text-foreground";

export function MonthCalculator() {
	const calc = useCalc();
	const sum = totals(calc);
	const ratio = sum.plans > 0 ? sum.api / sum.plans : 0;
	return (
		<Panel label="Your month">
			<div className="grid gap-2">
				{sum.rows.map((row) => (
					<div key={row.agent.id} className="grid grid-cols-[1fr_auto] items-center gap-x-3 gap-y-1.5 border-b border-dotted border-border pb-2 sm:grid-cols-[8rem_auto_1fr_1fr]">
						<span className={row.count ? "text-foreground" : ""}>{row.agent.label}</span>
						<span className="flex items-center gap-1.5">
							<button type="button" aria-label={`Fewer ${row.agent.label} agents`} className="size-6 border border-border hover:border-foreground/50" onClick={() => setAgent(row.agent.id, { count: Math.max(0, row.count - 1) })}>
								−
							</button>
							<span className="w-[4.75rem] whitespace-nowrap text-center tabular-nums text-foreground" aria-live="polite">
								{row.count} {row.count === 1 ? "agent" : "agents"}
							</span>
							<button type="button" aria-label={`More ${row.agent.label} agents`} className="size-6 border border-border hover:border-foreground/50" onClick={() => setAgent(row.agent.id, { count: Math.min(6, row.count + 1) })}>
								+
							</button>
						</span>
						<select aria-label={`${row.agent.label} model`} className={selectClass} value={row.model.id} onChange={(event) => setAgent(row.agent.id, { model: event.target.value })}>
							{row.agent.models.map((model) => (
								<option key={model.id} value={model.id}>
									{model.label}
								</option>
							))}
						</select>
						<select aria-label={`${row.agent.label} plan`} className={selectClass} value={row.plan.id} onChange={(event) => setAgent(row.agent.id, { plan: event.target.value })}>
							{row.agent.plans.map((plan) => (
								<option key={plan.id} value={plan.id}>
									{plan.label} · ${plan.price}/mo
								</option>
							))}
						</select>
					</div>
				))}
			</div>
			<div className="mt-5 grid gap-4 sm:grid-cols-[auto_1fr_1fr] sm:items-end">
				<Segmented label="Intensity" value={calc.intensity} options={INTENSITY_OPTIONS} onChange={setIntensity} />
				<Slider label="Hours a day" value={calc.hoursPerDay} min={1} max={16} step={1} format={(value) => `${value} h`} onChange={(value) => update({ hoursPerDay: value })} />
				<Slider label="Days a month" value={calc.days} min={1} max={31} step={1} format={(value) => `${value}`} onChange={(value) => update({ days: value })} />
			</div>
			<div className="mt-6 grid grid-cols-2 gap-4 sm:grid-cols-3">
				<Big value={dollars(sum.api)} caption="at API rates" strong />
				<Big value={dollars(sum.plans)} caption="on subscriptions" strong />
				<Big value={ratio ? `${ratio >= 10 ? Math.round(ratio) : ratio.toFixed(1)}×` : "—"} caption="difference" />
			</div>
			<div className="mt-5 flex flex-wrap items-center gap-3">
				<button type="button" className="border border-border px-2.5 py-1 hover:border-foreground/50 hover:text-foreground" onClick={() => update(HEAVY_MONTH)}>
					Load the heavy month
				</button>
				<span>Plan prices from each vendor&apos;s pricing page, {PRICES_CHECKED}.</span>
			</div>
		</Panel>
	);
}

export function BreakEven() {
	const calc = useCalc();
	const sum = totals(calc);
	const rows = (sum.active.length ? sum.active : sum.rows.slice(0, 1)).map((row) => ({ ...row, hours: row.plan.price / row.hourly }));
	return (
		<Panel label="When a plan pays for itself">
			<Segmented label="Intensity" value={calc.intensity} options={INTENSITY_OPTIONS} onChange={setIntensity} />
			<ul className="mt-5 grid gap-3">
				{rows.map((row) => {
					const used = row.count * calc.hoursPerDay * calc.days;
					return (
						<li key={row.agent.id} className="grid gap-1.5">
							<div className="flex flex-wrap items-baseline justify-between gap-2">
								<span>
									<span className="text-foreground">
										{row.agent.label} {row.plan.label}
									</span>{" "}
									· {dollars(row.plan.price)} · {row.model.label}
								</span>
								<span className="tabular-nums text-foreground">{row.hours < 10 ? row.hours.toFixed(1) : Math.round(row.hours)} agent-hours</span>
							</div>
							<span className="relative h-2 bg-foreground/5" aria-hidden="true">
								<span className="absolute inset-y-0 left-0 bg-foreground/70" style={{ width: `${Math.min(100, (row.hours / Math.max(used, row.hours)) * 100)}%` }} />
							</span>
							<span>
								Your month: {Math.round(used)} agent-hours, so the plan has paid for itself {used > row.hours ? `${Math.floor(used / row.hours)} times over` : "not yet"}.
							</span>
						</li>
					);
				})}
			</ul>
		</Panel>
	);
}

export function MachineClock() {
	const calc = useCalc();
	const sum = totals(calc);
	const computers = calc.shared ? Math.min(1, sum.agentCount) : sum.agentCount;
	const awakeHours = computers * calc.days * (calc.hoursPerDay + MACHINE.sleepAfterMinutes / 60);
	const minutes = machineMinutes(awakeHours, calc.large ? MACHINE.largeRate : MACHINE.standardRate);
	const bill = cloudBill(minutes, calc.large);
	const allowance = bill.plan === "Cloud" ? MACHINE.cloudMinutes : MACHINE.maxMinutes;
	const overCap = bill.plan === "Cloud" && bill.extra > MACHINE.defaultCap;
	return (
		<Panel label="The time clock">
			<div className="flex flex-wrap gap-4">
				<Segmented label="Where agents run" value={calc.local ? "local" : "cloud"} options={[{ id: "local", label: "Your computer" }, { id: "cloud", label: "Zuse Cloud" }]} onChange={(value) => update({ local: value === "local" })} />
				{calc.local ? null : (
					<>
						<Segmented label="Computer size" value={calc.large ? "large" : "standard"} options={[{ id: "standard", label: "Standard · 4 vCPU" }, { id: "large", label: "Large · 8 vCPU" }]} onChange={(value) => update({ large: value === "large" })} />
						<Segmented label="Computers" value={calc.shared ? "shared" : "each"} options={[{ id: "each", label: "One per chat" }, { id: "shared", label: "One shared" }]} onChange={(value) => update({ shared: value === "shared" })} />
					</>
				)}
			</div>
			{calc.local ? (
				<div className="mt-6 grid grid-cols-2 gap-4">
					<Big value="$0" caption="machine time" strong />
					<Big value={dollars(sum.plans)} caption="your subscriptions" strong />
				</div>
			) : (
				<>
					<div className="mt-6 grid grid-cols-2 gap-4 sm:grid-cols-3">
						<Big value={minutes.toLocaleString("en-US")} caption="machine minutes" strong />
						<Big value={dollars(bill.total)} caption={bill.plan} strong />
						<Big value={dollars(sum.plans + bill.total)} caption="with subscriptions" />
					</div>
					<div className="mt-5 grid gap-1.5">
						<span className="relative h-2 bg-foreground/5" aria-hidden="true">
							<span className="absolute inset-y-0 left-0 bg-foreground/70" style={{ width: `${Math.min(100, (minutes / Math.max(minutes, allowance)) * 100)}%` }} />
						</span>
						<span>
							{computers} {computers === 1 ? "computer" : "computers"} awake {calc.hoursPerDay} h a day plus a {MACHINE.sleepAfterMinutes}-minute wind-down, {calc.days} days. {bill.plan} includes {allowance.toLocaleString("en-US")} minutes
							{bill.extra > 0 ? `; the rest is ${dollars(bill.extra)} of pay-as-you-go at the same rate` : ""}.
							{overCap ? ` That is past the default $${MACHINE.defaultCap} cap, so raise it or move to Cloud Max.` : ""}
						</span>
					</div>
				</>
			)}
		</Panel>
	);
}
