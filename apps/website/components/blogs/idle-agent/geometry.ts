import {
	circleRing,
	corners,
	cylinder,
	depthOf,
	fitProjection,
	insetPlan,
	iso,
	lineOnTop,
	pathOf,
	planOutline,
	ring,
	sideMatrix,
	sideSeam,
	slabOf,
	type Flat,
	type Plan,
	type Point3,
} from "@/components/iso/iso-kit";
import { DIAL_MAX, QUIET_MINUTES } from "./model";

export const WIDTH = 600;
export const HEIGHT = 380;
const AZIMUTH = 50;

const PLATE: Plan = { x: -180, y: -104, w: 302, d: 196, r: 8 };
const PLATE_H = 10;
const FOOT_H = 5;

const TRAY: Plan = { x: -150, y: -84, w: 150, d: 124, r: 6 };
const TRAY_H = 6;
const COLUMNS = 5;
const ROWS = 4;
const PITCH = 28;
const CELL_X = TRAY.x + 19;
const CELL_Y = TRAY.y + 20;
const CUP_R = 9;
const CUP_H = 5;

const MANIFOLD: Plan = { x: -150, y: -98, w: 150, d: 8, r: 3 };
const MANIFOLD_Z = 2;
const MANIFOLD_H = 7;

const HOUSING: Plan = { x: 40, y: -84, w: 70, d: 54, r: 6 };
const HOUSING_Z = 4;
const HOUSING_TOP = 72;
const HOUSING_FACE = HOUSING.y + HOUSING.d;

const STRIP: Plan = { x: -140, y: 60, w: 130, d: 8, r: 1 };
const STRIP_FROM = STRIP.x + 6;
export const MINUTE_STEP = (STRIP.w - 12) / QUIET_MINUTES;

