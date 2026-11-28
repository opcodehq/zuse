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
export const HEIGHT = 380;
const AZIMUTH = 55;

const PLATE: Plan = { x: -172, y: -66, w: 330, d: 124, r: 8 };
const PLATE_H = 10;
const FOOT_H = 5;

const RAIL: Plan = { x: -152, y: -36, w: 256, d: 14, r: 1.5 };
const RAIL_H = 4;
const BODY_W = 44;
const BODY_D = 40;
const STEP = 58;
const FIRST = -146;
export const TOP = RAIL_H + 46;
const BODY_Y = -49;
const FRONT = BODY_Y + BODY_D;
const LAMP_Y = FRONT - 9;
const POST_H = 24;
const FLAG_H = 10;
export const FLAG_RISE = { down: 0, half: 4.6, up: 9.4 };

export const BAR_Z = { down: TOP + 0.2, up: TOP + POST_H + 4 };
const BAR_H = 5;
const BAR_Y = -31;
const BAR_D = 6;

const LOCK: Plan = { x: 98, y: 2, w: 40, d: 34, r: 3 };
const LOCK_TOP = 32;
const CTRL: Plan = { x: -156, y: 16, w: 92, d: 32, r: 4 };
const CTRL_TOP = 34;
const CTRL_FACE = CTRL.y + CTRL.d;

