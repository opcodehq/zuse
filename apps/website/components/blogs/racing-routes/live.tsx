"use client";

import * as React from "react";
import { IsoFigure } from "@/components/iso/react/draw";
import { asksForStillness, follow, useInView, useLoop, useReadout } from "@/components/iso/react/live";
import { translateAlong, type SolidPaths } from "@/components/iso/iso-kit";
import { DECK_LIFT, depthKey, needleOf, trainOf } from "./geometry";
import { LABEL, ROUTES, createSim, readoutOf, setAwake, setRoute, step, tripOf, trainTripOf, type RouteName, type Sim } from "./model";
import { HEIGHT, LANES, P, RAIL_Z, WIDTH, at, sampleAt } from "./view";

export interface Structure {
	id: string;
	key: number;
	node: React.ReactNode;
}

interface Props {
	label: string;
	base: React.ReactNode;
	board: React.ReactNode;
	pits: React.ReactNode;
	sleepers: React.ReactNode;
	rails: Record<RouteName, React.ReactNode>;
	decks: Record<RouteName, React.ReactNode>;
	structures: Structure[];
}

const TOUR_FIRST = 1.4;
const TOUR_BACK = 3.6;
const LOOP = 80;
const PELLETS = 8;
const SOLID_PARTS = ["fill", "s0", "s1", "s2", "s3", "top", "crease", "edge"] as const;

type Control = RouteName | "awake";
const TOUR: [number, Control, boolean][] = [
	[16, "lan", false],
	[27, "lan", true],
	[37, "awake", false],
	[45, "awake", true],
	[54, "awake", false],
	[71, "awake", true],
];

interface Train {
	route: RouteName | null;
	s: number;
	direction: 1 | -1;
	opacity: number;
	pending: RouteName | null;
}

function writeSolid(nodes: (SVGPathElement | null)[], paths: SolidPaths) {
	const values = [paths.fill, ...paths.shades, paths.top, paths.crease, paths.outline];
	values.forEach((d, index) => nodes[index]?.setAttribute("d", d ?? ""));
}

const describe = (sim: Sim) => {
	const down = ROUTES.filter((route) => !sim.up[route]).map((route) => LABEL[route]);
	return readoutOf(sim) + (down.length ? ". Cut: " + down.join(", ") : "") + (sim.awake ? "" : ". Laptop asleep");
};

