import {
	cylinder as kitCylinder,
	corners,
	dotGrid,
	extrude,
	insetPlan,
	onTop,
	pathOf,
	planOutline,
	ring as kitRing,
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
import { ROUTES, type RouteName } from "./model";
import {
	AZIMUTH,
	BASE,
	BASE_H,
	BOARD,
	BOARD_FACE,
	BOARD_H,
	CORK_Z,
	END_X,
	FOOT_H,
	GAP,
	GAUGE,
	LANES,
	LANE_Y,
	P,
	RAIL_Z,
	SLEEPER_H,
	at,
	distanceAtX,
	offset,
	sampleAt,
} from "./view";

const solid = (plan: Plan, z: number, h: number, steps = 4, bevel = 0) => slabOf(plan, z, h, P, steps, bevel);
const post = (x: number, y: number, r: number, z: number, h: number, steps = 18) => kitCylinder(x, y, r, z, h, P, steps);
const segment = (a: Point3, b: Point3) => pathOf([at(a), at(b)]);
const join = (paths: string[]) => paths.join("");
const ringOn = (x: number, y: number, r: number, z: number, steps = 16) => kitRing(x, y, r, z, P, steps);
export const depthKey = ([x, y]: Plane) => x * Math.cos((AZIMUTH * Math.PI) / 180) + y * Math.sin((AZIMUTH * Math.PI) / 180);

export function boxRing(cx: number, cy: number, length: number, width: number, angle: number): Plane[] {
	const c = Math.cos(angle);
	const s = Math.sin(angle);
	const hl = length / 2;
	const hw = width / 2;
	return [
		[cx - c * hl + s * hw, cy - s * hl - c * hw],
		[cx + c * hl + s * hw, cy + s * hl - c * hw],
		[cx + c * hl - s * hw, cy + s * hl + c * hw],
		[cx - c * hl - s * hw, cy - s * hl + c * hw],
	];
}

const faceRect = (y: number, x0: number, x1: number, z0: number, z1: number) =>
	pathOf([at([x0, y, z0]), at([x1, y, z0]), at([x1, y, z1]), at([x0, y, z1])], true);
const sideRect = (x: number, y0: number, y1: number, z0: number, z1: number) =>
	pathOf([at([x, y0, z0]), at([x, y1, z0]), at([x, y1, z1]), at([x, y0, z1])], true);

const FEET = corners(BASE, 26);
const SCREWS = corners(BASE, 9);
const PLATE: Plan = { x: BASE.x + 18, y: BASE.y + BASE.d - 26, w: 58, d: 12, r: 2 };
const STRIP: Plan = { x: -150, y: BASE.y + BASE.d - 18, w: 300, d: 7, r: 1 };
const STRIP_TICKS = topTicks(STRIP.x + 4, STRIP.x + STRIP.w - 4, 5, 5, STRIP.y + 0.4, 0.6, [2, 3.8], P);
export const CORK: Plan = { x: -226, y: -76, w: 452, d: 154, r: 8 };

export const BASE_ART = {
	halo: solid(BASE, -BASE_H - FOOT_H, BASE_H).fill,
	feet: FEET.map(([x, y]) => [post(x, y, 8, -BASE_H - FOOT_H, 1.6, 28), post(x, y, 6, -BASE_H - FOOT_H + 1.6, FOOT_H - 1.6, 28)]),
	slab: solid(BASE, -BASE_H, BASE_H, 8, 1.6),
	groove: planOutline(insetPlan(BASE, 4.5), 0, P),
	screws: SCREWS.map(([x, y]) => at([x, y, 0])),
	screwRings: join(SCREWS.map(([x, y]) => ringOn(x, y, 2.2, 0))),
	cork: solid(CORK, 0, CORK_Z, 6),
	corkEdge: planOutline(insetPlan(CORK, 2.5), CORK_Z, P, 6),
	plate: solid(PLATE, 0, 0.8, 3),
	plateLine: planOutline(insetPlan(PLATE, 1.6), 0.8, P, 3),
	plateRule: join([0, 1, 2].map((row) => onTop([[PLATE.x + 7, PLATE.y + 3.5 + row * 2.4], [PLATE.x + (row === 1 ? 30 : 44), PLATE.y + 3.5 + row * 2.4]], 0.8, P))),
	plateScrews: [at([PLATE.x + 3.4, PLATE.y + PLATE.d / 2, 0.8]), at([PLATE.x + PLATE.w - 3.4, PLATE.y + PLATE.d / 2, 0.8])],
	strip: solid(STRIP, 0, 0.6, 3),
	stripMinor: STRIP_TICKS.minor,
	stripMajor: STRIP_TICKS.major,
};

function sleepersOf(route: RouteName, from: number, to: number) {
	const lane = LANES[route];
	const out: { paths: SolidPaths; key: number }[] = [];
	for (let s = from; s <= to; s += 8) {
		const { point, angle } = sampleAt(lane, s);
		const [x, y] = point;
		if (x > GAP[0] - 3 && x < GAP[1] + 3) continue;
		if (route !== "tail" && Math.abs(y) < 13) continue;
		out.push({ paths: extrude(boxRing(x, y, 3.2, 15, angle), CORK_Z, SLEEPER_H, P), key: depthKey(point) });
	}
	return out;
}

const SLEEPERS = ROUTES.flatMap((route) => sleepersOf(route, 3, LANES[route].total - 3)).sort((a, b) => a.key - b.key);

function railsOf(route: RouteName) {
	const lane = LANES[route];
	const bent = lane.points.map(([, y], index) => (Math.abs(y) > 0.01 ? index : -1)).filter((index) => index >= 0);
	const start = route === "tail" ? 0 : lane.lengths[Math.max(0, bent[0]! - 1)]! - 4;
	const end = route === "tail" ? lane.total : lane.lengths[Math.min(lane.points.length - 1, bent[bent.length - 1]! + 1)]! + 4;
	const gapFrom = distanceAtX(lane, GAP[0]);
	const gapTo = distanceAtX(lane, GAP[1]);
	const spans: [number, number][] = [
		[start, gapFrom],
		[gapTo, end],
	];
	const lines = spans.flatMap(([a, b]) => [-GAUGE, GAUGE].map((side) => offset(lane, side, a, b)));
	const ends = lines.flatMap((points, index) => {
		const first = points[0]!;
		const last = points[points.length - 1]!;
		const picks = index < 2 ? (route === "tail" ? [first, last] : [last]) : route === "tail" ? [first, last] : [first];
		return picks.map(([x, y]) => at([x, y, RAIL_Z]));
	});
	return { d: join(lines.map((points) => onTop(points, RAIL_Z, P))), ends };
}

export const TRACK_ART = {
	sleepers: SLEEPERS.map(({ paths }) => paths),
	rails: Object.fromEntries(ROUTES.map((route) => [route, railsOf(route)])) as Record<RouteName, ReturnType<typeof railsOf>>,
	lanePaths: Object.fromEntries(ROUTES.map((route) => [route, pathOf(LANES[route].points.map(([x, y]) => at([x, y, RAIL_Z])))])) as Record<RouteName, string>,
	pits: ROUTES.map((route) => planOutline({ x: GAP[0], y: LANE_Y[route] - 9, w: GAP[1] - GAP[0], d: 18, r: 1.5 }, CORK_Z, P, 3)),
};

export function deckOf(route: RouteName) {
	const y = LANE_Y[route];
	const plan: Plan = { x: GAP[0] + 0.6, y: y - 8.4, w: GAP[1] - GAP[0] - 1.2, d: 16.8, r: 1.2 };
	return {
		plate: solid(plan, CORK_Z, 1.2, 3, 0.4),
		sleepers: [GAP[0] + 5, GAP[1] - 5].map((x) => solid({ x: x - 1.6, y: y - 7.5, w: 3.2, d: 15, r: 0.4 }, CORK_Z + 1.2, SLEEPER_H - 1.2 + 0.6, 2)),
		rails: join([-GAUGE, GAUGE].map((side) => onTop([[GAP[0] + 0.6, y + side], [GAP[1] - 0.6, y + side]], RAIL_Z, P))),
		bolts: [GAP[0] + 5, GAP[1] - 5].flatMap((x) => [at([x, y - 6, CORK_Z + SLEEPER_H + 0.6]), at([x, y + 6, CORK_Z + SLEEPER_H + 0.6])]),
		handle: solid({ x: GAP[0] + 7.5, y: y + 8.4, w: 5, d: 1.6, r: 0.5 }, CORK_Z + 0.4, 2.2, 2),
	};
}
export const DECKS = Object.fromEntries(ROUTES.map((route) => [route, deckOf(route)])) as Record<RouteName, ReturnType<typeof deckOf>>;

export function bufferOf(x: number, side: 1 | -1) {
	const block: Plan = { x: x - (side > 0 ? 0 : 6), y: -9, w: 6, d: 18, r: 1 };
	const beamX = side > 0 ? x - 1.6 : x + 0.2;
	return {
		block: solid(block, CORK_Z, 7, 3, 0.5),
		beam: solid({ x: beamX, y: -10.5, w: 1.4, d: 21, r: 0.4 }, CORK_Z + 4, 3.4, 2),
		pads: [-6, 6].map((y) => post(beamX + (side > 0 ? -0.8 : 1.4 + 0.8), y, 1.6, CORK_Z + 4.6, 2.2, 14)),
		bolts: [at([block.x + 3, -6.5, CORK_Z + 7]), at([block.x + 3, 6.5, CORK_Z + 7])],
	};
}

const PHONE_PLATFORM: Plan = { x: -248, y: -30, w: 32, d: 60, r: 3 };
const COMPUTER_PLATFORM: Plan = { x: 216, y: -30, w: 32, d: 60, r: 3 };
const PLATFORM_H = 6;

const PHONE: Plan = { x: -240, y: -3.4, w: 16, d: 3.4, r: 1.4 };
const PHONE_Z = PLATFORM_H + 3;
const PHONE_H = 30;
const DOCK: Plan = { x: -243, y: -8, w: 22, d: 12, r: 2 };
const PHONE_FACE = PHONE.y + PHONE.d;

export const PHONE_ART = {
	platform: solid(PHONE_PLATFORM, 0, PLATFORM_H, 4, 0.8),
	platformEdge: planOutline(insetPlan(PHONE_PLATFORM, 2.5), PLATFORM_H, P, 4),
	platformScrews: corners(PHONE_PLATFORM, 4).map(([x, y]) => at([x, y, PLATFORM_H])),
	dock: solid(DOCK, PLATFORM_H, 3, 4, 0.6),
	slot: planOutline({ x: PHONE.x - 0.6, y: PHONE.y - 0.6, w: PHONE.w + 1.2, d: PHONE.d + 1.2, r: 1.6 }, PLATFORM_H + 3, P, 3),
	phone: solid(PHONE, PHONE_Z, PHONE_H, 4, 0.5),
	screen: faceRect(PHONE_FACE, PHONE.x + 1.6, PHONE.x + PHONE.w - 1.6, PHONE_Z + 3.4, PHONE_Z + PHONE_H - 2.6),
	camera: at([PHONE.x + PHONE.w / 2, PHONE_FACE, PHONE_Z + PHONE_H - 1.3]),
	button: segment([PHONE.x + PHONE.w, PHONE.y + 1.2, PHONE_Z + 18], [PHONE.x + PHONE.w, PHONE.y + 1.2, PHONE_Z + 22]),
	rows: join([0, 1, 2, 3].map((row) => segment([PHONE.x + 3.2, PHONE_FACE, PHONE_Z + PHONE_H - 7 - row * 4.2], [PHONE.x + (row % 2 ? 9 : 12.6), PHONE_FACE, PHONE_Z + PHONE_H - 7 - row * 4.2]))),
	buffer: bufferOf(-END_X - 2, 1),
};

const BASE_LAP: Plan = { x: 219, y: -10, w: 26, d: 20, r: 1.6 };
const LAP_Z = PLATFORM_H;
const LID: Plan = { x: 219, y: -12.4, w: 26, d: 2.4, r: 0.8 };
const LID_H = 22;
export const LID_FACE = LID.y + LID.d;

export const COMPUTER_ART = {
	platform: solid(COMPUTER_PLATFORM, 0, PLATFORM_H, 4, 0.8),
	platformEdge: planOutline(insetPlan(COMPUTER_PLATFORM, 2.5), PLATFORM_H, P, 4),
	platformScrews: corners(COMPUTER_PLATFORM, 4).map(([x, y]) => at([x, y, PLATFORM_H])),
	base: solid(BASE_LAP, LAP_Z, 2.2, 4, 0.5),
	keys: dotGrid(BASE_LAP.x + 3, BASE_LAP.y + 2.6, 11, 4, 2, LAP_Z + 2.2, P),
	pad: planOutline({ x: BASE_LAP.x + 9, y: BASE_LAP.y + 12, w: 8, d: 5.4, r: 0.8 }, LAP_Z + 2.2, P, 3),
	lid: solid(LID, LAP_Z, LID_H + 2.2, 3, 0.4),
	screen: faceRect(LID_FACE, LID.x + 1.8, LID.x + LID.w - 1.8, LAP_Z + 4.6, LAP_Z + LID_H),
	hinge: segment([LID.x + 2, LID_FACE, LAP_Z + 2.2], [LID.x + LID.w - 2, LID_FACE, LAP_Z + 2.2]),
	led: at([BASE_LAP.x + BASE_LAP.w - 3, BASE_LAP.y + BASE_LAP.d, LAP_Z + 1.1]),
	buffer: bufferOf(END_X + 2, -1),
};
export const SCREEN_GLOW = faceRect(LID_FACE, LID.x + 3.2, LID.x + LID.w - 3.2, LAP_Z + 6, LAP_Z + LID_H - 1.4);

const TUNNEL: Plan = { x: -30, y: -15, w: 60, d: 30, r: 3 };
const TUNNEL_H = 20;
const PORTAL_X = TUNNEL.x + TUNNEL.w;
function arch(x: number, half: number, z0: number, spring: number) {
	const points: Flat[] = [at([x, -half, z0])];
	for (let step = 0; step <= 16; step++) {
		const angle = Math.PI - (step / 16) * Math.PI;
		points.push(at([x, Math.cos(angle) * half, spring + Math.sin(angle) * half]));
	}
	points.push(at([x, half, z0]));
	return pathOf(points);
}
export const TUNNEL_ART = {
	body: solid(TUNNEL, CORK_Z, TUNNEL_H, 6, 1.2),
	seams: join([-16, -2, 12].map((x) => join([segment([x, TUNNEL.y + TUNNEL.d, CORK_Z], [x, TUNNEL.y + TUNNEL.d, CORK_Z + TUNNEL_H]), onTop([[x, TUNNEL.y + TUNNEL.d], [x, TUNNEL.y]], CORK_Z + TUNNEL_H, P)])) ),
	portal: solid({ x: PORTAL_X, y: -18, w: 3, d: 36, r: 0.8 }, CORK_Z, TUNNEL_H + 3, 3, 0.6),
	coping: solid({ x: PORTAL_X - 0.6, y: -19, w: 4.2, d: 38, r: 0.8 }, CORK_Z + TUNNEL_H + 3, 1.6, 3, 0.4),
	arch: arch(PORTAL_X + 3, 10, CORK_Z, CORK_Z + 6),
	archInner: arch(PORTAL_X + 3, 8.4, CORK_Z, CORK_Z + 6),
	vent: post(10, 0, 2.4, CORK_Z + TUNNEL_H, 3, 18),
	ventRing: ringOn(10, 0, 1.4, CORK_Z + TUNNEL_H + 3, 14),
};
export const TUNNEL_KEY = depthKey([0, 15]);

const HOUSE: Plan = { x: -26, y: LANE_Y.relay - 15, w: 52, d: 30, r: 1.5 };
const HOUSE_H = 22;
const ROOF: Plan = insetPlan(HOUSE, -3.2);
const DOOR_X = HOUSE.x + HOUSE.w;
const HOUSE_FACE = HOUSE.y + HOUSE.d;
export const HOUSE_ART = {
	body: solid(HOUSE, CORK_Z, HOUSE_H, 3, 0.6),
	plinth: sideSeam(HOUSE, CORK_Z + 3, P, 3),
	door: sideRect(DOOR_X, LANE_Y.relay - 9, LANE_Y.relay + 9, CORK_Z, CORK_Z + 16),
	doorInner: sideRect(DOOR_X, LANE_Y.relay - 7.6, LANE_Y.relay + 7.6, CORK_Z, CORK_Z + 14.6),
	windows: join([-16, 0, 16].map((x) => faceRect(HOUSE_FACE, x - 4.5, x + 4.5, CORK_Z + 9, CORK_Z + 16))),
	sills: join([-16, 0, 16].map((x) => segment([x - 4.5, HOUSE_FACE, CORK_Z + 12.5], [x + 4.5, HOUSE_FACE, CORK_Z + 12.5]))),
	roof: solid(ROOF, CORK_Z + HOUSE_H, 2.6, 3, 0.8),
	ridge: solid({ x: ROOF.x + 4, y: LANE_Y.relay - 4, w: ROOF.w - 8, d: 8, r: 1 }, CORK_Z + HOUSE_H + 2.6, 2.4, 3, 0.5),
	mast: post(16, LANE_Y.relay, 0.9, CORK_Z + HOUSE_H + 5, 14, 12),
	mastBase: post(16, LANE_Y.relay, 2.2, CORK_Z + HOUSE_H + 5, 1.4, 16),
	mastTip: at([16, LANE_Y.relay, CORK_Z + HOUSE_H + 19]),
};
export const HOUSE_KEY = depthKey([0, LANE_Y.relay + 15]);

export const SIGNAL_X = -118;
export function signalOf(route: RouteName) {
	const y = LANE_Y[route] + 14;
	const head: Plan = { x: SIGNAL_X - 2.4, y: y - 1.6, w: 4.8, d: 3.2, r: 1 };
	return {
		pad: solid({ x: SIGNAL_X - 3.5, y: y - 3.5, w: 7, d: 7, r: 1 }, CORK_Z, 1.6, 3, 0.4),
		post: post(SIGNAL_X, y, 1.1, CORK_Z + 1.6, 24, 14),
		head: solid(head, CORK_Z + 24, 11, 3, 0.4),
		hood: solid({ x: SIGNAL_X - 2.8, y: y + 1.6, w: 5.6, d: 1.4, r: 0.4 }, CORK_Z + 32, 1.2, 2),
		lamps: [at([SIGNAL_X, y + 1.6, CORK_Z + 30.6]), at([SIGNAL_X, y + 1.6, CORK_Z + 26.8])],
		rings: join([CORK_Z + 30.6, CORK_Z + 26.8].map((z) => sideRing([SIGNAL_X, y + 1.6, z], 1.5, "y", P, 14))),
		key: depthKey([SIGNAL_X, y]),
	};
}
export const SIGNALS = Object.fromEntries(ROUTES.map((route) => [route, signalOf(route)])) as Record<RouteName, ReturnType<typeof signalOf>>;

export function frameOf(x: number) {
	const y = 20;
	const base: Plan = { x: x - 9, y: y - 3.5, w: 18, d: 7, r: 1 };
	const levers = ROUTES.map((route, index) => {
		const lx = x - 5 + index * 5;
		return {
			route,
			pulled: solid({ x: lx - 0.7, y: y - 0.7, w: 1.4, d: 1.4, r: 0.3 }, CORK_Z + 3, 11, 2),
			normal: solid({ x: lx - 0.7, y: y - 0.7, w: 1.4, d: 1.4, r: 0.3 }, CORK_Z + 3, 5.5, 2),
			pulledGrip: at([lx, y, CORK_Z + 14]),
			normalGrip: at([lx, y, CORK_Z + 8.5]),
		};
	});
	return {
		base: solid(base, CORK_Z, 3, 3, 0.5),
		slot: planOutline(insetPlan(base, 1.6), CORK_Z + 3, P, 2),
		bolts: [at([base.x + 1.6, y, CORK_Z + 3]), at([base.x + base.w - 1.6, y, CORK_Z + 3])],
		levers,
		key: depthKey([x, y + 4]),
	};
}
export const FRAMES = [frameOf(-178), frameOf(178)];

const SILL: Plan = { x: BOARD.x - 4, y: BOARD_FACE - 0.2, w: BOARD.w + 8, d: 10, r: 1.5 };
export const DIAL_X: Record<RouteName, number> = { relay: -56, tail: 0, lan: 56 };
export const DIAL_Z = 30;
export const DIAL_R = 11;
function dialTicks(cx: number) {
	const minor: string[] = [];
	const major: string[] = [];
	for (let step = 0; step <= 20; step++) {
		const angle = Math.PI * (1.1 - (step / 20) * 1.2);
		const big = step % 5 === 0;
		const outer = DIAL_R - 2;
		const inner = outer - (big ? 2.4 : 1.3);
		const point = (r: number): Point3 => [cx + Math.cos(angle) * r, BOARD_FACE, DIAL_Z + Math.sin(angle) * r];
		(big ? major : minor).push(segment(point(inner), point(outer)));
	}
	return { minor: join(minor), major: join(major) };
}
export const BOARD_ART = {
	cabinet: solid(BOARD, 0, BOARD_H, 4, 1),
	topSeam: sideSeam(BOARD, BOARD_H - 4, P, 4),
	sill: solid(SILL, 0, 7, 4, 0.8),
	sillEdge: planOutline(insetPlan(SILL, 1.5), 7, P, 4),
	dials: ROUTES.map((route) => ({
		route,
		bezel: sideRing([DIAL_X[route], BOARD_FACE, DIAL_Z], DIAL_R, "y", P, 40),
		face: sideRing([DIAL_X[route], BOARD_FACE, DIAL_Z], DIAL_R - 1.4, "y", P, 40),
		...dialTicks(DIAL_X[route]),
		pivot: at([DIAL_X[route], BOARD_FACE, DIAL_Z]),
		lamp: at([DIAL_X[route], BOARD_FACE, 13]),
		lampRing: sideRing([DIAL_X[route], BOARD_FACE, 13], 2, "y", P, 16),
		toggle: post(DIAL_X[route], SILL.y + 5, 1.8, 7, 2.4, 16),
		toggleCap: ringOn(DIAL_X[route], SILL.y + 5, 1, 9.4, 12),
	})),
	screws: [BOARD.x + 5, BOARD.x + BOARD.w - 5].flatMap((x) => [at([x, BOARD_FACE, BOARD_H - 7]), at([x, BOARD_FACE, 11])]),
};

export function needleOf(route: RouteName, fraction: number) {
	const angle = Math.PI * (1.1 - Math.max(0, Math.min(1, fraction)) * 1.2);
	const cx = DIAL_X[route];
	const tip: Point3 = [cx + Math.cos(angle) * (DIAL_R - 2.6), BOARD_FACE, DIAL_Z + Math.sin(angle) * (DIAL_R - 2.6)];
	const tail: Point3 = [cx - Math.cos(angle) * 2, BOARD_FACE, DIAL_Z - Math.sin(angle) * 2];
	return segment(tail, tip);
}

export const DECK_LIFT = 16;

export const TRAIN_LENGTH = 20;
export function trainOf(x: number, y: number, angle: number) {
	return {
		body: extrude(boxRing(x, y, TRAIN_LENGTH, 9, angle), RAIL_Z, 6.5, P),
		roof: extrude(boxRing(x, y, TRAIN_LENGTH - 3, 7, angle), RAIL_Z + 6.5, 1.6, P),
		lamp: at([x, y, RAIL_Z + 8.1]),
	};
}
