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

export const WIDTH = 600;
export const HEIGHT = 360;
const AZIMUTH = 57;

const PLATE: Plan = { x: -176, y: -64, w: 352, d: 132, r: 8 };
const PLATE_H = 10;
const FOOT_H = 5;

const TAXI: Plan = { x: -156, y: -40, w: 76, d: 52, r: 6 };
const TAXI_Z = 3;
const TAXI_TOP = 55;
const TAXI_FACE = TAXI.y + TAXI.d;
const BANK: Plan = { x: -46, y: -42, w: 92, d: 58, r: 5 };
const BANK_TOP = 22;
const BANK_FACE = BANK.y + BANK.d;
const METER: Plan = { x: 84, y: -40, w: 70, d: 50, r: 8 };
const METER_Z = 12;
const METER_TOP = 62;
const METER_FACE = METER.y + METER.d;
const CLOCK: Plan = { x: -26, y: 28, w: 52, d: 24, r: 4 };
const CLOCK_TOP = 36;
const CLOCK_FACE = CLOCK.y + CLOCK.d;
const STRIP: Plan = { x: -120, y: 56, w: 272, d: 7, r: 1 };
export const DAYS = 22;
const DAY_FROM = STRIP.x + 6;
export const DAY_STEP = (STRIP.w - 12) / (DAYS - 1);

export const P = fitProjection(
	[
		[PLATE.x, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[PLATE.x, PLATE.y, 0],
		[PLATE.x + PLATE.w, PLATE.y + PLATE.d, 0],
		[-84, -40, 76],
		[-156, -40, 66],
		[154, -40, 66],
	],
	WIDTH,
	HEIGHT,
	{ pad: 16, azimuth: AZIMUTH },
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
		minor: Array.from({ length: DAYS }, (_, day) => day).filter((day) => day % 5 !== 0).map((day) => lineOnTop([DAY_FROM + day * DAY_STEP, STRIP.y + 0.4], [DAY_FROM + day * DAY_STEP, STRIP.y + 2.4], 0.6, P)).join(""),
		major: Array.from({ length: DAYS }, (_, day) => day).filter((day) => day % 5 === 0).map((day) => lineOnTop([DAY_FROM + day * DAY_STEP, STRIP.y + 0.4], [DAY_FROM + day * DAY_STEP, STRIP.y + 4.2], 0.6, P)).join(""),
	},
	label: slab({ x: -166, y: 48, w: 38, d: 15, r: 1.5 }, 0, 0.8, 4),
	labelGroove: planOutline({ x: -164.4, y: 49.6, w: 34.8, d: 11.8, r: 0.8 }, 0.8, P, 4),
	labelRules: [lineOnTop([-160, 53], [-138, 53], 0.8, P), lineOnTop([-160, 56.5], [-146, 56.5], 0.8, P), lineOnTop([-160, 60], [-142, 60], 0.8, P)].join(""),
	labelScrews: dots([[-131.5, 52.5, 0.8], [-131.5, 59.5, 0.8]]),
};

export const RIDER = {
	body: slab({ x: DAY_FROM - 3, y: STRIP.y - 2, w: 6, d: 11, r: 1.2 }, 0.6, 3.4, 3, 0.4),
	index: lineOnTop([DAY_FROM, STRIP.y - 1.2], [DAY_FROM, STRIP.y + 8.2], 4, P),
};

export const TAXI_ART = {
	plinth: slab({ x: TAXI.x - 4, y: TAXI.y - 4, w: TAXI.w + 8, d: TAXI.d + 8, r: 4 }, 0, TAXI_Z, 4, 0.6),
	body: slab(TAXI, TAXI_Z, TAXI_TOP - TAXI_Z, 8, 1),
	seam: sideSeam(TAXI, 47, P, 8),
	screws: dots([[TAXI.x + 6, TAXI_FACE, 51], [TAXI.x + TAXI.w - 6, TAXI_FACE, 51], [TAXI.x + 6, TAXI_FACE, 8], [TAXI.x + TAXI.w - 6, TAXI_FACE, 8]]),
	roof: slab({ x: -136, y: -26, w: 34, d: 16, r: 3 }, TAXI_TOP, 7, 4, 0.5),
	roofBase: slab({ x: -139, y: -29, w: 40, d: 22, r: 3 }, TAXI_TOP, 1.2, 4),
	buttons: [post(-148, 2, 3, TAXI_TOP, 2.4, 20), post(-139, 2, 3, TAXI_TOP, 2.4, 20)],
	flagPost: post(-84, -24, 2, TAXI_TOP, 6, 20),
	flagUp: slab({ x: -85.6, y: -36, w: 3.2, d: 24, r: 0.8 }, TAXI_TOP + 6, 14, 3, 0.4),
	flagDown: slab({ x: -80, y: -36, w: 3, d: 24, r: 0.8 }, 30, 16, 3, 0.4),
	window: { transform: face(TAXI.x + 8, TAXI_FACE, 42), width: 60, height: 16 },
	legend: { transform: face(TAXI.x + 8, TAXI_FACE, 22), width: 60 },
};

