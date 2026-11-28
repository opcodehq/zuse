import {
	corners,
	cylinder,
	fitProjection,
	insetPlan,
	iso,
	knurl,
	onTop,
	pathOf,
	planOutline,
	ring,
	sideArc,
	sideRing,
	sideSeam,
	slabOf,
	topTicks,
	type Flat,
	type Plan,
	type Plane,
	type Point3,
	type SolidPaths,
} from "@/components/iso/iso-kit";
import type { EngineId } from "./bench";

export const WIDTH = 640;
export const HEIGHT = 360;
export const AZIMUTH = 58;

const BASE: Plan = { x: -200, y: -82, w: 400, d: 148, r: 8 };
const BASE_H = 10;
const FOOT_H = 5;
const PANEL_Y = -74;
const PANEL_D = 10;
const PANEL_Z = 2;
const PANEL_H = 50;
const PANEL_FACE = PANEL_Y + PANEL_D;
const SHAFT_Z = 10;
const SHAFT_H = 4;

export const P = fitProjection(
	[
		[BASE.x, BASE.y, -BASE_H - FOOT_H],
		[BASE.x + BASE.w, BASE.y, -BASE_H - FOOT_H],
		[BASE.x, BASE.y + BASE.d, -BASE_H - FOOT_H],
		[BASE.x + BASE.w, BASE.y + BASE.d, -BASE_H - FOOT_H],
		[-168, PANEL_Y, PANEL_Z + PANEL_H + 2],
		[190, PANEL_Y, PANEL_Z + PANEL_H + 2],
	],
	WIDTH,
	HEIGHT,
	{ pad: 14, azimuth: AZIMUTH },
);

export const at = (point: Point3): Flat => iso(point, P);
const solid = (plan: Plan, z: number, h: number, steps = 4, bevel = 0) => slabOf(plan, z, h, P, steps, bevel);
const post = (x: number, y: number, r: number, z: number, h: number, steps = 28) => cylinder(x, y, r, z, h, P, steps);
const join = (paths: string[]) => paths.join("");
const segment = (a: Point3, b: Point3) => pathOf([at(a), at(b)]);
const depthKey = ([x, y]: Plane) => x * Math.cos((AZIMUTH * Math.PI) / 180) + y * Math.sin((AZIMUTH * Math.PI) / 180);

const FEET = corners(BASE, 24);
const SCREWS = corners(BASE, 8);
const LABEL: Plan = { x: -186, y: 44, w: 54, d: 14, r: 2 };
const STRIP: Plan = { x: -116, y: 52, w: 296, d: 7, r: 1 };
const STRIP_TICKS = topTicks(STRIP.x + 4, STRIP.x + STRIP.w - 4, 4, 5, STRIP.y + 0.4, 0.6, [2, 3.8], P);

export const BASE_ART = {
	halo: solid(BASE, -BASE_H - FOOT_H, BASE_H).fill,
	feet: FEET.map(([x, y]) => [post(x, y, 8, -BASE_H - FOOT_H, 1.6), post(x, y, 6, -BASE_H - FOOT_H + 1.6, FOOT_H - 1.6)]),
	slab: solid(BASE, -BASE_H, BASE_H, 8, 1.6),
	groove: planOutline(insetPlan(BASE, 4.5), 0, P),
	screws: SCREWS.map(([x, y]) => at([x, y, 0])),
	screwRings: join(SCREWS.map(([x, y]) => ring(x, y, 2.2, 0, P, 16))),
	label: solid(LABEL, 0, 0.8, 3),
	labelGroove: planOutline(insetPlan(LABEL, 1.6), 0.8, P, 3),
	labelRules: join([0, 1, 2].map((row) => onTop([[LABEL.x + 7, LABEL.y + 3.8 + row * 2.6], [LABEL.x + (row === 1 ? 30 : 42), LABEL.y + 3.8 + row * 2.6]], 0.8, P))),
	labelScrews: [at([LABEL.x + 3.4, LABEL.y + LABEL.d / 2, 0.8]), at([LABEL.x + LABEL.w - 3.4, LABEL.y + LABEL.d / 2, 0.8])],
	strip: solid(STRIP, 0, 0.6, 3),
	stripMinor: STRIP_TICKS.minor,
	stripMajor: STRIP_TICKS.major,
};

export interface Cylinder {
	kind: "cylinder";
	key: number;
	x: number;
	y: number;
	barrel: SolidPaths;
	fins: string;
	head: SolidPaths;
	headBolts: Flat[];
	guide: SolidPaths;
	guideTop: number;
	phase: number;
}

