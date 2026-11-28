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
	segment,
	sideMatrix,
	sideSeam,
	slabOf,
	translateAlong,
	type Plan,
	type Point3,
} from "@/components/iso/iso-kit";

export const WIDTH = 600;
export const HEIGHT = 380;
const AZIMUTH = 57;

const PLATE: Plan = { x: -184, y: -72, w: 344, d: 148, r: 8 };
const PLATE_H = 10;
const FOOT_H = 5;

const BIN_W = 34;
const BIN_D = 28;
const BIN_Y = -56;
const BIN_STEP = 42;
const BIN_X0 = -118;
const BIN_FOOT = 6;
const BIN_TOP = 70;
export const FILL_FROM = BIN_FOOT + 0.6;
export const FILL_MOST = BIN_TOP - FILL_FROM - 4;
const BIN_FACE = BIN_Y + BIN_D;

const HOPPER: Plan = { x: -172, y: -60, w: 40, d: 36, r: 4 };
const HOPPER_TOP = 86;
const HOPPER_FACE = HOPPER.y + HOPPER.d;

const CHUTE: Plan = { x: HOPPER.x + HOPPER.w, y: BIN_Y + 2, w: BIN_X0 + 4 * BIN_STEP + BIN_W - (HOPPER.x + HOPPER.w), d: 8, r: 1.5 };
const CHUTE_Z = BIN_TOP + 2;

const PANEL: Plan = { x: -150, y: 16, w: 272, d: 40, r: 4 };
const PANEL_TOP = 20;
const PANEL_FACE = PANEL.y + PANEL.d;
const SWITCH_X0 = -116;
const SWITCH_STEP = 50;
export const SWITCHES = 5;
const SLOT_Y = 24;
const SLOT_D = 20;
export const KNOB_TRAVEL = 11;

const STRIP: Plan = { x: -108, y: PLATE.y + PLATE.d - 12, w: 248, d: 7, r: 1 };

export const P = fitProjection(
	[
		[PLATE.x, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[HOPPER.x, HOPPER.y, HOPPER_TOP + 6],
		[HOPPER.x + HOPPER.w, HOPPER.y, HOPPER_TOP + 6],
		[CHUTE.x + CHUTE.w, CHUTE.y, CHUTE_Z + 6],
	],
	WIDTH,
	HEIGHT,
	{ pad: 22, azimuth: AZIMUTH },
);

const at = (point: Point3) => iso(point, P);
const slab = (plan: Plan, z: number, h: number, steps = 6, bevel = 0) => slabOf(plan, z, h, P, steps, bevel);
const post = (cx: number, cy: number, r: number, z: number, h: number, steps = 28) => cylinder(cx, cy, r, z, h, P, steps);
const dots = (points: Point3[]) => points.map(at);
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
		minor: Array.from({ length: 63 }, (_, i) => i).filter((i) => i % 5 !== 0).map((i) => lineOnTop([STRIP.x + 2 + i * 4, STRIP.y + 0.4], [STRIP.x + 2 + i * 4, STRIP.y + 2.4], 0.6, P)).join(""),
		major: Array.from({ length: 63 }, (_, i) => i).filter((i) => i % 5 === 0).map((i) => lineOnTop([STRIP.x + 2 + i * 4, STRIP.y + 0.4], [STRIP.x + 2 + i * 4, STRIP.y + 4.2], 0.6, P)).join(""),
	},
	label: slab({ x: -176, y: 52, w: 20, d: 18, r: 1.5 }, 0, 0.8, 4),
	labelGroove: planOutline({ x: -174.4, y: 53.6, w: 16.8, d: 14.8, r: 0.8 }, 0.8, P, 4),
	labelRules: [lineOnTop([-171, 57], [-171, 67], 0.8, P), lineOnTop([-167, 57], [-167, 64], 0.8, P), lineOnTop([-163, 57], [-163, 66], 0.8, P)].join(""),
	labelScrews: dots([[-160, 55.5, 0.8], [-160, 67.5, 0.8]]),
};