export const SHAFTS = {
	left: [
		slab({ x: -68, y: -21, w: 10, d: 14, r: 1.5 }, 0, 8.4, 4, 0.5),
		slab({ x: -80, y: -16.5, w: 34, d: 5, r: 2 }, 8.4, 5, 4),
		slab({ x: -75, y: -19, w: 5, d: 10, r: 1.2 }, 6.4, 9, 3, 0.4),
		slab({ x: -55, y: -19, w: 5, d: 10, r: 1.2 }, 6.4, 9, 3, 0.4),
	],
	right: [
		slab({ x: 58, y: -21, w: 10, d: 14, r: 1.5 }, 0, 8.4, 4, 0.5),
		slab({ x: 46, y: -16.5, w: 38, d: 5, r: 2 }, 8.4, 5, 4),
		slab({ x: 50, y: -19, w: 5, d: 10, r: 1.2 }, 6.4, 9, 3, 0.4),
		slab({ x: 72, y: -19, w: 5, d: 10, r: 1.2 }, 6.4, 9, 3, 0.4),
	],
	front: [slab({ x: -2.5, y: BANK_FACE, w: 5, d: CLOCK.y - BANK_FACE, r: 2 }, 8.4, 5, 4), slab({ x: -5, y: 20, w: 10, d: 5, r: 1.2 }, 6.4, 9, 3, 0.4)],
	leftScrews: dots([[-65, -18, 8.4], [-61, -10, 8.4]]),
	rightScrews: dots([[61, -18, 8.4], [65, -10, 8.4]]),
};

const LAMPS: Point3[] = [
	[-28, -26, BANK_TOP],
	[0, -26, BANK_TOP],
	[28, -26, BANK_TOP],
	[-28, 0, BANK_TOP],
	[0, 0, BANK_TOP],
	[28, 0, BANK_TOP],
];

export const BANK_ART = {
	body: slab(BANK, 0, BANK_TOP, 8, 1),
	seam: sideSeam(BANK, 4, P, 8),
	lamps: LAMPS.map(([x, y, z], index) => ({
		index,
		collar: post(x, y, 6.2, z, 1.6, 32),
		lamp: post(x, y, 4.4, z + 1.6, 5.4, 32),
		ring: ring(x, y, 2.4, z + 7, P, 20),
	})),
	screws: dots(corners(BANK, 6).map(([x, y]) => [x, y, BANK_TOP])),
	vents: { transform: face(BANK.x, BANK_FACE, BANK_TOP) },
};

export const METER_ART = {
	plinth: slab({ x: METER.x - 4, y: METER.y - 4, w: METER.w + 8, d: METER.d + 8, r: 4 }, 0, METER_Z, 6, 0.8),
	plinthSeam: sideSeam({ x: METER.x - 4, y: METER.y - 4, w: METER.w + 8, d: METER.d + 8, r: 4 }, 8, P, 6),
	body: slab(METER, METER_Z, METER_TOP - METER_Z, 10, 1),
	lid: slab(insetPlan(METER, 3), METER_TOP, 3, 10, 0.6),
	lugs: [post(98, -28, 3.2, METER_TOP + 3, 4, 24), post(140, -28, 3.2, METER_TOP + 3, 4, 24)],
	seal: post(146, 2, 1.8, METER_TOP + 3, 2.2, 16),
	screws: dots([[METER.x + 7, METER_FACE, 58], [METER.x + METER.w - 7, METER_FACE, 58], [METER.x + 7, METER_FACE, 16], [METER.x + METER.w - 7, METER_FACE, 16]]),
	dial: { transform: face(METER.x + METER.w / 2, METER_FACE, 42) },
	window: { transform: face(METER.x + 17, METER_FACE, 27), width: 36, height: 9 },
};

export const CLOCK_ART = {
	body: slab(CLOCK, 0, CLOCK_TOP, 6, 1),
	seam: sideSeam(CLOCK, 30, P, 6),
	slot: planOutline({ x: -16, y: 33, w: 24, d: 3, r: 1.4 }, CLOCK_TOP, P, 3),
	lamp: post(16, 40, 3.2, CLOCK_TOP, 4, 24),
	lampCollar: post(16, 40, 4.4, CLOCK_TOP, 1, 24),
	leverBoss: slab({ x: 26, y: 37, w: 2.4, d: 6, r: 0.8 }, 20, 8, 3, 0.3),
	lever: slab({ x: 28.4, y: 38.5, w: 9, d: 3, r: 1.2 }, 22.5, 3, 3, 0.3),
	knob: post(38, 40, 2.6, 21, 5, 20),
	window: { transform: face(CLOCK.x + 6, CLOCK_FACE, 26), width: 40, height: 10 },
};

export const DIAL_TICKS = Array.from({ length: 41 }, (_, index) => {
	const angle = (-135 + (index * 270) / 40) * (Math.PI / 180);
	const major = index % 10 === 0;
	const inner = major ? 10.6 : 11.8;
	return { major, d: pathOf([[Math.sin(angle) * inner, -Math.cos(angle) * inner], [Math.sin(angle) * 13.2, -Math.cos(angle) * 13.2]]) };
});

export const DIAL_RIM = pathOf(circleRing(0, 0, 13.2, 72).map(([x, y]) => [x, y] as Flat), true);
export const DIAL_BEZEL = pathOf(circleRing(0, 0, 16, 72).map(([x, y]) => [x, y] as Flat), true);
export const DIAL_GLASS = pathOf(circleRing(0, 0, 15, 72).map(([x, y]) => [x, y] as Flat), true);