export interface Stack {
	kind: "stack";
	key: number;
	x: number;
	y: number;
	pipe: SolidPaths;
	lip: SolidPaths;
	top: number;
}

function cylinderOf(x: number, y: number, r: number, z: number, h: number, phase: number): Cylinder {
	const headZ = z + h;
	const guideZ = headZ + 3;
	return {
		kind: "cylinder",
		key: depthKey([x, y]),
		x,
		y,
		barrel: post(x, y, r, z, h, 36),
		fins: join([0.25, 0.5, 0.75].map((t) => sideArc(x, y, r, z + h * t, P, 28))),
		head: post(x, y, r + 1.4, headZ, 3, 36),
		headBolts: [0, 1, 2, 3].map((index) => {
			const angle = Math.PI / 4 + (index * Math.PI) / 2;
			return at([x + Math.cos(angle) * (r - 1), y + Math.sin(angle) * (r - 1), headZ + 3]);
		}),
		guide: post(x, y, 3, guideZ, 3, 20),
		guideTop: guideZ + 3,
		phase,
	};
}

function stackOf(x: number, y: number, z: number, h: number): Stack {
	return { kind: "stack", key: depthKey([x, y]), x, y, pipe: post(x, y, 2.2, z, h, 20), lip: post(x, y, 3, z + h - 1.6, 1.6, 20), top: z + h };
}

export interface Engine {
	id: EngineId;
	mount: SolidPaths;
	mountEdge: string;
	mountBolts: Flat[];
	crankcase: SolidPaths;
	caseSeam: string;
	caseBolts: Flat[];
	sump: SolidPaths;
	parts: (Cylinder | Stack)[];
	flange: SolidPaths;
	flangeBolts: Flat[];
}

function engineOf(id: EngineId, mount: Plan, crank: Plan, crankH: number, cylinders: Cylinder[], stacks: Stack[], flangeX: number, flangeFacing: boolean): Engine {
	const top = 3 + crankH;
	const sump: Plan = { x: crank.x + 6, y: crank.y + crank.d, w: crank.w - 12, d: 2.4, r: 0.8 };
	return {
		id,
		mount: solid(mount, 0, 3, 4, 0.6),
		mountEdge: planOutline(insetPlan(mount, 2.4), 3, P, 4),
		mountBolts: corners(mount, 3.6).map(([x, y]) => at([x, y, 3])),
		crankcase: solid(crank, 3, crankH, 4, 1),
		caseSeam: sideSeam(crank, 3 + crankH * 0.42, P, 4),
		caseBolts: corners(crank, 3).map(([x, y]) => at([x, y, top])),
		sump: solid(sump, 3, crankH * 0.3, 3, 0.4),
		parts: [...cylinders, ...stacks].sort((a, b) => a.key - b.key),
		flange: solid({ x: flangeX, y: -5, w: 2, d: 10, r: 1 }, SHAFT_Z - 3, SHAFT_H + 6, 3, 0.3),
		flangeBolts: flangeFacing ? [at([flangeX + 2, -3, SHAFT_Z + SHAFT_H + 1.6]), at([flangeX + 2, 3, SHAFT_Z + SHAFT_H + 1.6]), at([flangeX + 2, -3, SHAFT_Z - 1.6]), at([flangeX + 2, 3, SHAFT_Z - 1.6])] : [],
	};
}

const LEAN_MOUNT: Plan = { x: -162, y: -26, w: 62, d: 52, r: 3 };
const LEAN_CRANK: Plan = { x: -152, y: -17, w: 42, d: 34, r: 2.5 };
const LEAN_H = 18;
const LEAN = engineOf(
	"native",
	LEAN_MOUNT,
	LEAN_CRANK,
	LEAN_H,
	[cylinderOf(-131, 0, 9, 3 + LEAN_H, 16, 0)],
	[stackOf(-147, 11, 3 + LEAN_H, 12)],
	LEAN_CRANK.x + LEAN_CRANK.w,
	true,
);