export const HOPPER_ART = {
	plinth: slab({ x: HOPPER.x - 3, y: HOPPER.y - 3, w: HOPPER.w + 6, d: HOPPER.d + 6, r: 4 }, 0, 4, 6, 0.6),
	body: slab(HOPPER, 4, HOPPER_TOP - 4, 6, 1),
	seamLow: sideSeam(HOPPER, 16, P, 6),
	seamHigh: sideSeam(HOPPER, 74, P, 6),
	collar: slab(insetPlan(HOPPER, -1.5), HOPPER_TOP, 3, 6, 0.6),
	mouthRim: planOutline(insetPlan(HOPPER, 6), HOPPER_TOP + 3, P, 4),
	mouth: planOutline(insetPlan(HOPPER, 7), HOPPER_TOP + 3, P, 4),
	window: { transform: face(HOPPER.x + 8, HOPPER_FACE, 52), width: 24, height: 11 },
	vents: { transform: face(HOPPER.x + 8, HOPPER_FACE, 32) },
	screws: dots([[HOPPER.x + 5, HOPPER_FACE, 70], [HOPPER.x + HOPPER.w - 5, HOPPER_FACE, 70], [HOPPER.x + 5, HOPPER_FACE, 20], [HOPPER.x + HOPPER.w - 5, HOPPER_FACE, 20]]),
};

function bin(index: number) {
	const x = BIN_X0 + index * BIN_STEP;
	const plan: Plan = { x, y: BIN_Y, w: BIN_W, d: BIN_D, r: 2 };
	const inner: Plan = { x: x + 2.4, y: BIN_Y + 2.4, w: BIN_W - 4.8, d: BIN_D - 4.8, r: 1 };
	const postAt = (px: number, py: number) => slab({ x: px - 1.6, y: py - 1.6, w: 3.2, d: 3.2, r: 0.6 }, BIN_FOOT, BIN_TOP - BIN_FOOT, 2, 0.3);
	const scaleX = x + BIN_W - 5.6;
	const scale: Plan = { x: scaleX, y: BIN_FACE + 0.4, w: 3, d: 0.8, r: 0.2 };
	const tickZ = Array.from({ length: 11 }, (_, i) => FILL_FROM + (i * FILL_MOST) / 10);
	return {
		index,
		at: [x + BIN_W / 2, BIN_Y + BIN_D / 2, 0] as Point3,
		inner,
		foot: slab({ x: x - 2, y: BIN_Y - 2, w: BIN_W + 4, d: BIN_D + 4, r: 2.6 }, 0, BIN_FOOT, 4, 0.6),
		floor: planOutline(insetPlan(plan, 2.4), BIN_FOOT, P, 3),
		back: slab({ x, y: BIN_Y, w: BIN_W, d: 1.6, r: 0.4 }, BIN_FOOT, BIN_TOP - BIN_FOOT, 2),
		side: slab({ x: x + BIN_W - 1.2, y: BIN_Y + 1.6, w: 1.2, d: BIN_D - 3.2, r: 0.3 }, BIN_FOOT, BIN_TOP - BIN_FOOT, 2),
		frontPosts: [postAt(x + 1.6, BIN_FACE - 1.6), postAt(x + BIN_W - 1.6, BIN_FACE - 1.6)],
		glass: slab({ x: x + 3.2, y: BIN_FACE - 1.4, w: BIN_W - 6.4, d: 0.8, r: 0.2 }, BIN_FOOT, BIN_TOP - BIN_FOOT, 2),
		scale: slab(scale, BIN_FOOT + 2, BIN_TOP - BIN_FOOT - 6, 2),
		ticks: {
			minor: tickZ.filter((_, i) => i % 5 !== 0).map((z) => segment([scaleX, BIN_FACE + 1.2, z], [scaleX + 1.4, BIN_FACE + 1.2, z], P)).join(""),
			major: tickZ.filter((_, i) => i % 5 === 0).map((z) => segment([scaleX, BIN_FACE + 1.2, z], [scaleX + 2.4, BIN_FACE + 1.2, z], P)).join(""),
		},
		cap: slab({ x: x - 1, y: BIN_Y - 1, w: BIN_W + 2, d: BIN_D + 2, r: 2.2 }, BIN_TOP, 2, 4, 0.4),
		capMouth: planOutline(insetPlan(plan, 3), BIN_TOP + 2, P, 3),
		gate: slab({ x: x + BIN_W / 2 - 5, y: BIN_Y + 3, w: 10, d: 4, r: 0.8 }, CHUTE_Z + 3, 1.6, 3, 0.3),
		gateScrews: dots([[x + BIN_W / 2 - 3.2, BIN_Y + 5, CHUTE_Z + 4.6], [x + BIN_W / 2 + 3.2, BIN_Y + 5, CHUTE_Z + 4.6]]),
		capScrews: dots([[x + 1.2, BIN_FACE - 0.4, BIN_TOP + 2], [x + BIN_W - 1.2, BIN_FACE - 0.4, BIN_TOP + 2]]),
	};
}

