import { fitProjection, iso, type Flat, type Plan, type Plane, type Point3 } from "@/components/iso/iso-kit";
import type { RouteName } from "./model";

export const WIDTH = 640;
export const HEIGHT = 350;
export const AZIMUTH = 58;

export const BASE: Plan = { x: -250, y: -150, w: 500, d: 262, r: 10 };
export const BASE_H = 10;
export const FOOT_H = 5;
export const CORK_Z = 0.8;
export const SLEEPER_H = 1.4;
export const RAIL_Z = CORK_Z + SLEEPER_H + 0.9;
export const GAUGE = 4.2;

export const LANE_Y: Record<RouteName, number> = { relay: -52, tail: 0, lan: 52 };
export const STUB_X = 152;
export const FAN_X = 100;
export const END_X = 206;
export const GAP: [number, number] = [-84, -64];

export const BOARD: Plan = { x: -96, y: -138, w: 192, d: 14, r: 2 };
export const BOARD_Z = 0;
export const BOARD_H = 46;
export const BOARD_FACE = BOARD.y + BOARD.d;

export const P = fitProjection(
	[
		[BASE.x, BASE.y, -BASE_H - FOOT_H],
		[BASE.x + BASE.w, BASE.y, -BASE_H - FOOT_H],
		[BASE.x, BASE.y + BASE.d, -BASE_H - FOOT_H],
		[BASE.x + BASE.w, BASE.y + BASE.d, -BASE_H - FOOT_H],
		[BASE.x, BASE.y, 50],
		[BASE.x + BASE.w, BASE.y, 50],
		[BOARD.x, BOARD.y, BOARD_H + 2],
		[BOARD.x + BOARD.w, BOARD.y, BOARD_H + 2],
	],
	WIDTH,
	HEIGHT,
	{ pad: 18, azimuth: AZIMUTH },
);

export const at = (point: Point3): Flat => iso(point, P);

function corner(from: Plane, via: Plane, to: Plane, radius: number, out: Plane[]) {
	const inLength = Math.hypot(via[0] - from[0], via[1] - from[1]);
	const outLength = Math.hypot(to[0] - via[0], to[1] - via[1]);
	const r = Math.min(radius, inLength / 2, outLength / 2);
	const a: Plane = [via[0] + ((from[0] - via[0]) / inLength) * r, via[1] + ((from[1] - via[1]) / inLength) * r];
	const b: Plane = [via[0] + ((to[0] - via[0]) / outLength) * r, via[1] + ((to[1] - via[1]) / outLength) * r];
	for (let index = 0; index <= 10; index++) {
		const t = index / 10;
		const u = 1 - t;
		out.push([u * u * a[0] + 2 * u * t * via[0] + t * t * b[0], u * u * a[1] + 2 * u * t * via[1] + t * t * b[1]]);
	}
}

function densify(points: Plane[], step = 1): Plane[] {
	const out: Plane[] = [points[0]!];
	for (let index = 1; index < points.length; index++) {
		const [ax, ay] = points[index - 1]!;
		const [bx, by] = points[index]!;
		const length = Math.hypot(bx - ax, by - ay);
		const count = Math.max(1, Math.ceil(length / step));
		for (let k = 1; k <= count; k++) out.push([ax + ((bx - ax) * k) / count, ay + ((by - ay) * k) / count]);
	}
	return out;
}

export interface Path {
	points: Plane[];
	lengths: number[];
	total: number;
}

function pathOfPoints(points: Plane[]): Path {
	const lengths = [0];
	for (let index = 1; index < points.length; index++) {
		const [ax, ay] = points[index - 1]!;
		const [bx, by] = points[index]!;
		lengths.push(lengths[index - 1]! + Math.hypot(bx - ax, by - ay));
	}
	return { points, lengths, total: lengths[lengths.length - 1]! };
}

function laneOf(y: number): Path {
	const corners: Plane[] = [[-END_X, 0], [-STUB_X, 0], [-FAN_X, y], [FAN_X, y], [STUB_X, 0], [END_X, 0]];
	if (y === 0) return pathOfPoints(densify([corners[0]!, corners[5]!]));
	const out: Plane[] = [corners[0]!];
	for (let index = 1; index < corners.length - 1; index++) corner(corners[index - 1]!, corners[index]!, corners[index + 1]!, 34, out);
	out.push(corners[5]!);
	return pathOfPoints(densify(out));
}

export const LANES: Record<RouteName, Path> = { lan: laneOf(LANE_Y.lan), tail: laneOf(0), relay: laneOf(LANE_Y.relay) };

export function sampleAt(path: Path, s: number): { point: Plane; angle: number } {
	const distance = Math.max(0, Math.min(path.total, s));
	let index = 1;
	while (index < path.lengths.length - 1 && path.lengths[index]! < distance) index++;
	const a = path.points[index - 1]!;
	const b = path.points[index]!;
	const span = path.lengths[index]! - path.lengths[index - 1]! || 1;
	const t = (distance - path.lengths[index - 1]!) / span;
	return { point: [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], angle: Math.atan2(b[1] - a[1], b[0] - a[0]) };
}

export function distanceAtX(path: Path, x: number) {
	for (let index = 1; index < path.points.length; index++) if (path.points[index]![0] >= x) return path.lengths[index]!;
	return path.total;
}

export function offset(path: Path, by: number, from = 0, to = Infinity): Plane[] {
	const out: Plane[] = [];
	for (let index = 0; index < path.points.length; index++) {
		const s = path.lengths[index]!;
		if (s < from - 0.01 || s > to + 0.01) continue;
		const a = path.points[Math.max(0, index - 1)]!;
		const b = path.points[Math.min(path.points.length - 1, index + 1)]!;
		const length = Math.hypot(b[0] - a[0], b[1] - a[1]) || 1;
		const nx = -(b[1] - a[1]) / length;
		const ny = (b[0] - a[0]) / length;
		const [x, y] = path.points[index]!;
		out.push([x + nx * by, y + ny * by]);
	}
	return out;
}