const HEAVY_MOUNT: Plan = { x: 96, y: -34, w: 94, d: 68, r: 3 };
const HEAVY_CRANK: Plan = { x: 104, y: -26, w: 80, d: 52, r: 3 };
const HEAVY_H = 22;
const FIRING = [0, 4, 2, 5, 1, 3];
const HEAVY = engineOf(
	"electron",
	HEAVY_MOUNT,
	HEAVY_CRANK,
	HEAVY_H,
	[116, 138, 160].flatMap((x, column) => [-12, 12].map((y, row) => cylinderOf(x, y, 8, 3 + HEAVY_H, 16, (FIRING[column * 2 + row]! / 6) * Math.PI * 2))),
	[stackOf(177, -15, 3 + HEAVY_H, 13), stackOf(177, 15, 3 + HEAVY_H, 13)],
	HEAVY_CRANK.x - 2,
	false,
);

export const ENGINES: Record<EngineId, Engine> = { native: LEAN, electron: HEAVY };

const HOUSING: Plan = { x: -18, y: -20, w: 36, d: 40, r: 3 };
const HOUSING_H = 26;
const DRUM_R = 12;
const faceSlot = (y: number, x0: number, x1: number, z0: number, z1: number) => pathOf([at([x0, y, z0]), at([x1, y, z0]), at([x1, y, z1]), at([x0, y, z1])], true);
const sideSlot = (x: number, y0: number, y1: number, z0: number, z1: number) => pathOf([at([x, y0, z0]), at([x, y1, z0]), at([x, y1, z1]), at([x, y0, z1])], true);
export const HOUSING_ART = {
	plinth: solid(insetPlan(HOUSING, -2.5), 0, 2, 4, 0.5),
	plinthBolts: corners(insetPlan(HOUSING, -2.5), 1.4).map(([x, y]) => at([x, y, 2])),
	body: solid(HOUSING, 2, HOUSING_H, 4, 1),
	seam: sideSeam(HOUSING, 2 + HOUSING_H - 5, P, 4),
	vents: join([-10, 0, 10].map((x) => faceSlot(HOUSING.y + HOUSING.d, x - 2.4, x + 2.4, 6, 18))),
	sideVents: join([-10, 0, 10].map((y) => sideSlot(HOUSING.x + HOUSING.w, y - 2.4, y + 2.4, 18, 23))),
	collar: post(0, 0, DRUM_R + 2, 2 + HOUSING_H, 2, 40),
	drum: post(0, 0, DRUM_R, 4 + HOUSING_H, 9, 40),
	knurl: knurl(0, 0, DRUM_R, 4 + HOUSING_H, 13 + HOUSING_H, 48, P),
	track: ring(0, 0, DRUM_R - 3, 13 + HOUSING_H, P, 40),
	crown: post(0, 0, 4.5, 13 + HOUSING_H, 2.6, 24),
	flanges: [-20, 18].map((x) => solid({ x, y: -5, w: 2, d: 10, r: 1 }, SHAFT_Z - 3, SHAFT_H + 6, 3, 0.3)),
	flangeBolts: [at([20, -3, SHAFT_Z + SHAFT_H + 1.6]), at([20, 3, SHAFT_Z + SHAFT_H + 1.6]), at([20, -3, SHAFT_Z - 1.6]), at([20, 3, SHAFT_Z - 1.6])],
};
export const drumMark = (angle: number) => at([Math.cos(angle) * (DRUM_R - 3), Math.sin(angle) * (DRUM_R - 3), 13 + HOUSING_H]);

const shaftOf = (x0: number, x1: number) => solid({ x: x0, y: -SHAFT_H / 2, w: x1 - x0, d: SHAFT_H, r: 0.8 }, SHAFT_Z, SHAFT_H, 3, 0.2);

function pillowOf(x: number) {
	const block: Plan = { x: x - 4, y: -7, w: 8, d: 14, r: 1.2 };
	const foot: Plan = { x: x - 7, y: -10, w: 14, d: 20, r: 1.2 };
	return {
		foot: solid(foot, 0, 2, 3, 0.4),
		footBolts: [at([x - 5, -8, 2]), at([x + 5, -8, 2]), at([x - 5, 8, 2]), at([x + 5, 8, 2])],
		block: solid(block, 2, SHAFT_Z + SHAFT_H + 3 - 2, 3, 0.5),
		cap: sideSeam(block, SHAFT_Z + SHAFT_H / 2, P, 3),
		capBolts: [at([x, -4.5, SHAFT_Z + SHAFT_H + 3]), at([x, 4.5, SHAFT_Z + SHAFT_H + 3])],
	};
}

export const SHAFTS = {
	leanOut: shaftOf(LEAN_CRANK.x + LEAN_CRANK.w + 2, -66),
	leanIn: shaftOf(-58, -20),
	heavyIn: shaftOf(20, 58),
	heavyOut: shaftOf(66, HEAVY_CRANK.x - 2),
};
export const PILLOWS = [pillowOf(-62), pillowOf(62)];

