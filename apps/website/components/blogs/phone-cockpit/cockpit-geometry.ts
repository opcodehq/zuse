import {
	corners,
	cylinder,
	depthOf,
	fitProjection,
	insetPlan,
	iso,
	lineOnTop,
	planOutline,
	ring,
	sideMatrix,
	sideSeam,
	slabOf,
	topTicks,
	type Flat,
	type Plan,
	type Point3,
} from "@/components/iso/iso-kit";
import { LOG_LINES, PHASES } from "./cockpit-model";

export const WIDTH = 600;
export const HEIGHT = 400;
const AZIMUTH = 52;

const PLATE: Plan = { x: -164, y: -72, w: 324, d: 152, r: 8 };
const PLATE_H = 10;
const FOOT_H = 5;

const HOUSING: Plan = { x: -148, y: -62, w: 170, d: 40, r: 4 };
const HOUSING_TOP = 80;
const HOUSING_FACE = HOUSING.y + HOUSING.d;
const DECK: Plan = { x: -148, y: HOUSING_FACE, w: 170, d: 52, r: 4 };
const DECK_TOP = 13;

const CRADLE: Plan = { x: 58, y: -32, w: 80, d: 58, r: 6 };
const CRADLE_TOP = 9;
const REST: Plan = { x: 66, y: -24, w: 64, d: 8, r: 2 };
const PHONE: Plan = { x: 70, y: -14, w: 56, d: 8, r: 5 };
const PHONE_TOP = CRADLE_TOP + 106;
const PHONE_FACE = PHONE.y + PHONE.d;
const LIP: Plan = { x: 66, y: PHONE_FACE + 1, w: 64, d: 6, r: 2 };
export const LIFT = 28;

const STRIP: Plan = { x: -148, y: 58, w: 250, d: 7, r: 1 };
const STEPS = PHASES.length;
const RULER_FROM = STRIP.x + 10;
export const RULER_STEP = (STRIP.w - 20) / (STEPS - 1);