export const BINS_ART = [0, 1, 2, 3, 4].map(bin);

export function fillPaths(index: number, height: number) {
	return slabOf(BINS_ART[index].inner, FILL_FROM, Math.max(0.6, height), P, 3, 0);
}

export function levelLine(index: number, height: number) {
	return planOutline(insetPlan(BINS_ART[index].inner, 0.4), FILL_FROM + Math.max(0.6, height), P, 3);
}

export const CHUTE_ART = {
	trough: slab(CHUTE, CHUTE_Z, 3, 3, 0.4),
	lip: slab({ x: CHUTE.x, y: CHUTE.y + CHUTE.d - 1.2, w: CHUTE.w, d: 1.2, r: 0.3 }, CHUTE_Z + 3, 2, 2),
	flange: slab({ x: CHUTE.x - 1, y: CHUTE.y - 2, w: 3, d: CHUTE.d + 4, r: 0.6 }, CHUTE_Z - 2, 8, 2),
	flangeScrews: dots([[CHUTE.x + 2, CHUTE.y + CHUTE.d + 2, CHUTE_Z + 4.4], [CHUTE.x + 2, CHUTE.y + CHUTE.d + 2, CHUTE_Z - 0.6]]),
};

function toggle(index: number) {
	const x = SWITCH_X0 + index * SWITCH_STEP;
	return {
		index,
		bezel: slab({ x: x - 6, y: SLOT_Y - 2, w: 12, d: SLOT_D + 4, r: 2 }, PANEL_TOP, 0.8, 4, 0.2),
		slot: planOutline({ x: x - 2.4, y: SLOT_Y, w: 4.8, d: SLOT_D, r: 2.2 }, PANEL_TOP + 0.8, P, 4),
		slotFloor: slab({ x: x - 2.4, y: SLOT_Y, w: 4.8, d: SLOT_D, r: 2.2 }, PANEL_TOP + 0.8, 0.01, 4),
		knob: slab({ x: x - 4.4, y: SLOT_Y + 0.6, w: 8.8, d: 6.4, r: 1.6 }, PANEL_TOP + 3.8, 4.2, 4, 0.5),
		grip: [0, 1, 2].map((row) => lineOnTop([x - 3.9, SLOT_Y + 2.6 + row * 1.2], [x + 3.9, SLOT_Y + 2.6 + row * 1.2], PANEL_TOP + 8, P)).join(""),
		lampCollar: post(x + 10, SLOT_Y + 4, 3, PANEL_TOP, 1, 20),
		lamp: post(x + 10, SLOT_Y + 4, 2.1, PANEL_TOP + 1, 2.4, 20),
		pin: post(x, SLOT_Y + 7.6, 1.4, PANEL_TOP + 0.8, 7, 16),
		pinHead: post(x, SLOT_Y + 7.6, 2.4, PANEL_TOP + 7.8, 1.4, 16),
		marks: [lineOnTop([x + 6, SLOT_Y + 3], [x + 8, SLOT_Y + 3], PANEL_TOP + 0.8, P), lineOnTop([x + 6, SLOT_Y + 3 + KNOB_TRAVEL], [x + 7.2, SLOT_Y + 3 + KNOB_TRAVEL], PANEL_TOP + 0.8, P)].join(""),
	};
}

export const SWITCH_ART = Array.from({ length: SWITCHES }, (_, index) => toggle(index));
export const KNOB_OFF = translateAlong(P, [0, KNOB_TRAVEL, 0]);

export const PANEL_ART = {
	plinth: slab({ x: PANEL.x - 3, y: PANEL.y - 3, w: PANEL.w + 6, d: PANEL.d + 6, r: 5 }, 0, 3, 6, 0.6),
	body: slab(PANEL, 3, PANEL_TOP - 3, 8, 1),
	seam: sideSeam(PANEL, 7, P, 8),
	inset: planOutline(insetPlan(PANEL, 3.5), PANEL_TOP, P, 6),
	screws: dots(corners(PANEL, 6.5).map(([x, y]) => [x, y, PANEL_TOP])),
	total: { transform: face(PANEL.x + PANEL.w - 96, PANEL_FACE, 17.4), width: 88, height: 9.4 },
	tokens: { transform: face(PANEL.x + PANEL.w - 150, PANEL_FACE, 17.4), width: 46, height: 9.4 },
	faceScrews: dots([[PANEL.x + 6, PANEL_FACE, 13], [PANEL.x + 16, PANEL_FACE, 13]]),
};