export const P = fitProjection(
	[
		[PLATE.x, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y, -PLATE_H - FOOT_H],
		[PLATE.x, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[PLATE.x + PLATE.w, PLATE.y + PLATE.d, -PLATE_H - FOOT_H],
		[FIRST, BODY_Y, BAR_Z.up + BAR_H + 2],
		[FIRST + 3 * STEP + BODY_W, BODY_Y, BAR_Z.up + BAR_H + 2],
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
function hull(points: Flat[]): Flat[] {
	const sorted = [...points].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
	const cross = (o: Flat, a: Flat, b: Flat) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
	const half = (list: Flat[]) => list.reduce<Flat[]>((chain, point) => {
		while (chain.length >= 2 && cross(chain[chain.length - 2], chain[chain.length - 1], point) <= 0) chain.pop();
		chain.push(point);
		return chain;
	}, []);
	const lower = half(sorted);
	const upper = half([...sorted].reverse());
	return [...lower.slice(0, -1), ...upper.slice(0, -1)];
}
export const EZ = minus(at([0, 0, 1]), at([0, 0, 0]));
export const EY = minus(at([0, 1, 0]), at([0, 0, 0]));
export const LEVER_TRAVEL = 12;
const face = (x: number, y: number, z: number) => sideMatrix([x, y], z, P, "left");
const byDepth = <T extends { at: Point3 }>(list: T[]) => [...list].sort((a, b) => depthOf(a.at, P) - depthOf(b.at, P));

export const BASE = {
	halo: slabOf(PLATE, -PLATE_H - 6, PLATE_H, P, 8).fill,
	feet: byDepth(corners(PLATE, 22).map(([x, y]) => ({ at: [x, y, 0] as Point3, flange: post(x, y, 7, -PLATE_H - FOOT_H, 1.6), foot: post(x, y, 5.4, -PLATE_H - FOOT_H + 1.6, FOOT_H - 1.6) }))),
	slab: slab(PLATE, -PLATE_H, PLATE_H, 8, 1.6),
	groove: planOutline(insetPlan(PLATE, 5), 0, P),
	screwRings: corners(PLATE, 10).map(([x, y]) => ring(x, y, 2.2, 0, P, 16)).join(""),
	screws: dots(corners(PLATE, 10).map(([x, y]) => [x, y, 0])),
	label: slab({ x: 10, y: 30, w: 44, d: 16, r: 1.5 }, 0, 0.8, 4),
	labelGroove: planOutline({ x: 11.6, y: 31.6, w: 40.8, d: 12.8, r: 0.8 }, 0.8, P, 4),
	labelRules: [lineOnTop([16, 35.5], [40, 35.5], 0.8, P), lineOnTop([16, 39], [32, 39], 0.8, P), lineOnTop([16, 42.5], [36, 42.5], 0.8, P)].join(""),
	labelScrews: dots([[48, 34.5, 0.8], [48, 41.5, 0.8]]),
};

export const RAIL_ART = {
	rail: slab(RAIL, 0, RAIL_H, 3, 0.4),
	lips: [lineOnTop([RAIL.x + 3, RAIL.y + 3], [RAIL.x + RAIL.w - 3, RAIL.y + 3], RAIL_H, P), lineOnTop([RAIL.x + 3, RAIL.y + RAIL.d - 3], [RAIL.x + RAIL.w - 3, RAIL.y + RAIL.d - 3], RAIL_H, P)].join(""),
	stops: [slab({ x: RAIL.x - 4, y: RAIL.y - 2, w: 4, d: RAIL.d + 4, r: 1 }, 0, 8, 3, 0.4), slab({ x: RAIL.x + RAIL.w, y: RAIL.y - 2, w: 4, d: RAIL.d + 4, r: 1 }, 0, 8, 3, 0.4)],
	bus: slab({ x: -117, y: RAIL.y + RAIL.d, w: 14, d: CTRL.y - 3 - RAIL.y - RAIL.d, r: 1 }, 0, 1.6, 3),
	busScrews: dots([[-110, RAIL.y + RAIL.d + 4, 1.6], [-110, CTRL.y - 7, 1.6]]),
};

export const MODULE_IDS = ["read", "edit", "shell", "network"] as const;
export type ModuleId = (typeof MODULE_IDS)[number];

export const MODULES = byDepth(
	MODULE_IDS.map((id, index) => {
		const x = FIRST + index * STEP;
		const cx = x + BODY_W / 2;
		const plan: Plan = { x, y: BODY_Y, w: BODY_W, d: BODY_D, r: 3 };
		return {
			id,
			index,
			at: [cx, BODY_Y + BODY_D / 2, 0] as Point3,
			foot: slab({ x: x + 6, y: RAIL.y - 1, w: BODY_W - 12, d: RAIL.d + 2, r: 1 }, RAIL_H - 1, 2, 3),
			body: slab(plan, RAIL_H + 1, TOP - RAIL_H - 1, 6, 1),
			seam: sideSeam(plan, TOP - 8, P, 6),
			lid: planOutline(insetPlan(plan, 3.5), TOP, P, 6),
			vents: face(x + 8, FRONT, TOP - 14),
			screws: dots([[x + 4, FRONT, TOP - 4], [x + BODY_W - 4, FRONT, TOP - 4], [x + 4, FRONT, RAIL_H + 6], [x + BODY_W - 4, FRONT, RAIL_H + 6]]),
			collar: post(x + 13, LAMP_Y, 6, TOP, 1.4, 32),
			lamp: post(x + 13, LAMP_Y, 4.4, TOP + 1.4, 4.6, 32),
			lampRing: ring(x + 13, LAMP_Y, 2.2, TOP + 6, P, 20),
			slot: slab({ x: x + 23, y: LAMP_Y - 2, w: 16, d: 4, r: 1.2 }, TOP - 0.01, 0.01, 3),
			flag: slab({ x: x + 24.2, y: LAMP_Y - 0.9, w: 13.6, d: 1.8, r: 0.6 }, TOP - FLAG_H, FLAG_H, 3, 0.3),
			clip: pathOf(hull([x + 24.2, x + 37.8].flatMap((fx) => [LAMP_Y - 0.9, LAMP_Y + 0.9].flatMap((fy) => [at([fx, fy, TOP]), at([fx, fy, TOP + 40])]))), true),
		};
	}),
);

const BAR_FROM = FIRST + STEP - 7;
const BAR_TO = FIRST + 3 * STEP + BODY_W + 7;
export const GUIDES = [BAR_FROM, BAR_TO].map((x) => ({
	at: [x, BAR_Y + BAR_D / 2, 0] as Point3,
	base: post(x, BAR_Y + BAR_D / 2, 3.6, RAIL_H, 2, 24),
	post: post(x, BAR_Y + BAR_D / 2, 1.8, RAIL_H + 2, BAR_Z.up + BAR_H + 3 - RAIL_H - 2, 20),
	cap: post(x, BAR_Y + BAR_D / 2, 2.8, BAR_Z.up + BAR_H + 3, 1.4, 20),
}));

export const BAR = {
	body: slab({ x: BAR_FROM, y: BAR_Y, w: BAR_TO - BAR_FROM, d: BAR_D, r: 1.2 }, BAR_Z.down, BAR_H, 3, 0.4),
	sleeves: [post(BAR_FROM, BAR_Y + BAR_D / 2, 4.2, BAR_Z.down - 0.6, BAR_H + 1.2, 24), post(BAR_TO, BAR_Y + BAR_D / 2, 4.2, BAR_Z.down - 0.6, BAR_H + 1.2, 24)],
	hasp: slab({ x: (BAR_FROM + BAR_TO) / 2 - 5, y: BAR_Y + BAR_D, w: 10, d: 1.2, r: 0.4 }, BAR_Z.down - 3, BAR_H + 3, 3),
	haspHole: at([(BAR_FROM + BAR_TO) / 2, BAR_Y + BAR_D + 1.2, BAR_Z.down - 1]),
};

const LOCK_FACE = LOCK.y + LOCK.d;
const PAD: Plan = { x: LOCK.x + 12, y: LOCK.y + 12, w: 16, d: 9, r: 1.6 };
const LOCK_PX = LOCK.x + LOCK.w - 7;
const LOCK_PY = LOCK.y + 5;
export const LOCKBOX = {
	plinth: slab({ x: LOCK.x - 3, y: LOCK.y - 3, w: LOCK.w + 6, d: LOCK.d + 6, r: 3 }, 0, 3, 4, 0.5),
	body: slab(LOCK, 3, LOCK_TOP - 3, 6, 1),
	seam: sideSeam(LOCK, LOCK_TOP - 6, P, 6),
	slot: planOutline({ x: LOCK.x + 6, y: LOCK.y + 25, w: 28, d: 3, r: 1.4 }, LOCK_TOP, P, 3),
	screws: dots([[LOCK.x + 4, LOCK_FACE, LOCK_TOP - 3], [LOCK.x + LOCK.w - 4, LOCK_FACE, LOCK_TOP - 3], [LOCK.x + 4, LOCK_FACE, 7], [LOCK.x + LOCK.w - 4, LOCK_FACE, 7]]),
	staples: [slab({ x: PAD.x + 2, y: PAD.y + 3, w: 3, d: 3, r: 0.6 }, LOCK_TOP, 2, 3), slab({ x: PAD.x + PAD.w - 5, y: PAD.y + 3, w: 3, d: 3, r: 0.6 }, LOCK_TOP, 2, 3)],
	pad: slab(PAD, LOCK_TOP + 2, 13, 4, 0.8),
	keyhole: at([PAD.x + PAD.w / 2, PAD.y + PAD.d, LOCK_TOP + 7]),
	shackle: [
		post(PAD.x + 3.5, PAD.y + PAD.d / 2, 1.4, LOCK_TOP + 15, 7, 16),
		post(PAD.x + PAD.w - 3.5, PAD.y + PAD.d / 2, 1.4, LOCK_TOP + 15, 7, 16),
		slab({ x: PAD.x + 2.1, y: PAD.y + PAD.d / 2 - 1.4, w: PAD.w - 4.2, d: 2.8, r: 1.2 }, LOCK_TOP + 22, 2.4, 4),
	],
	boss: post(LOCK_PX, LOCK_PY, 3, LOCK_TOP, 1.4, 20),
	post: post(LOCK_PX, LOCK_PY, 1.3, LOCK_TOP + 1.4, POST_H - 1.4, 16),
	cap: post(LOCK_PX, LOCK_PY, 2, LOCK_TOP + POST_H, 1.2, 16),
	flag: slab({ x: LOCK_PX - 14.6, y: LOCK_PY - 0.9, w: 13, d: 1.8, r: 0.6 }, LOCK_TOP + FLAG_H + 7.4, FLAG_H, 3, 0.3),
	sleeve: post(LOCK_PX, LOCK_PY, 2.1, LOCK_TOP + FLAG_H + 7.4, FLAG_H, 16),
};

export const CONTROL = {
	plinth: slab({ x: CTRL.x - 3, y: CTRL.y - 3, w: CTRL.w + 6, d: CTRL.d + 6, r: 4 }, 0, 3, 6, 0.5),
	body: slab(CTRL, 3, CTRL_TOP - 3, 8, 1),
	seam: sideSeam(CTRL, CTRL_TOP - 6, P, 8),
	screws: dots([[CTRL.x + 5, CTRL_FACE, CTRL_TOP - 3.5], [CTRL.x + CTRL.w - 5, CTRL_FACE, CTRL_TOP - 3.5], [CTRL.x + 5, CTRL_FACE, 7], [CTRL.x + CTRL.w - 5, CTRL_FACE, 7]]),
	key: face(CTRL.x + 30, CTRL_FACE, 17),
	leverBoss: slab({ x: CTRL.x + 62, y: CTRL.y + 4, w: 16, d: 24, r: 1.6 }, CTRL_TOP, 2, 4, 0.4),
	leverSlot: planOutline({ x: CTRL.x + 67.6, y: CTRL.y + 7, w: 4.8, d: 18, r: 2.4 }, CTRL_TOP + 2, P, 6),
	leverTicks: [lineOnTop([CTRL.x + 64, CTRL.y + 10], [CTRL.x + 66.4, CTRL.y + 10], CTRL_TOP + 2, P), lineOnTop([CTRL.x + 64, CTRL.y + 22], [CTRL.x + 66.4, CTRL.y + 22], CTRL_TOP + 2, P)].join(""),
	lever: { stem: post(CTRL.x + 70, CTRL.y + 10, 1.2, CTRL_TOP + 2, 12, 16), knob: post(CTRL.x + 70, CTRL.y + 10, 2.8, CTRL_TOP + 14, 3.4, 20) },
	plate: face(CTRL.x + 58, CTRL_FACE, CTRL_TOP - 8),
};

const deg = (value: number) => (value * Math.PI) / 180;
export const KEY_ANGLES = [-52, 0, 52];
export const KEY_TICKS = KEY_ANGLES.map((angle) => pathOf([[Math.sin(deg(angle)) * 10.2, -Math.cos(deg(angle)) * 10.2], [Math.sin(deg(angle)) * 13, -Math.cos(deg(angle)) * 13]]));
const circle = (r: number) => pathOf(circleRing(0, 0, r, 48).map(([x, y]) => [x, y] as Flat), true);
export const KEY_BEZEL = circle(9.4);
export const KEY_PLUG = circle(6.4);
export const KEY_RIM = circle(7.6);
export const KEY_BOW = "M-1.3 -1.6H1.3V-5.4H3.6Q4.6 -5.4 4.6 -6.4V-12.6Q4.6 -13.6 3.6 -13.6H-3.6Q-4.6 -13.6 -4.6 -12.6V-6.4Q-4.6 -5.4 -3.6 -5.4H-1.3Z";
export const KEY_HOLE = circle(1.2);
export const KEY_BLADE = "M-0.9 -1.2H0.9V2.2H-0.9Z";