export function RoutesLive(props: Props) {
	const { anchor, say } = useReadout<HTMLDivElement>();
	const visible = useInView(anchor);
	const sim = React.useRef<Sim>(createSim(false));
	const touring = React.useRef(true);
	const clock = React.useRef(0);
	const idle = React.useRef(0);
	const lifts = React.useRef<Record<RouteName, number>>({ lan: 0, tail: 0, relay: 0 });
	const needleAt = React.useRef<Record<RouteName, number>>({ lan: 0, tail: 0, relay: 0 });
	const train = React.useRef<Train>({ route: null, s: 0, direction: 1, opacity: 0, pending: null });
	const decks = React.useRef<Record<RouteName, SVGGElement | null>>({ lan: null, tail: null, relay: null });
	const needles = React.useRef<Record<RouteName, SVGPathElement | null>>({ lan: null, tail: null, relay: null });
	const pellets = React.useRef<(SVGCircleElement | null)[]>([]);
	const slots = React.useRef<Record<string, SVGGElement | null>>({});
	const trainNode = React.useRef<SVGGElement | null>(null);
	const body = React.useRef<(SVGPathElement | null)[]>([]);
	const roof = React.useRef<(SVGPathElement | null)[]>([]);
	const lamp = React.useRef<SVGCircleElement | null>(null);
	const slotOf = React.useRef("");
	const stateRoot = React.useRef<HTMLDivElement | null>(null);
	const spokenPhase = React.useRef("");
	const [controls, setControls] = React.useState({ lan: true, tail: true, relay: true, awake: true });
	const [spoken, setSpoken] = React.useState("Racing the routes");

	const sync = React.useCallback(() => {
		const s = sim.current;
		setControls({ lan: s.up.lan, tail: s.up.tail, relay: s.up.relay, awake: s.awake });
		setSpoken(describe(s));
	}, []);

	const apply = React.useCallback(
		(control: Control, value: boolean) => {
			if (control === "awake") setAwake(sim.current, value);
			else setRoute(sim.current, control, value);
			sync();
		},
		[sync],
	);

	const draw = React.useCallback(() => {
		const s = sim.current;
		const root = stateRoot.current;
		if (root) {
			root.dataset.mode = s.mode;
			root.dataset.current = s.current ?? "";
			root.dataset.awake = s.awake ? "yes" : "no";
			for (const route of ROUTES) {
				root.dataset[route] = !s.up[route] ? "cut" : s.current === route && s.mode === "connected" ? "live" : s.pellets.some((pellet) => pellet.route === route) ? "probe" : s.known[route] ? "idle" : "unknown";
			}
		}
		for (const route of ROUTES) {
			decks.current[route]?.setAttribute("transform", translateAlong(P, [0, 0, lifts.current[route]]));
			needles.current[route]?.setAttribute("d", needleOf(route, needleAt.current[route]));
		}
		s.pellets.slice(0, PELLETS).forEach((pellet, index) => {
			const node = pellets.current[index];
			if (!node) return;
			const lane = LANES[pellet.route];
			const trip = tripOf(pellet.route);
			const elapsed = s.now - pellet.start;
			let distance: number;
			let opacity = 1;
			if (pellet.fails) {
				const reach = Math.min(1, elapsed / (trip * 0.8));
				distance = lane.total * (s.up[pellet.route] ? Math.min(1, elapsed / trip) : 0.3 * reach);
				opacity = Math.max(0, 1 - Math.max(0, elapsed - trip * (s.up[pellet.route] ? 1 : 0.8)) / 0.6);
			} else {
				const half = elapsed / trip;
				distance = lane.total * (half <= 1 ? half : Math.max(0, 2 - half));
			}
			const { point } = sampleAt(lane, distance);
			const [x, y] = at([point[0], point[1], RAIL_Z + 1.6]);
			node.setAttribute("cx", x.toFixed(1));
			node.setAttribute("cy", y.toFixed(1));
			node.style.opacity = opacity.toFixed(2);
			node.dataset.role = pellet.role;
		});
		for (let index = s.pellets.length; index < PELLETS; index++) {
			const node = pellets.current[index];
			if (node) node.style.opacity = "0";
		}
		const t = train.current;
		const node = trainNode.current;
		if (!node) return;
		if (!t.route) {
			node.style.opacity = "0";
			return;
		}
		const { point, angle } = sampleAt(LANES[t.route], t.s);
		const solids = trainOf(point[0], point[1], angle);
		writeSolid(body.current, solids.body);
		writeSolid(roof.current, solids.roof);
		lamp.current?.setAttribute("cx", solids.lamp[0].toFixed(1));
		lamp.current?.setAttribute("cy", solids.lamp[1].toFixed(1));
		node.style.opacity = t.opacity.toFixed(2);
		if (s.mode === "connected") delete node.dataset.held;
		else node.dataset.held = "";
		const key = depthKey(point);
		const slot = props.structures.find((structure) => structure.key > key)?.id ?? "end";
		if (slot !== slotOf.current) {
			slots.current[slot]?.appendChild(node);
			slotOf.current = slot;
		}
	}, [props.structures]);

	const advance = React.useCallback(
		(dt: number, calm: boolean) => {
			const s = sim.current;
			step(s, dt);
			for (const route of ROUTES) {
				lifts.current[route] = follow(lifts.current[route], s.up[route] ? 0 : DECK_LIFT, dt, 0.25, calm);
				const measured = s.measured[route];
				const target = measured === null ? 0 : Math.log10(Math.max(1, measured)) / 2;
				needleAt.current[route] = follow(needleAt.current[route], target, dt, 0.35, calm);
			}
			const t = train.current;
			const live = s.mode === "connected" ? s.current : null;
			if (live && t.route !== live && t.pending !== live) t.pending = live;
			if (t.pending) {
				t.opacity = t.route ? follow(t.opacity, 0, dt, 0.08, calm) : 0;
				if (t.opacity < 0.05) {
					const changed = t.route !== null;
					t.route = t.pending;
					t.pending = null;
					if (changed || t.s === 0) {
						t.s = 0;
						t.direction = 1;
					}
				}
			} else if (t.route) {
				t.opacity = follow(t.opacity, live ? 1 : 0.55, dt, 0.15, calm);
				if (live === t.route) {
					const lane = LANES[t.route];
					t.s += (t.direction * lane.total * dt) / trainTripOf(t.route);
					if (t.s >= lane.total) {
						t.s = lane.total;
						t.direction = -1;
					} else if (t.s <= 0) {
						t.s = 0;
						t.direction = 1;
					}
				}
			}
			if (!t.route && live) t.route = live;
			if (s.mode === "lost" && t.route) t.opacity = follow(t.opacity, 0, dt, 0.2, calm);
		},
		[],
	);

	const tick = React.useCallback(
		(dt: number, calm: boolean) => {
			if (touring.current && !calm) {
				const before = clock.current;
				clock.current += dt;
				if (clock.current >= LOOP) {
					clock.current = 0;
					sim.current = createSim(false);
					train.current = { route: null, s: 0, direction: 1, opacity: 0, pending: null };
					sync();
				}
				for (const [time, control, value] of TOUR) if (before < time && clock.current >= time) apply(control, value);
			} else if (!touring.current) {
				idle.current += dt;
				if (idle.current > TOUR_BACK + 30 && !calm) touring.current = true;
			}
			if (calm) {
				for (let index = 0; index < 400; index++) advance(0.05, true);
				const t = train.current;
				if (t.route) t.s = LANES[t.route].total * 0.62;
			} else advance(dt, false);
			draw();
			say(readoutOf(sim.current));
			const phase = sim.current.mode + (sim.current.current ?? "");
			if (phase !== spokenPhase.current) {
				spokenPhase.current = phase;
				setSpoken(describe(sim.current));
			}
			return !calm;
		},
		[advance, apply, draw, say, sync],
	);

	const kick = useLoop(tick, visible);

	React.useEffect(() => {
		if (asksForStillness()) {
			touring.current = false;
			sim.current = createSim(true);
		}
		const timer = window.setTimeout(() => kick(), TOUR_FIRST * 1000);
		return () => window.clearTimeout(timer);
	}, [kick]);

	const interact = React.useCallback(() => {
		if (touring.current) {
			touring.current = false;
			const s = sim.current;
			for (const route of ROUTES) s.known[route] = true;
		}
		idle.current = 0;
		kick();
	}, [kick]);

	const toggle = (control: Control) => {
		interact();
		const s = sim.current;
		apply(control, control === "awake" ? !s.awake : !s.up[control]);
		kick();
	};

	const onKey = (event: React.KeyboardEvent) => {
		const map: Record<string, Control> = { "1": "lan", "2": "tail", "3": "relay", s: "awake", S: "awake" };
		const control = map[event.key];
		if (!control) return;
		event.preventDefault();
		toggle(control);
	};

	const solidNodes = (store: React.RefObject<(SVGPathElement | null)[]>, tone: string) =>
		SOLID_PARTS.map((part, index) => (
			<path
				key={part}
				ref={(node) => {
					store.current[index] = node;
				}}
				className={part === "fill" ? "iso-fill" : part === "top" ? "iso-top" : part === "crease" ? "iso-line iso-crease" : part === "edge" ? "iso-line iso-edge" : "iso-shade"}
				data-shade={part.startsWith("s") ? part.slice(1) : undefined}
				data-tone={part === "crease" ? "faint" : part === "edge" ? tone : undefined}
			/>
		));

	return (
		<div
			ref={anchor}
			className="rr-focus"
			tabIndex={0}
			role="group"
			aria-label="Railway switchyard. Press 1, 2 or 3 to cut or restore the local network, tailnet or relay track, and S to put the laptop to sleep or wake it."
			onKeyDown={onKey}
			onPointerDown={interact}
		>
			<div ref={stateRoot} className="rr-live">
			<IsoFigure width={WIDTH} height={HEIGHT} label={props.label}>
				{props.base}
				{props.board}
				{ROUTES.map((route) => (
					<path
						key={route}
						ref={(node) => {
							needles.current[route] = node;
						}}
						className="iso-line rr-needle"
						data-route={route}
						data-free="start"
						d={needleOf(route, 0)}
					/>
				))}
				{props.pits}
				{props.sleepers}
				{ROUTES.map((route) => (
					<g key={route} className="rr-lane" data-route={route}>
						{props.rails[route]}
					</g>
				))}
				{ROUTES.map((route) => (
					<g
						key={route}
						className="rr-deck"
						ref={(node) => {
							decks.current[route] = node;
						}}
					>
						{props.decks[route]}
					</g>
				))}
				<g className="rr-pellets">
					{Array.from({ length: PELLETS }, (_, index) => (
						<circle
							key={index}
							ref={(node) => {
								pellets.current[index] = node;
							}}
							r={1.9}
							cx={0}
							cy={0}
							style={{ opacity: 0 }}
						/>
					))}
				</g>
				{props.structures.map((structure) => (
					<React.Fragment key={structure.id}>
						<g
							ref={(node) => {
								slots.current[structure.id] = node;
							}}
						/>
						{structure.node}
					</React.Fragment>
				))}
				<g
					ref={(node) => {
						slots.current.end = node;
					}}
				/>
				<g className="rr-hold">
					<g ref={trainNode} className="rr-train" style={{ opacity: 0 }}>
						<g className="iso-solid">{solidNodes(body, "hi")}</g>
						<g className="iso-solid" data-lit="">
							{solidNodes(roof, "lit")}
						</g>
						<circle ref={lamp} className="rr-lamp" data-kind="train" r={1} />
					</g>
				</g>
				{ROUTES.map((route) => (
					<path key={route} className="rr-hit" d={pathOfLane(route)} onClick={() => toggle(route)} />
				))}
			</IsoFigure>
			</div>
			<div className="rr-controls" role="group" aria-label="Cut or restore a route">
				{(["lan", "tail", "relay"] as const).map((route, index) => (
					<button key={route} type="button" aria-pressed={!controls[route]} onClick={() => toggle(route)}>
						<span aria-hidden="true">{index + 1}</span>
						{route === "lan" ? "Wi-Fi" : LABEL[route]}
						<em>{controls[route] ? "on" : "cut"}</em>
					</button>
				))}
				<button type="button" aria-pressed={!controls.awake} onClick={() => toggle("awake")}>
					<span aria-hidden="true">S</span>
					Laptop
					<em>{controls.awake ? "awake" : "asleep"}</em>
				</button>
			</div>
			<p className="rr-sr" aria-live="polite">
				{spoken}
			</p>
		</div>
	);
}

const lanePaths: Partial<Record<RouteName, string>> = {};
function pathOfLane(route: RouteName) {
	lanePaths[route] ??= LANES[route].points.map(([x, y], index) => (index ? "L" : "M") + at([x, y, RAIL_Z]).map((v) => v.toFixed(1)).join(" ")).join("");
	return lanePaths[route]!;
}