export const P = fitProjection(
	[
		[PLATE.x, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[HOUSING.x, HOUSING.y, HOUSING_TOP],
		[HOUSING.x + HOUSING.w, HOUSING.y, HOUSING_TOP],
		[PHONE.x, PHONE.y, PHONE_TOP + LIFT],
		[PHONE.x + PHONE.w, PHONE.y, PHONE_TOP + LIFT],
	],
	WIDTH,
	HEIGHT,
	{ pad: 14, azimuth: AZIMUTH },
);

const at = (point: Point3) => iso(point, P);
const slab = (plan: Plan, z: number, h: number, steps = 6, bevel = 0) => slabOf(plan, z, h, P, steps, bevel);
const post = (cx: number, cy: number, r: number, z: number, h: number, steps = 28) => cylinder(cx, cy, r, z, h, P, steps);
const dots = (points: Point3[]) => points.map(at);
const front = (x: number, y: number, z: number) => sideMatrix([x, y], z, P, "left");
const byDepth = <T extends { at: Point3 }>(list: T[]) => [...list].sort((a, b) => depthOf(a.at, P) - depthOf(b.at, P));

const minus = (a: Flat, b: Flat): Flat => [a[0] - b[0], a[1] - b[1]];
export const UP = minus(at([0, 0, 1]), at([0, 0, 0]));
export const EX = minus(at([1, 0, 0]), at([0, 0, 0]));

const LABEL: Plan = { x: 116, y: 50, w: 34, d: 20, r: 1.5 };

export const BASE = {
	halo: slabOf(PLATE, -PLATE_H - 6, PLATE_H, P, 8).fill,
	feet: byDepth(corners(PLATE, 22).map(([x, y]) => ({ at: [x, y, 0] as Point3, flange: post(x, y, 7, -PLATE_H - FOOT_H, 1.6), foot: post(x, y, 5.4, -PLATE_H - FOOT_H + 1.6, FOOT_H - 1.6) }))),
	slab: slab(PLATE, -PLATE_H, PLATE_H, 8, 1.6),
	groove: planOutline(insetPlan(PLATE, 5), 0, P),
	screwRings: corners(PLATE, 10).map(([x, y]) => ring(x, y, 2.2, 0, P, 16)).join(""),
	screws: dots(corners(PLATE, 10).map(([x, y]) => [x, y, 0])),
	strip: slab(STRIP, 0, 0.6, 3),
	ticks: topTicks(STRIP.x + 4, STRIP.x + STRIP.w - 4, (STRIP.w - 8) / 40, 5, STRIP.y + 0.4, 0.6, [2, 3.8], P),
	stations: PHASES.map((_, index) => lineOnTop([RULER_FROM + index * RULER_STEP, STRIP.y], [RULER_FROM + index * RULER_STEP, STRIP.y + STRIP.d], 0.6, P)).join(""),
	label: slab(LABEL, 0, 0.8, 4),
	labelGroove: planOutline(insetPlan(LABEL, 1.6), 0.8, P, 4),
	labelRules: [lineOnTop([LABEL.x + 4, LABEL.y + 5], [LABEL.x + 24, LABEL.y + 5], 0.8, P), lineOnTop([LABEL.x + 4, LABEL.y + 9], [LABEL.x + 18, LABEL.y + 9], 0.8, P), lineOnTop([LABEL.x + 4, LABEL.y + 13], [LABEL.x + 21, LABEL.y + 13], 0.8, P)].join(""),
	labelScrews: dots([[LABEL.x + LABEL.w - 4, LABEL.y + 4, 0.8], [LABEL.x + LABEL.w - 4, LABEL.y + LABEL.d - 4, 0.8]]),
};

export const RIDER = {
	body: slab({ x: RULER_FROM - 3, y: STRIP.y - 2, w: 6, d: 11, r: 1.2 }, 0.6, 3.4, 3, 0.4),
	index: lineOnTop([RULER_FROM, STRIP.y - 1.2], [RULER_FROM, STRIP.y + 8.2], 4, P),
};

const KEY_W = 12.4;
const KEY_D = 9.4;
const KEY_GAP = 3.4;
const KEY_COLUMNS = 8;
const KEY_ROWS = 3;
const KEYS_X = DECK.x + 10;
const KEYS_Y = DECK.y + 8;

const keyPlans = Array.from({ length: KEY_ROWS }, (_, row) =>
	Array.from({ length: KEY_COLUMNS }, (_, column) => ({ x: KEYS_X + column * (KEY_W + KEY_GAP) + (row === 1 ? 3 : 0), y: KEYS_Y + row * (KEY_D + KEY_GAP), w: KEY_W, d: KEY_D, r: 1.4 })),
).flat();
const spacebar: Plan = { x: KEYS_X + 2 * (KEY_W + KEY_GAP), y: KEYS_Y + KEY_ROWS * (KEY_D + KEY_GAP), w: 4 * KEY_W + 3 * KEY_GAP, d: KEY_D - 1, r: 1.4 };

const LAMP_X = DECK.x + DECK.w - 16;
const LAMP_YS = [DECK.y + 10, DECK.y + 22, DECK.y + 34];

const SCREEN = { x: HOUSING.x + 9, z: HOUSING_TOP - 8, width: HOUSING.w - 18, height: 52 };

export const CONSOLE = {
	housing: slab(HOUSING, 0, HOUSING_TOP, 8, 1.2),
	housingSeam: sideSeam(HOUSING, 14, P, 8),
	lid: slab(insetPlan(HOUSING, 4), HOUSING_TOP, 2, 8, 0.5),
	vents: Array.from({ length: 6 }, (_, index) => planOutline({ x: HOUSING.x + 18 + index * 9, y: HOUSING.y + 12, w: 4, d: 16, r: 2 }, HOUSING_TOP + 2, P, 4)).join(""),
	handle: slab({ x: HOUSING.x + HOUSING.w - 56, y: HOUSING.y + 14, w: 34, d: 8, r: 3 }, HOUSING_TOP + 2, 3, 4, 0.5),
	faceScrews: dots([
		[HOUSING.x + 4.5, HOUSING_FACE, HOUSING_TOP - 4],
		[HOUSING.x + HOUSING.w - 4.5, HOUSING_FACE, HOUSING_TOP - 4],
		[HOUSING.x + 4.5, HOUSING_FACE, 18],
		[HOUSING.x + HOUSING.w - 4.5, HOUSING_FACE, 18],
	]),
	screen: { transform: front(SCREEN.x, HOUSING_FACE, SCREEN.z), width: SCREEN.width, height: SCREEN.height },
	badge: { transform: front(HOUSING.x + 10, HOUSING_FACE, 15) },
	deck: slab(DECK, 0, DECK_TOP, 8, 1),
	deckSeam: sideSeam(DECK, 5, P, 8),
	well: planOutline({ x: KEYS_X - 4, y: KEYS_Y - 4, w: KEY_COLUMNS * (KEY_W + KEY_GAP) + 3 + 5, d: (KEY_ROWS + 1) * (KEY_D + KEY_GAP) + 4, r: 2.5 }, DECK_TOP, P, 4),
	keys: byDepth([...keyPlans, spacebar].map((plan) => ({ at: [plan.x + plan.w / 2, plan.y + plan.d / 2, DECK_TOP] as Point3, paths: slab(plan, DECK_TOP, 2.6, 3, 0.5) }))),
	lamps: LAMP_YS.map((y, index) => ({
		index,
		collar: post(LAMP_X, y, 4.4, DECK_TOP, 1.2, 28),
		lamp: post(LAMP_X, y, 3.1, DECK_TOP + 1.2, 3.2, 28),
		ring: ring(LAMP_X, y, 1.6, DECK_TOP + 4.4, P, 16),
	})),
};

const BUS: Plan = { x: DECK.x + DECK.w, y: 12, w: CRADLE.x - (DECK.x + DECK.w), d: 6, r: 0 };

export const BUS_ART = {
	ribbon: slab(BUS, 0, 0.6, 2),
	traces: [lineOnTop([BUS.x, BUS.y + 2], [BUS.x + BUS.w, BUS.y + 2], 0.6, P), lineOnTop([BUS.x, BUS.y + 4], [BUS.x + BUS.w, BUS.y + 4], 0.6, P)].join(""),
};

const PHONE_SCREEN = { x: PHONE.x + 3.4, z: PHONE_TOP - 4.6, width: PHONE.w - 6.8, height: 84 };
const DOCK_LAMP: [number, number] = [CRADLE.x + CRADLE.w / 2, LIP.y + LIP.d + 8];

export const CRADLE_ART = {
	base: slab(CRADLE, 0, CRADLE_TOP, 8, 1),
	baseSeam: sideSeam(CRADLE, 3, P, 8),
	pads: [post(CRADLE.x + 8, CRADLE.y + CRADLE.d - 8, 2, CRADLE_TOP, 0.8, 16), post(CRADLE.x + CRADLE.w - 8, CRADLE.y + CRADLE.d - 8, 2, CRADLE_TOP, 0.8, 16)],
	rest: slab(REST, CRADLE_TOP, 58, 4, 0.6),
	restSeam: sideSeam(REST, CRADLE_TOP + 46, P, 4),
	pin: slab({ x: PHONE.x + PHONE.w / 2 - 5, y: PHONE.y + 1, w: 10, d: 6, r: 1 }, CRADLE_TOP, 2, 3, 0.3),
	lip: slab(LIP, CRADLE_TOP, 11, 4, 0.6),
	lampCollar: post(DOCK_LAMP[0], DOCK_LAMP[1], 4.4, CRADLE_TOP, 1, 24),
	lamp: post(DOCK_LAMP[0], DOCK_LAMP[1], 3, CRADLE_TOP + 1, 3, 24),
	lampRing: ring(DOCK_LAMP[0], DOCK_LAMP[1], 1.4, CRADLE_TOP + 4, P, 16),
	screws: dots([[CRADLE.x + 5, CRADLE.y + 5, CRADLE_TOP], [CRADLE.x + CRADLE.w - 5, CRADLE.y + 5, CRADLE_TOP]]),
};

export const PHONE_ART = {
	halo: slabOf(PHONE, CRADLE_TOP - 4, 4, P, 6).fill,
	body: slab(PHONE, CRADLE_TOP, PHONE_TOP - CRADLE_TOP, 8, 1.2),
	band: sideSeam(PHONE, PHONE_TOP - 1.4, P, 8),
	button: slab({ x: PHONE.x + PHONE.w, y: PHONE.y + 2.5, w: 1, d: 3, r: 0.4 }, PHONE_TOP - 34, 12, 2, 0.2),
	screen: { transform: front(PHONE_SCREEN.x, PHONE_FACE, PHONE_SCREEN.z), width: PHONE_SCREEN.width, height: PHONE_SCREEN.height },
	chin: { transform: front(PHONE.x + PHONE.w / 2, PHONE_FACE, PHONE_SCREEN.z - PHONE_SCREEN.height - 2) },
};

export const LOG = Array.from({ length: LOG_LINES }, (_, index) => {
	const widths = [0.62, 0.48, 0.71, 0.4, 0.86, 0.55, 0.68, 0.33, 0.77, 0.6, 0.45, 0.8];
	return { y: 5 + index * 3.7, width: widths[index % widths.length] * (SCREEN.width - 12) };
});
