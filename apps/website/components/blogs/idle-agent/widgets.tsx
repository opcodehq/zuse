"use client";

import * as React from "react";
import { Plate } from "@/components/iso/draw";
import { useStillness } from "@/components/iso/live";
import "@/components/iso/iso.css";
import { CLOUD_MINUTES, CLOUD_PRICE, NATIVE_AFTER_CLOSE, QUIET_MINUTES, SAMPLES, STANDARD_RATE, count, dollars, floorOf, mib, monthOf } from "./model";
import "./idle-agent.css";

function Choice<T extends string | number>({ label, value, options, onChange, format }: { label: string; value: T; options: T[]; onChange: (value: T) => void; format: (value: T) => string }) {
	return (
		<div className="ia-pick" role="radiogroup" aria-label={label}>
			<span className="ia-pick-label">{label}</span>
			{options.map((option) => (
				<button key={String(option)} type="button" role="radio" aria-checked={value === option} onClick={() => onChange(option)}>
					{format(option)}
				</button>
			))}
		</div>
	);
}

function Slider({ label, value, min, max, step = 1, format, onChange }: { label: string; value: number; min: number; max: number; step?: number; format: (value: number) => string; onChange: (value: number) => void }) {
	const id = React.useId();
	return (
		<div className="ia-slider">
			<label htmlFor={id}>{label}</label>
			<input id={id} type="range" min={min} max={max} step={step} value={value} onChange={(event) => onChange(Number(event.target.value))} />
			<output htmlFor={id}>{format(value)}</output>
		</div>
	);
}

const SCALE = 45;

export function FloorChart() {
	const [agents, setAgents] = React.useState(20);
	const sample = SAMPLES.find((entry) => entry.agents === agents) ?? SAMPLES[SAMPLES.length - 1];
	const rows = (["native", "reference"] as const).map((arm) => {
		const floor = floorOf(arm).first;
		const total = sample[arm];
		return { arm, name: arm === "native" ? "Zuse agent" : "Reference server", floor, rest: Math.max(0, total - floor), total };
	});
	const native = floorOf("native");
	const reference = floorOf("reference");
	return (
		<div className="iso not-prose ia-widget">
			<Plate
				fig="Fig 2"
				title="The floor and the step"
				hint="Pick how many agents are open"
				readout={`${mib(rows[0].total)} vs ${mib(rows[1].total)}`}
				keys={[
					{ mark: "raised", label: "The floor: one open agent" },
					{ mark: "flat", label: "Every agent after the first" },
				]}
				caption={`Settled memory of one shared host with idle agents open, measured three times at each size; the second agent's reference sample is within noise of the first. After all twenty were closed, our host settled at ${mib(NATIVE_AFTER_CLOSE)}.`}
			>
				<div className="ia-panel">
					<Choice label="Open agents" value={agents} options={SAMPLES.map((entry) => entry.agents)} onChange={setAgents} format={(value) => `${value}`} />
					<ul className="ia-bars">
						{rows.map((row) => (
							<li key={row.arm}>
								<span className="ia-bar-name">{row.name}</span>
								<span className="ia-bar" aria-hidden="true">
									<span data-part="floor" style={{ width: `${(row.floor / SCALE) * 100}%` }} />
									<span data-part="rest" style={{ width: `${(row.rest / SCALE) * 100}%` }} />
								</span>
								<span className="ia-bar-value">{mib(row.total)}</span>
							</li>
						))}
					</ul>
					<dl className="ia-facts">
						<div>
							<dt>Our floor</dt>
							<dd>{mib(native.first)}</dd>
						</div>
						<div>
							<dt>Each agent after</dt>
							<dd>{Math.round(native.perAgent * 1024)} KiB</dd>
						</div>
						<div>
							<dt>Reference floor</dt>
							<dd>{mib(reference.first)}</dd>
						</div>
						<div>
							<dt>Each agent after</dt>
							<dd>{Math.round(reference.perAgent * 1024)} KiB</dd>
						</div>
					</dl>
				</div>
			</Plate>
		</div>
	);
}

const SERVERS = ["Docs search", "Issue tracker", "Browser", "Database", "Design files"];
const RETIRE_SECONDS = 6;

