"use client";

import * as React from "react";
import { Dots, IsoFigure, Line, Plate, Solid, Wire } from "@/components/iso/draw";
import { follow, useInView, useLoop, useReadout, useStillness } from "@/components/iso/live";
import type { SolidPaths } from "@/components/iso/iso-kit";
import { CPU_MAX, MEMORY_MAX, PROCESSES, WORKLOADS, fraction, megabytes, percent, seconds, strokeRate, type EngineId } from "./bench";
import { BASE_ART, ENGINES, HEIGHT, HOUSING_ART, PANELS, PILLOWS, SHAFTS, WIDTH, drumMark, heatAt, needleOf, rodOf, type Cylinder, type Engine, type Panel, type Stack } from "./dyno-geometry";
import "@/components/iso/iso.css";
import "./native-rust.css";

const IDS: EngineId[] = ["native", "electron"];
const NAME: Record<EngineId, string> = { native: "Zuse", electron: "Electron-based app" };
const HEAT_POOL = 10;
const HEAT_LIFE = 2.2;
const TOUR_STEP = 7;
const TOUR_REST = 15;
const DRUM_RATE = 0.5;

function RodNodes({ live, rod }: { live: LiveRefs; rod: number }) {
	const set = (index: number) => (node: SVGPathElement | null) => live.rod(rod, index, node);
	return (
		<g className="iso-solid">
			<path ref={set(0)} className="iso-fill" />
			{[0, 1, 2, 3].map((shade) => (
				<path key={shade} ref={set(1 + shade)} className="iso-shade" data-shade={shade} />
			))}
			<path ref={set(5)} className="iso-top" />
			<path ref={set(6)} className="iso-line iso-edge" data-tone="hi" />
		</g>
	);
}

function writeSolid(nodes: (SVGPathElement | null)[], paths: SolidPaths) {
	[paths.fill, ...paths.shades, paths.top, paths.outline].forEach((d, index) => nodes[index]?.setAttribute("d", d ?? ""));
}

function PanelArt({ panel }: { panel: Panel }) {
	return (
		<g>
			<Solid paths={panel.plinth} tone="lo" />
			<Solid paths={panel.cabinet} tone="mid" inner={<Line d={panel.seam} tone="faint" />} />
			<Dots points={panel.screws} size={0.5} tone="mid" />
			{[panel.memory, panel.cpu].map((dial) => (
				<g key={dial.cx}>
					<path className="nr-dial-face" d={dial.face} />
					<Line d={dial.bezel} tone="mid" />
					<Line d={dial.face} tone="lo" />
					<Line d={dial.minor} tone="lo" />
					<Line d={dial.major} tone="mid" />
				</g>
			))}
			<Line d={panel.lampRing} tone="lo" />
			<Line d={panel.socket} tone="lo" />
		</g>
	);
}

function Mount({ engine }: { engine: Engine }) {
	return (
		<g>
			<Solid paths={engine.mount} tone="lo" />
			<Line d={engine.mountEdge} tone="faint" />
			<Dots points={engine.mountBolts} size={0.5} tone="mid" />
		</g>
	);
}

interface LiveRefs {
	rod: (rod: number, index: number, node: SVGPathElement | null) => void;
	tip: (rod: number, node: SVGCircleElement | null) => void;
}

function CylinderArt({ part, index, live }: { part: Cylinder; index: number; live: LiveRefs }) {
	return (
		<g>
			<Solid paths={part.barrel} tone="mid" inner={<Line d={part.fins} tone="lo" />} />
			<Solid paths={part.head} tone="mid" />
			<Dots points={part.headBolts} size={0.45} tone="mid" />
			<Solid paths={part.guide} tone="mid" crease="none" />
			<RodNodes live={live} rod={index} />
			<circle
				ref={(node) => live.tip(index, node)}
				className="nr-tip"
				r={0.9}
			/>
		</g>
	);
}

function StackArt({ part }: { part: Stack }) {
	return (
		<g>
			<Solid paths={part.pipe} tone="mid" crease="none" />
			<Solid paths={part.lip} tone="mid" crease="none" />
		</g>
	);
}