const DIAL_R = 11;
const DIAL_Z = 32;
const SWEEP_FROM = (Math.PI * 7) / 6;
const SWEEP = (Math.PI * 4) / 3;
const angleOf = (fraction: number) => SWEEP_FROM - Math.max(0, Math.min(1, fraction)) * SWEEP;

function dialTicks(cx: number, steps: number, every: number) {
	const minor: string[] = [];
	const major: string[] = [];
	for (let step = 0; step <= steps; step++) {
		const angle = angleOf(step / steps);
		const big = step % every === 0;
		const outer = DIAL_R - 1.2;
		const inner = outer - (big ? 2.6 : 1.4);
		const point = (r: number): Point3 => [cx + Math.cos(angle) * r, PANEL_FACE, DIAL_Z + Math.sin(angle) * r];
		(big ? major : minor).push(segment(point(inner), point(outer)));
	}
	return { minor: join(minor), major: join(major) };
}

function dialOf(cx: number, steps: number, every: number) {
	return {
		cx,
		bezel: sideRing([cx, PANEL_FACE, DIAL_Z], DIAL_R + 1.2, "y", P, 48),
		face: sideRing([cx, PANEL_FACE, DIAL_Z], DIAL_R, "y", P, 48),
		...dialTicks(cx, steps, every),
		pivot: at([cx, PANEL_FACE, DIAL_Z]),
	};
}
export type Dial = ReturnType<typeof dialOf>;

export function needleOf(cx: number, fraction: number) {
	const angle = angleOf(fraction);
	const tip: Point3 = [cx + Math.cos(angle) * (DIAL_R - 2), PANEL_FACE, DIAL_Z + Math.sin(angle) * (DIAL_R - 2)];
	const tail: Point3 = [cx - Math.cos(angle) * 2.2, PANEL_FACE, DIAL_Z - Math.sin(angle) * 2.2];
	return segment(tail, tip);
}

function panelOf(id: EngineId, x: number, w: number, crank: Plan, mount: Plan, leadX: number) {
	const plan: Plan = { x, y: PANEL_Y, w, d: PANEL_D, r: 2 };
	const mid = x + w / 2;
	const from: Point3 = [leadX, crank.y, 9];
	return {
		id,
		plinth: solid(insetPlan(plan, -2), 0, PANEL_Z, 4, 0.5),
		cabinet: solid(plan, PANEL_Z, PANEL_H, 4, 1),
		seam: sideSeam(plan, PANEL_Z + PANEL_H - 6, P, 4),
		screws: [x + 4, x + w - 4].flatMap((sx) => [at([sx, PANEL_FACE, PANEL_Z + PANEL_H - 10]), at([sx, PANEL_FACE, PANEL_Z + 5])]),
		memory: dialOf(mid - 17, 12, 4),
		cpu: dialOf(mid + 17, 10, 5),
		lamp: at([mid, PANEL_FACE, 14]),
		lampRing: sideRing([mid, PANEL_FACE, 14], 2.4, "y", P, 18),
		socket: sideRing([leadX, PANEL_FACE, 9], 1.8, "y", P, 14),
		lead: {
			from: at(from),
			to: at([leadX, PANEL_FACE, 9]),
			via: [at([leadX, crank.y - 1.5, 7]), at([leadX, crank.y - 1.5, 3.5]), at([leadX, mount.y + 1, 3.5]), at([leadX, mount.y - 1.5, 0.5]), at([leadX, PANEL_FACE + 2.5, 0.5]), at([leadX, PANEL_FACE + 1.2, 6])],
		},
	};
}
export type Panel = ReturnType<typeof panelOf>;

export const PANELS: Record<EngineId, Panel> = {
	native: panelOf("native", -168, 70, LEAN_CRANK, LEAN_MOUNT, -140),
	electron: panelOf("electron", 110, 78, HEAVY_CRANK, HEAVY_MOUNT, 126),
};

export function rodOf(part: Cylinder, lift: number) {
	const height = 2.4 + lift * 4.2;
	return { paths: post(part.x, part.y, 1.5, part.guideTop, height, 16), tip: at([part.x, part.y, part.guideTop + height]) };
}

export const heatAt = (stack: Stack, rise: number, drift: number) => at([stack.x + drift, stack.y, stack.top + rise]);