export function LazyTools() {
	const still = useStillness();
	const [used, setUsed] = React.useState<Record<string, number>>({});
	const [now, setNow] = React.useState(0);
	const running = SERVERS.filter((name) => used[name] !== undefined && (still || now - used[name] < RETIRE_SECONDS));

	React.useEffect(() => {
		if (still || !Object.keys(used).length) return;
		const timer = window.setInterval(() => setNow(performance.now() / 1000), 250);
		return () => window.clearInterval(timer);
	}, [still, used]);

	const call = (name: string) => {
		const at = performance.now() / 1000;
		setNow(at);
		setUsed((current) => ({ ...current, [name]: at }));
	};

	return (
		<div className="iso not-prose ia-widget">
			<Plate
				fig="Fig 3"
				title="Tools start when they are called"
				hint="Call a tool, then wait"
				readout={`${running.length} of ${SERVERS.length} running`}
				keys={[{ mark: "lit", label: "A tool server process" }]}
				caption="Five configured tool servers. Nothing starts until an agent calls one; a connection that goes quiet is retired, and closing the agent ends every process it started. Time is compressed here."
			>
				<div className="ia-panel">
					<ul className="ia-tools">
						{SERVERS.map((name) => {
							const on = running.includes(name);
							return (
								<li key={name} data-on={on ? "" : undefined}>
									<span className="ia-led" aria-hidden="true" />
									<span className="ia-tool-name">{name}</span>
									<span className="ia-tool-state">{on ? "running" : used[name] !== undefined ? "retired" : "configured"}</span>
									<button type="button" onClick={() => call(name)}>
										Call
									</button>
								</li>
							);
						})}
					</ul>
					<button type="button" className="ia-close" onClick={() => setUsed({})} disabled={!running.length}>
						Close the agent
					</button>
				</div>
			</Plate>
		</div>
	);
}

export function SleepMonth() {
	const [workHours, setWorkHours] = React.useState(4);
	const [sessions, setSessions] = React.useState(3);
	const [days, setDays] = React.useState(22);
	const month = monthOf({ workHours, sessions, days });
	const included = month.machineMinutes <= CLOUD_MINUTES;
	return (
		<div className="iso not-prose ia-widget">
			<Plate
				fig="Fig 4"
				title="A month of one cloud computer"
				hint="Describe your month"
				readout={`${count(month.machineMinutes)} vs ${count(month.alwaysOn)} min`}
				keys={[
					{ mark: "raised", label: "Machine minutes: working, then the quiet minutes before sleep" },
					{ mark: "flat", label: "Machine minutes left always on" },
				]}
				caption={`One standard computer at ${STANDARD_RATE} machine minutes per awake minute. Each time you step away it stays up about ${QUIET_MINUTES} quiet minutes, then sleeps. Cloud is ${dollars(CLOUD_PRICE)} a month with ${count(CLOUD_MINUTES)} machine minutes, and extra minutes cost the same.`}
			>
				<div className="ia-panel">
					<div className="ia-sliders">
						<Slider label="Agent work a day" value={workHours} min={0} max={16} step={0.5} format={(value) => `${value} h`} onChange={setWorkHours} />
						<Slider label="Times you step away" value={sessions} min={0} max={12} format={(value) => `${value}`} onChange={setSessions} />
						<Slider label="Days in use" value={days} min={1} max={30} format={(value) => `${value}`} onChange={setDays} />
					</div>
					<ul className="ia-bars">
						<li>
							<span className="ia-bar-name">Sleeps when quiet</span>
							<span className="ia-bar" aria-hidden="true">
								<span data-part="floor" style={{ width: `${month.share * 100}%` }} />
							</span>
							<span className="ia-bar-value">{count(month.machineMinutes)}</span>
						</li>
						<li>
							<span className="ia-bar-name">Always on</span>
							<span className="ia-bar" aria-hidden="true">
								<span data-part="rest" style={{ width: "100%" }} />
							</span>
							<span className="ia-bar-value">{count(month.alwaysOn)}</span>
						</li>
					</ul>
					<p className="ia-verdict" aria-live="polite">
						{Math.round(month.awakeHours)} awake hours, {count(month.machineMinutes)} machine minutes. {included ? `That fits inside Cloud's ${count(CLOUD_MINUTES)}, so the month costs the $${CLOUD_PRICE} plan.` : `That is ${count(month.machineMinutes - CLOUD_MINUTES)} past Cloud's allowance, ${dollars(month.cost - CLOUD_PRICE)} on top of the plan.`} Left on around the clock for {days} {days === 1 ? "day" : "days"}, the same computer would use {count(month.alwaysOn)}, worth {dollars(month.alwaysOnCost)} at the same rate.
					</p>
				</div>
			</Plate>
		</div>
	);
}