const CYLINDERS = IDS.flatMap((id) => ENGINES[id].parts.filter((part): part is Cylinder => part.kind === "cylinder").map((part) => ({ id, part })));
const STACKS = IDS.flatMap((id) => ENGINES[id].parts.filter((part): part is Stack => part.kind === "stack").map((part) => ({ id, part })));

function EngineArt({ engine, live }: { engine: Engine; live: LiveRefs }) {
	const facing = engine.flangeBolts.length > 0;
	const flange = (
		<g>
			<Solid paths={engine.flange} tone="mid" crease="none" />
			<Dots points={engine.flangeBolts} size={0.4} tone="mid" />
		</g>
	);
	return (
		<g data-engine={engine.id}>
			{facing ? null : flange}
			<Solid paths={engine.crankcase} tone="mid" inner={<Line d={engine.caseSeam} tone="faint" />} />
			<Solid paths={engine.sump} tone="mid" crease="none" />
			<Dots points={engine.caseBolts} size={0.5} tone="mid" />
			{facing ? flange : null}
			{engine.parts.map((part) =>
				part.kind === "cylinder" ? (
					<CylinderArt key={part.key} part={part} index={CYLINDERS.findIndex((entry) => entry.part === part)} live={live} />
				) : (
					<StackArt key={part.key} part={part} />
				),
			)}
		</g>
	);
}

function Pillow({ index, part }: { index: number; part: "foot" | "block" }) {
	const pillow = PILLOWS[index]!;
	if (part === "foot") {
		return (
			<g>
				<Solid paths={pillow.foot} tone="lo" />
				<Dots points={pillow.footBolts} size={0.45} tone="mid" />
			</g>
		);
	}
	return (
		<g>
			<Solid paths={pillow.block} tone="mid" inner={<Line d={pillow.cap} tone="lo" />} />
			<Dots points={pillow.capBolts} size={0.45} tone="mid" />
		</g>
	);
}

const LABEL =
	"A dynamometer bench with two engines coupled to one load drum in the middle, so both turn under the same workload. The small one-cylinder engine on the left is Zuse; the six-cylinder engine on the right is a typical Electron-based agent app, one cylinder per process. Each engine has a gauge cabinet behind it with a memory dial, a heat dial for idle CPU and a lamp that lights when the app is ready to type. Small heat marks rise from the exhaust stacks in proportion to idle CPU.";

interface Heat {
	age: number;
	alive: boolean;
}