export const P = fitProjection(
	[
		[PLATE.x, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[HOUSING.x, HOUSING.y, HOUSING_TOP + 10],
		[HOUSING.x + HOUSING.w, HOUSING.y, HOUSING_TOP + 10],
	],
	WIDTH,
	HEIGHT,
	{ pad: 14, azimuth: AZIMUTH },
);

const at = (point: Point3) => iso(point, P);
const slab = (plan: Plan, z: number, h: number, steps = 6, bevel = 0) => slabOf(plan, z, h, P, steps, bevel);
const post = (cx: number, cy: number, r: number, z: number, h: number, steps = 28) => cylinder(cx, cy, r, z, h, P, steps);
const dots = (points: Point3[]) => points.map(at);
const minus = (a: Flat, b: Flat): Flat => [a[0] - b[0], a[1] - b[1]];
export const EX = minus(at([1, 0, 0]), at([0, 0, 0]));
const face = (x: number, y: number, z: number) => sideMatrix([x, y], z, P, "left");
const byDepth = <T extends { at: Point3 }>(list: T[]) => [...list].sort((a, b) => depthOf(a.at, P) - depthOf(b.at, P));

export const BASE = {
	halo: slabOf(PLATE, -PLATE_H - 6, PLATE_H, P, 8).fill,
	feet: byDepth(corners(PLATE, 22).map(([x, y]) => ({ at: [x, y, 0] as Point3, flange: post(x, y, 7, -PLATE_H - FOOT_H, 1.6), foot: post(x, y, 5.4, -PLATE_H - FOOT_H + 1.6, FOOT_H - 1.6) }))),
	slab: slab(PLATE, -PLATE_H, PLATE_H, 8, 1.6),
	groove: planOutline(insetPlan(PLATE, 5), 0, P),
	screwRings: corners(PLATE, 10).map(([x, y]) => ring(x, y, 2.2, 0, P, 16)).join(""),
	screws: dots(corners(PLATE, 10).map(([x, y]) => [x, y, 0])),
	strip: slab(STRIP, 0, 0.6, 3),
	ticks: {
		minor: Array.from({ length: QUIET_MINUTES * 2 + 1 }, (_, half) => half).filter((half) => half % 2 === 1).map((half) => lineOnTop([STRIP_FROM + (half * MINUTE_STEP) / 2, STRIP.y + 0.4], [STRIP_FROM + (half * MINUTE_STEP) / 2, STRIP.y + 2.2], 0.6, P)).join(""),
		major: Array.from({ length: QUIET_MINUTES + 1 }, (_, minute) => minute).filter((minute) => minute % 5 !== 0).map((minute) => lineOnTop([STRIP_FROM + minute * MINUTE_STEP, STRIP.y + 0.4], [STRIP_FROM + minute * MINUTE_STEP, STRIP.y + 3.4], 0.6, P)).join(""),
		zero: [0, 5, 10].map((minute) => lineOnTop([STRIP_FROM + minute * MINUTE_STEP, STRIP.y + 0.4], [STRIP_FROM + minute * MINUTE_STEP, STRIP.y + 5], 0.6, P)).join(""),
	},
	stop: slab({ x: STRIP_FROM + QUIET_MINUTES * MINUTE_STEP + 3, y: STRIP.y - 1, w: 4, d: 10, r: 1 }, 0, 4, 3, 0.4),
	label: slab({ x: 50, y: 36, w: 44, d: 20, r: 1.5 }, 0, 0.8, 4),
	labelGroove: planOutline({ x: 51.6, y: 37.6, w: 40.8, d: 16.8, r: 0.8 }, 0.8, P, 4),
	labelRules: [lineOnTop([56, 41.5], [82, 41.5], 0.8, P), lineOnTop([56, 46], [74, 46], 0.8, P), lineOnTop([56, 50.5], [78, 50.5], 0.8, P)].join(""),
	labelScrews: dots([[88, 41, 0.8], [88, 51, 0.8]]),
};

export const RIDER = {
	body: slab({ x: STRIP_FROM - 3, y: STRIP.y - 2, w: 6, d: 12, r: 1.2 }, 0.6, 3.4, 3, 0.4),
	index: lineOnTop([STRIP_FROM, STRIP.y - 1.2], [STRIP_FROM, STRIP.y + 9.2], 4, P),
};

export const TRAY_ART = {
	body: slab(TRAY, 0, TRAY_H, 8, 1),
	seam: sideSeam(TRAY, 2.2, P, 8),
	screws: dots(corners(TRAY, 5).map(([x, y]) => [x, y, TRAY_H])),
};

export const MANIFOLD_ART = {
	saddles: [-134, -96, -58, -20].map((x) => slab({ x: x - 4, y: MANIFOLD.y - 1.5, w: 8, d: MANIFOLD.d + 3, r: 1 }, 0, MANIFOLD_Z + 2, 3, 0.3)),
	body: slab(MANIFOLD, MANIFOLD_Z, MANIFOLD_H, 4, 0.6),
	bands: [-134, -96, -58, -20].map((x) => slab({ x: x - 2.5, y: MANIFOLD.y - 0.8, w: 5, d: MANIFOLD.d + 1.6, r: 1 }, MANIFOLD_Z - 0.4, MANIFOLD_H + 0.8, 3, 0.3)),
	cap: slab({ x: MANIFOLD.x + MANIFOLD.w, y: MANIFOLD.y - 1, w: 4, d: MANIFOLD.d + 2, r: 1.2 }, MANIFOLD_Z - 0.6, MANIFOLD_H + 1.2, 3, 0.4),
	feeds: Array.from({ length: COLUMNS }, (_, column) => slab({ x: CELL_X + column * PITCH - 2, y: MANIFOLD.y + MANIFOLD.d, w: 4, d: TRAY.y - MANIFOLD.y - MANIFOLD.d + 1, r: 1 }, MANIFOLD_Z + 1.5, 3.5, 3)),
	valveBody: slab({ x: MANIFOLD.x - 12, y: MANIFOLD.y - 2, w: 12, d: MANIFOLD.d + 4, r: 2 }, 0, MANIFOLD_Z + MANIFOLD_H + 2, 4, 0.6),
	valveStem: post(MANIFOLD.x - 6, MANIFOLD.y + MANIFOLD.d / 2, 2, MANIFOLD_Z + MANIFOLD_H + 2, 3, 20),
	leverOpen: slab({ x: MANIFOLD.x - 22, y: MANIFOLD.y + MANIFOLD.d / 2 - 1.6, w: 19, d: 3.2, r: 1.4 }, MANIFOLD_Z + MANIFOLD_H + 5, 2.4, 3, 0.3),
	leverShut: slab({ x: MANIFOLD.x - 6 - 1.6, y: MANIFOLD.y + MANIFOLD.d / 2 - 3, w: 3.2, d: 19, r: 1.4 }, MANIFOLD_Z + MANIFOLD_H + 5, 2.4, 3, 0.3),
	valveScrews: dots([[MANIFOLD.x - 9.5, MANIFOLD.y + MANIFOLD.d + 2, 5], [MANIFOLD.x - 2.5, MANIFOLD.y + MANIFOLD.d + 2, 5]]),
};

export const CELLS = byDepth(
	Array.from({ length: COLUMNS * ROWS }, (_, index) => {
		const column = index % COLUMNS;
		const row = Math.floor(index / COLUMNS);
		const x = CELL_X + column * PITCH;
		const y = CELL_Y + row * PITCH;
		const z = TRAY_H;
		return {
			index,
			at: [x, y, z] as Point3,
			seat: post(x, y, CUP_R + 2, z, 1.2, 36),
			cup: post(x, y, CUP_R, z + 1.2, CUP_H, 36),
			ports: ring(x, y, CUP_R - 2.2, z + 1.2 + CUP_H, P, 32),
			crown: post(x, y, CUP_R - 3.6, z + 1.2 + CUP_H, 1.6, 30),
			cap: post(x, y, 2.6, z + 2.8 + CUP_H, 1.4, 20),
			pilot: post(x + CUP_R - 2.4, y - 3.2, 1.5, z + 1.2 + CUP_H, 3.2, 16),
			flame: post(x, y, CUP_R - 3.6, z + 2.8 + CUP_H, 4.2, 30),
		};
	}),
);

export const HOUSING_ART = {
	plinth: slab({ x: HOUSING.x - 4, y: HOUSING.y - 4, w: HOUSING.w + 8, d: HOUSING.d + 8, r: 4 }, 0, HOUSING_Z, 6, 0.8),
	plinthSeam: sideSeam({ x: HOUSING.x - 4, y: HOUSING.y - 4, w: HOUSING.w + 8, d: HOUSING.d + 8, r: 4 }, 2, P, 6),
	body: slab(HOUSING, HOUSING_Z, HOUSING_TOP - HOUSING_Z, 10, 1),
	seam: sideSeam(HOUSING, 22, P, 10),
	lid: slab(insetPlan(HOUSING, 3), HOUSING_TOP, 3, 10, 0.6),
	vents: { transform: face(HOUSING.x, HOUSING_FACE, 18) },
	screws: dots([[HOUSING.x + 6, HOUSING_FACE, 67], [HOUSING.x + HOUSING.w - 6, HOUSING_FACE, 67], [HOUSING.x + 6, HOUSING_FACE, 26], [HOUSING.x + HOUSING.w - 6, HOUSING_FACE, 26]]),
	dial: { transform: face(HOUSING.x + HOUSING.w / 2, HOUSING_FACE, 51) },
	window: { transform: face(HOUSING.x + 15, HOUSING_FACE, 33), width: 44, height: 10 },
	lampCollar: post(HOUSING.x + HOUSING.w - 14, HOUSING.y + 16, 4.6, HOUSING_TOP + 3, 1, 24),
	lamp: post(HOUSING.x + HOUSING.w - 14, HOUSING.y + 16, 3.2, HOUSING_TOP + 4, 4, 24),
	knobBoss: post(HOUSING.x + 16, HOUSING.y + 16, 5, HOUSING_TOP + 3, 1.2, 28),
	knob: post(HOUSING.x + 16, HOUSING.y + 16, 3.8, HOUSING_TOP + 4.2, 5, 28),
	knurl: ring(HOUSING.x + 16, HOUSING.y + 16, 1.4, HOUSING_TOP + 9.2, P, 16),
	tag: slab({ x: HOUSING.x + 28, y: HOUSING.y + 10, w: 22, d: 12, r: 1 }, HOUSING_TOP + 3, 0.6, 3),
	tagRules: [lineOnTop([HOUSING.x + 31, HOUSING.y + 14], [HOUSING.x + 46, HOUSING.y + 14], HOUSING_TOP + 3.6, P), lineOnTop([HOUSING.x + 31, HOUSING.y + 18], [HOUSING.x + 41, HOUSING.y + 18], HOUSING_TOP + 3.6, P)].join(""),
};

const SWEEP = 270;
export const angleOf = (value: number) => -SWEEP / 2 + (Math.max(0, Math.min(DIAL_MAX, value)) / DIAL_MAX) * SWEEP;

export const DIAL_TICKS = Array.from({ length: 51 }, (_, index) => {
	const angle = angleOf(index) * (Math.PI / 180);
	const major = index % 10 === 0;
	const mid = index % 5 === 0;
	const inner = major ? 14.2 : mid ? 15.2 : 16.2;
	return { major, d: pathOf([[Math.sin(angle) * inner, -Math.cos(angle) * inner], [Math.sin(angle) * 17.4, -Math.cos(angle) * 17.4]]) };
});

const circle = (r: number) => pathOf(circleRing(0, 0, r, 72).map(([x, y]) => [x, y] as Flat), true);
export const DIAL_RIM = circle(17.4);
export const DIAL_BEZEL = circle(20.6);
export const DIAL_GLASS = circle(19.6);
