"use client";

import * as React from "react";
import { FINAL_REPORT, MEAN_REQUEST, PRICED_AS, RATE, REQUESTS, SESSION_MODEL, cents, perThousand, tokens, usageCost } from "./session";

function Panel({ fig, label, children }: { fig: string; label: string; children: React.ReactNode }) {
	return (
		<div className="not-prose my-8 border border-dotted border-border bg-card/60 p-4 font-mono text-xs leading-5 text-muted-foreground sm:p-5" role="group" aria-label={label}>
			<p className="editorial-label mb-4">
				{fig} · {label}
			</p>
			{children}
		</div>
	);
}

function Segmented<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: { id: T; label: string }[]; onChange: (value: T) => void }) {
	return (
		<div role="radiogroup" aria-label={label} className="flex flex-wrap gap-1.5">
			{options.map((option) => (
				<button
					key={option.id}
					type="button"
					role="radio"
					aria-checked={value === option.id}
					onClick={() => onChange(option.id)}
					className={`border px-2.5 py-1 transition-colors motion-reduce:transition-none ${value === option.id ? "border-foreground bg-foreground text-background" : "border-border hover:border-foreground/50 hover:text-foreground"}`}
				>
					{option.label}
				</button>
			))}
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

const KINDS = [
	{ id: "cacheRead", label: "cache reads", rate: RATE.cacheRead, tone: "bg-foreground/25" },
	{ id: "cacheWrite", label: "cache writes", rate: RATE.cacheWrite, tone: "bg-foreground/55" },
	{ id: "uncached", label: "fresh input", rate: RATE.input, tone: "bg-foreground/75" },
	{ id: "output", label: "output", rate: RATE.output, tone: "bg-foreground" },
] as const;

const sum = (values: number[]) => values.reduce((total, value) => total + value, 0);

export function RequestStrip() {
	const [unit, setUnit] = React.useState<"tokens" | "dollars">("tokens");
	const weigh = (request: (typeof REQUESTS)[number], kind: (typeof KINDS)[number]) => request[kind.id] * (unit === "dollars" ? kind.rate : 1);
	const most = Math.max(...REQUESTS.map((request) => sum(KINDS.map((kind) => weigh(request, kind)))));
	const totals = KINDS.map((kind) => sum(REQUESTS.map((request) => weigh(request, kind))));
	const share = totals[0] / sum(totals);
	return (
		<Panel fig="Fig 2" label="The same five requests, weighed two ways">
			<Segmented label="Weigh by" value={unit} options={[{ id: "tokens", label: "Tokens" }, { id: "dollars", label: "Dollars" }]} onChange={setUnit} />
			<div className="mt-5 grid h-40 grid-cols-5 items-end gap-3 sm:gap-5" aria-hidden="true">
				{REQUESTS.map((request, index) => {
					const height = sum(KINDS.map((kind) => weigh(request, kind)));
					return (
						<div key={index} className="flex h-full flex-col justify-end">
							<div className="flex flex-col-reverse transition-[height] duration-500 motion-reduce:transition-none" style={{ height: `${(height / most) * 100}%` }}>
								{KINDS.map((kind) => (
									<div key={kind.id} className={`${kind.tone} transition-[flex-grow] duration-500 motion-reduce:transition-none`} style={{ flexGrow: weigh(request, kind), minHeight: weigh(request, kind) ? 1 : 0 }} />
								))}
							</div>
							<p className="mt-2 text-center tabular-nums">{unit === "tokens" ? tokens(height) : cents(height)}</p>
						</div>
					);
				})}
			</div>
			<ul className="mt-4 grid grid-cols-2 gap-x-4 gap-y-1 sm:grid-cols-4">
				{KINDS.map((kind, index) => (
					<li key={kind.id} className="flex items-center gap-2">
						<span className={`size-2 shrink-0 ${kind.tone}`} aria-hidden="true" />
						<span>
							{kind.label} <span className="text-foreground">{unit === "tokens" ? tokens(totals[index]) : cents(totals[index])}</span>
						</span>
					</li>
				))}
			</ul>
			<p className="mt-4" aria-live="polite">
				Cache reads are <span className="text-foreground">{Math.round(share * 100)}%</span> of the {unit === "tokens" ? "tokens" : "bill"}.
			</p>
		</Panel>
	);
}

const TOOLS = ["edit_file", "exec_command", "glob", "grep", "list_directory", "read_file", "update_plan", "write_file"];

function shuffled(list: string[], seed: number) {
	const next = [...list];
	let state = seed;
	for (let index = next.length - 1; index > 0; index--) {
		state = (state * 9301 + 49297) % 233280;
		const pick = Math.floor((state / 233280) * (index + 1));
		[next[index], next[pick]] = [next[pick], next[index]];
	}
	return next;
}

export function PrefixShuffle() {
	const [sorted, setSorted] = React.useState(true);
	const [turn, setTurn] = React.useState(1);
	const previous = sorted ? TOOLS : shuffled(TOOLS, turn);
	const current = sorted ? TOOLS : shuffled(TOOLS, turn + 1);
	const firstChange = current.findIndex((tool, index) => tool !== previous[index]);
	const missFrom = firstChange < 0 ? Infinity : firstChange + 1;
	const request = REQUESTS[1];
	const hit = usageCost(request);
	const miss = (request.cacheRead + request.cacheWrite + request.uncached) * RATE.cacheWrite + request.output * RATE.output;
	const cost = missFrom === Infinity ? hit : miss;
	const blocks = ["instructions", ...current, "conversation"];
	return (
		<Panel fig="Fig 3" label="The front of the prompt">
			<div className="flex flex-wrap items-center gap-3">
				<Segmented label="Tool order" value={sorted ? "sorted" : "shuffled"} options={[{ id: "sorted", label: "Sorted every time" }, { id: "shuffled", label: "Whatever order they load in" }]} onChange={(value) => setSorted(value === "sorted")} />
				<button type="button" onClick={() => setTurn((value) => value + 1)} className="border border-border px-2.5 py-1 hover:border-foreground/50 hover:text-foreground">
					Send the next request →
				</button>
			</div>
			<div className="mt-5 flex flex-wrap gap-1" aria-hidden="true">
				{blocks.map((block, index) => {
					const missed = index >= missFrom && index < blocks.length - 1;
					const last = index === blocks.length - 1;
					return (
						<span
							key={`${block}-${index}`}
							className={`border px-1.5 py-0.5 transition-colors duration-300 motion-reduce:transition-none ${last ? "border-dashed border-border" : missed || (last && missFrom !== Infinity) ? "border-foreground bg-foreground text-background" : "border-border bg-foreground/10 text-foreground"}`}
						>
							{block}
						</span>
					);
				})}
			</div>
			<p className="mt-3">{missFrom === Infinity ? "Every byte up to the conversation matches the last request, so the provider reads it back from its cache." : `The prefix stops matching at block ${missFrom + 1}. Everything from there on is written to the cache again.`}</p>
			<div className="mt-5 grid grid-cols-2 gap-4">
				<Big value={cents(cost)} caption="this request" strong />
				<Big value={`${(cost / hit).toFixed(1)}×`} caption="the cached price" />
			</div>
		</Panel>
	);
}

const BEFORE = [
	{ call: "exec_command  cat the file", result: "contents, but no edit hash" },
	{ call: "write_file  expected_hash: null", result: "rejected: the file already exists" },
	{ call: "read_file  the same file again", result: "contents and a fresh hash" },
	{ call: "edit_file → exec_command  tests", result: "edit lands, tests pass" },
	{ call: "exec_command  tidy up", result: "nothing the task asked for" },
	{ call: "final report", result: "" },
];

const AFTER = [
	{ call: "read_file  every file the task names", result: "contents and edit hashes" },
	{ call: "edit_file → exec_command  tests", result: "edit lands, tests pass" },
	{ call: "final report", result: "" },
];

export function RoundTrips() {
	const [fixed, setFixed] = React.useState(true);
	const steps = fixed ? AFTER : BEFORE;
	const count = steps.length;
	const each = usageCost(MEAN_REQUEST);
	return (
		<Panel fig="Fig 4" label="The pattern in our captured runs">
			<Segmented label="Harness" value={fixed ? "after" : "before"} options={[{ id: "before", label: "Before" }, { id: "after", label: "After" }]} onChange={(value) => setFixed(value === "after")} />
			<ol className="mt-5 grid gap-2">
				{steps.map((step, index) => (
					<li key={`${fixed}-${index}`} className="grid grid-cols-[1.5rem_1fr] gap-2">
						<span className="tabular-nums text-foreground">{index + 1}</span>
						<span className="min-w-0">
							<span className="block break-words text-foreground">{step.call}</span>
							{step.result ? <span className="block">{step.result}</span> : null}
						</span>
					</li>
				))}
			</ol>
			<div className="mt-5 grid grid-cols-2 gap-4">
				<Big value={String(count)} caption="requests" strong />
				<Big value={cents(count * each)} caption="at this session's average request" />
			</div>
		</Panel>
	);
}

export function FinalReport() {
	const [short, setShort] = React.useState(true);
	const count = short ? FINAL_REPORT.after : FINAL_REPORT.before;
	return (
		<Panel fig="Fig 5" label="The last message">
			<Segmented label="Final report" value={short ? "five" : "free"} options={[{ id: "free", label: "Say what you like" }, { id: "five", label: "Five lines" }]} onChange={(value) => setShort(value === "five")} />
			<div className="mt-5 h-2.5 w-full bg-foreground/5" aria-hidden="true">
				<div className="h-full bg-foreground/70 transition-[width] duration-500 motion-reduce:transition-none" style={{ width: `${(count / FINAL_REPORT.before) * 100}%` }} />
			</div>
			<div className="mt-5 grid grid-cols-2 gap-4">
				<Big value={String(count)} caption="output tokens" strong />
				<Big value={perThousand(count * RATE.output)} caption="for the report, per 1,000 tasks" />
			</div>
		</Panel>
	);
}

export function PriceTag() {
	const [as, setAs] = React.useState<"listed" | "actual">("listed");
	const counts = KINDS.map((kind) => sum(REQUESTS.map((request) => request[kind.id])));
	const priced = as === "listed";
	return (
		<Panel fig="Fig 6" label="Putting a price on it">
			<Segmented label="Price as" value={as} options={[{ id: "listed", label: `${PRICED_AS.label}, the closest listed rate` }, { id: "actual", label: `${SESSION_MODEL}, the model that ran it` }]} onChange={setAs} />
			<ul className="mt-5 grid gap-1.5">
				{KINDS.map((kind, index) => (
					<li key={kind.id} className="grid grid-cols-[1fr_auto_4.5rem] gap-3">
						<span>{kind.label}</span>
						<span className="tabular-nums">{counts[index].toLocaleString("en-US")}</span>
						<span className="text-right tabular-nums text-foreground">{priced ? cents(counts[index] * kind.rate) : "—"}</span>
					</li>
				))}
			</ul>
			<div className="mt-5 grid grid-cols-2 gap-4">
				<Big value={priced ? cents(sum(counts.map((count, index) => count * KINDS[index].rate))) : "unknown"} caption="estimated cost" strong />
				<Big value={tokens(sum(counts))} caption="tokens, counted either way" />
			</div>
		</Panel>
	);
}