export function DynoBench() {
	const { anchor, say } = useReadout<HTMLDivElement>();
	const visible = useInView(anchor);
	const calmPreference = useStillness();
	const [selected, setSelected] = React.useState(0);
	const selectedRef = React.useRef(0);
	const clock = React.useRef(0);
	const launched = React.useRef(0);
	const touring = React.useRef(true);
	const quiet = React.useRef(0);
	const needles = React.useRef<Record<string, SVGPathElement | null>>({});
	const needleAt = React.useRef<Record<string, number>>({ "native-memory": 0, "native-cpu": 0, "electron-memory": 0, "electron-cpu": 0 });
	const lamps = React.useRef<Record<string, SVGCircleElement | null>>({});
	const stats = React.useRef<HTMLDivElement>(null);
	const rods = React.useRef<(SVGPathElement | null)[][]>(CYLINDERS.map(() => []));
	const tips = React.useRef<(SVGCircleElement | null)[]>([]);
	const heatNodes = React.useRef<(SVGCircleElement | null)[]>([]);
	const heat = React.useRef<Heat[]>(STACKS.flatMap(() => Array.from({ length: HEAT_POOL }, () => ({ age: 0, alive: false }))));
	const emit = React.useRef<number[]>(STACKS.map(() => 0));
	const phases = React.useRef<Record<EngineId, number>>({ native: 0, electron: 0 });
	const drum = React.useRef<SVGCircleElement>(null);
	const drumAngle = React.useRef(0);
	const live = React.useMemo<LiveRefs>(
		() => ({
			rod: (rod, index, node) => {
				rods.current[rod]![index] = node;
			},
			tip: (rod, node) => {
				tips.current[rod] = node;
			},
		}),
		[],
	);

	const choose = React.useCallback((index: number, fromTour = false) => {
		const next = (index + WORKLOADS.length) % WORKLOADS.length;
		selectedRef.current = next;
		setSelected(next);
		if (!fromTour) {
			touring.current = false;
			quiet.current = 0;
		}
	}, []);

	const relaunch = React.useCallback((fromTour = false) => {
		launched.current = clock.current;
		if (!fromTour) {
			touring.current = false;
			quiet.current = 0;
		}
		for (const key of Object.keys(needleAt.current)) needleAt.current[key] = 0;
		phases.current = { native: 0, electron: 0 };
	}, []);

	const tick = (dt: number, calm: boolean) => {
		clock.current += dt;
		const workload = WORKLOADS[selectedRef.current]!;
		const since = calm ? 99 : clock.current - launched.current;
		if (!touring.current && !calm) {
			quiet.current += dt;
			if (quiet.current > TOUR_REST) {
				touring.current = true;
				quiet.current = 0;
			}
		}
		if (touring.current && !calm && since > TOUR_STEP) {
			const next = (selectedRef.current + 1) % WORKLOADS.length;
			choose(next, true);
			if (next === 0) relaunch(true);
			else launched.current = clock.current - 3;
		}
		const ready: Record<EngineId, boolean> = { native: false, electron: false };
		for (const id of IDS) {
			const reading = workload[id];
			ready[id] = since >= reading.ready;
			lamps.current[id]?.setAttribute("data-on", ready[id] ? "true" : "false");
			const targets = { memory: ready[id] ? fraction(reading.memory, MEMORY_MAX) : 0, cpu: ready[id] ? fraction(reading.cpu, CPU_MAX) : 0 };
			for (const dial of ["memory", "cpu"] as const) {
				const key = id + "-" + dial;
				needleAt.current[key] = follow(needleAt.current[key]!, targets[dial], dt, 0.35, calm);
				needles.current[key]?.setAttribute("d", needleOf(PANELS[id][dial].cx, needleAt.current[key]!));
			}
			if (ready[id] && !calm) phases.current[id] += dt * strokeRate(reading.cpu) * Math.PI * 2;
		}
		CYLINDERS.forEach(({ id, part }, index) => {
			const lift = calm || !ready[id] ? 0.5 : 0.5 + 0.5 * Math.sin(phases.current[id] + part.phase);
			const rod = rodOf(part, lift);
			writeSolid(rods.current[index]!, rod.paths);
			const tip = tips.current[index];
			tip?.setAttribute("cx", rod.tip[0].toFixed(1));
			tip?.setAttribute("cy", rod.tip[1].toFixed(1));
		});
		STACKS.forEach(({ id, part }, stack) => {
			if (!calm && ready[id]) {
				emit.current[stack]! += dt * workload[id].cpu * 0.45;
				while (emit.current[stack]! >= 1) {
					emit.current[stack]! -= 1;
					const free = heat.current.slice(stack * HEAT_POOL, stack * HEAT_POOL + HEAT_POOL).findIndex((entry) => !entry.alive);
					if (free >= 0) heat.current[stack * HEAT_POOL + free] = { age: 0, alive: true };
				}
			}
			for (let slot = 0; slot < HEAT_POOL; slot++) {
				const index = stack * HEAT_POOL + slot;
				const entry = heat.current[index]!;
				const node = heatNodes.current[index];
				if (!node) continue;
				if (entry.alive) entry.age += dt;
				if (entry.age > HEAT_LIFE || calm) entry.alive = false;
				if (!entry.alive) {
					node.setAttribute("opacity", "0");
					continue;
				}
				const t = entry.age / HEAT_LIFE;
				const [x, y] = heatAt(part, 2 + t * 20, Math.sin(entry.age * 2.4 + index * 1.7) * (1 + t * 3));
				node.setAttribute("cx", x.toFixed(1));
				node.setAttribute("cy", y.toFixed(1));
				node.setAttribute("opacity", (Math.sin(Math.PI * t) * 0.65).toFixed(2));
			}
		});
		if (!calm) drumAngle.current += dt * DRUM_RATE * Math.PI * 2;
		const mark = drumMark(drumAngle.current);
		drum.current?.setAttribute("cx", mark[0].toFixed(1));
		drum.current?.setAttribute("cy", mark[1].toFixed(1));
		stats.current?.querySelectorAll<HTMLElement>("[data-ready]").forEach((node) => node.setAttribute("data-on", ready[node.dataset.ready as EngineId] ? "true" : "false"));
		const launching = !ready.electron;
		say(launching ? "starting · " + seconds(Math.max(0, since)) : megabytes(workload.native.memory) + " vs " + megabytes(workload.electron.memory));
		return !calm || needleAt.current["native-memory"] !== fraction(workload.native.memory, MEMORY_MAX);
	};

	const kick = useLoop(tick, visible);
	React.useEffect(() => {
		kick();
	}, [selected, calmPreference, kick]);

	const onKey = (event: React.KeyboardEvent) => {
		const digit = Number(event.key);
		if (digit >= 1 && digit <= WORKLOADS.length) choose(digit - 1);
		else if (event.key === "l" || event.key === "L" || event.key === " ") relaunch();
		else if (event.key === "ArrowRight") choose(selectedRef.current + 1);
		else if (event.key === "ArrowLeft") choose(selectedRef.current - 1);
		else return;
		event.preventDefault();
		kick();
	};

	const workload = WORKLOADS[selected]!;
	return (
		<div className="iso nr-figure not-prose">
			<Plate
				fig="Fig 1"
				title="Same load, two engines"
				hint="Tap · 1–5 · L to relaunch"
				readout={megabytes(workload.native.memory) + " vs " + megabytes(workload.electron.memory)}
				keys={[
					{ mark: "lit", label: "Needles: memory at rest (left dial, 0–1,200 MB) and idle CPU (right dial, 0–10%)" },
					{ mark: "raised", label: "One cylinder per app process" },
				]}
				caption="Both apps ran on the same Mac with the same seeded chat history. Each value is the median of five launches. The lamp lights when a typed character first appears in the composer. Stroke speed and heat follow idle CPU, and the load drum turns at one speed for both."
			>
				<div
					ref={anchor}
					className="nr-dyno"
					tabIndex={0}
					role="group"
					aria-label="Dynamometer bench. Press 1 to 5 to pick a chat history, L to relaunch both apps."
					onKeyDown={onKey}
					onClick={() => {
						choose(selectedRef.current + 1);
						kick();
					}}
				>
					<IsoFigure width={WIDTH} height={HEIGHT} label={LABEL}>
						<path className="iso-halo" d={BASE_ART.halo} />
						{BASE_ART.feet.map((parts, index) => (
							<g key={index}>
								<Solid paths={parts[0]!} tone="lo" />
								<Solid paths={parts[1]!} tone="mid" />
							</g>
						))}
						<Solid paths={BASE_ART.slab} tone="mid" />
						<Line d={BASE_ART.groove} tone="lo" />
						<Line d={BASE_ART.screwRings} tone="lo" />
						<Dots points={BASE_ART.screws} size={0.5} tone="mid" />
						<Solid paths={BASE_ART.label} tone="lo" crease="none" />
						<Line d={BASE_ART.labelGroove} tone="faint" />
						<Line d={BASE_ART.labelRules} tone="lo" free />
						<Dots points={BASE_ART.labelScrews} size={0.45} tone="mid" />
						<Solid paths={BASE_ART.strip} tone="lo" crease="none" />
						<Line d={BASE_ART.stripMinor} tone="lo" />
						<Line d={BASE_ART.stripMajor} tone="mid" />

						{IDS.map((id) => (
							<g key={id}>
								<PanelArt panel={PANELS[id]} />
								{(["memory", "cpu"] as const).map((dial) => (
									<path
										key={dial}
										ref={(node) => {
											needles.current[id + "-" + dial] = node;
										}}
										className="iso-line nr-needle"
										data-tone="lit"
										data-free=""
										d={needleOf(PANELS[id][dial].cx, 0)}
									/>
								))}
								<Dots points={[PANELS[id].memory.pivot, PANELS[id].cpu.pivot]} size={0.9} tone="hi" />
								<circle
									ref={(node) => {
										lamps.current[id] = node;
									}}
									className="nr-lamp"
									data-on="false"
									cx={Math.round(PANELS[id].lamp[0] * 10) / 10}
									cy={Math.round(PANELS[id].lamp[1] * 10) / 10}
									r={1.5}
								/>
							</g>
						))}

						<Mount engine={ENGINES.native} />
						<Mount engine={ENGINES.electron} />
						<Pillow index={0} part="foot" />
						<Pillow index={1} part="foot" />
						<Solid paths={HOUSING_ART.plinth} tone="lo" />
						<Dots points={HOUSING_ART.plinthBolts} size={0.45} tone="mid" />
						{IDS.map((id) => (
							<Wire key={id} from={PANELS[id].lead.from} via={PANELS[id].lead.via} to={PANELS[id].lead.to} tone="mid" size={1} />
						))}

						<EngineArt engine={ENGINES.native} live={live} />
						<Solid paths={SHAFTS.leanOut} tone="hi" crease="none" />
						<Pillow index={0} part="block" />
						<Solid paths={SHAFTS.leanIn} tone="hi" crease="none" />
						<Solid paths={HOUSING_ART.flanges[0]!} tone="mid" crease="none" />
						<Solid paths={HOUSING_ART.body} tone="mid" inner={<Line d={HOUSING_ART.seam} tone="faint" />} />
						<Line d={HOUSING_ART.vents} tone="lo" />
						<Line d={HOUSING_ART.sideVents} tone="lo" />
						<Solid paths={HOUSING_ART.collar} tone="mid" />
						<Solid paths={HOUSING_ART.drum} tone="hi" inner={<Line d={HOUSING_ART.knurl} tone="faint" />} />
						<Line d={HOUSING_ART.track} tone="lo" />
						<Solid paths={HOUSING_ART.crown} tone="mid" />
						<circle ref={drum} className="nr-tip" data-lit="" r={1.1} />
						<Solid paths={HOUSING_ART.flanges[1]!} tone="mid" crease="none" />
						<Dots points={HOUSING_ART.flangeBolts} size={0.4} tone="mid" />
						<Solid paths={SHAFTS.heavyIn} tone="hi" crease="none" />
						<Pillow index={1} part="block" />
						<Solid paths={SHAFTS.heavyOut} tone="hi" crease="none" />
						<EngineArt engine={ENGINES.electron} live={live} />

						<g className="nr-heat">
							{STACKS.flatMap((_, stack) =>
								Array.from({ length: HEAT_POOL }, (_, slot) => (
									<circle
										key={stack * HEAT_POOL + slot}
										ref={(node) => {
											heatNodes.current[stack * HEAT_POOL + slot] = node;
										}}
										r={0.8}
										opacity={0}
									/>
								)),
							)}
						</g>
					</IsoFigure>
				</div>
				<div className="nr-controls" role="radiogroup" aria-label="Chat history">
					{WORKLOADS.map((entry, index) => (
						<button
							key={entry.id}
							type="button"
							role="radio"
							aria-checked={selected === index}
							onClick={() => {
								choose(index);
								kick();
							}}
						>
							{entry.label}
						</button>
					))}
					<button
						type="button"
						className="nr-relaunch"
						onClick={() => {
							relaunch();
							kick();
						}}
					>
						Relaunch
					</button>
				</div>
				<div ref={stats} className="nr-stats" aria-live="polite">
					<span />
					{IDS.map((id) => (
						<span key={id} className="nr-stats-head" data-engine={id}>
							{NAME[id]}
						</span>
					))}
					<span>Ready to type</span>
					{IDS.map((id) => (
						<span key={id} data-ready={id} data-on="true">
							{seconds(workload[id].ready)}
						</span>
					))}
					<span>Memory at rest</span>
					{IDS.map((id) => (
						<span key={id}>{megabytes(workload[id].memory)}</span>
					))}
					<span>Idle CPU</span>
					{IDS.map((id) => (
						<span key={id}>{percent(workload[id].cpu)}</span>
					))}
					<span>Processes</span>
					{IDS.map((id) => (
						<span key={id}>{PROCESSES[id]}</span>
					))}
				</div>
				<p className="nr-note">
					{workload.chats === 1 ? "One chat" : workload.chats + " chats"} of {workload.messages.toLocaleString("en-US")} messages each.
				</p>
			</Plate>
		</div>
	);
}
