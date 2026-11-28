"use client";

import * as React from "react";
import type { Flat } from "@/components/iso/iso-kit";
import { asksForStillness, follow, useInView, useLoop, useReadout } from "@/components/iso/live";
import { KEY_ANGLES, MODULE_IDS } from "./geometry";
import { NOTCHES, REST, benchText, moduleState, type Bench, type Flag } from "./hero";

const TOUR: Bench[] = [
	{ notch: 0, plan: false },
	{ notch: 1, plan: false },
	{ notch: 2, plan: false },
	{ notch: 2, plan: true },
];
const TOUR_SECONDS = 2.8;
const EASE = 0.22;

interface Motion {
	key: number;
	bar: number;
	lever: number;
	flags: number[];
}

export function BenchLive({ ez, ey, lift, travel, rises, children }: { ez: Flat; ey: Flat; lift: number; travel: number; rises: Record<Flag, number>; children: React.ReactNode }) {
	const { anchor, say } = useReadout<HTMLDivElement>();
	const inView = useInView(anchor);
	const [bench, setBench] = React.useState<Bench>(REST);
	const target = React.useRef<Bench>(REST);
	const touched = React.useRef(false);
	const clock = React.useRef(0);
	const step = React.useRef(0);
	const motion = React.useRef<Motion>({ key: KEY_ANGLES[0], bar: lift, lever: 0, flags: MODULE_IDS.map((id) => rises[moduleState(id, REST).flag]) });

	const goals = React.useCallback(
		(next: Bench): Motion => ({ key: KEY_ANGLES[next.notch], bar: next.plan ? 0 : lift, lever: next.plan ? travel : 0, flags: MODULE_IDS.map((id) => rises[moduleState(id, next).flag]) }),
		[lift, rises, travel],
	);

	const paint = React.useCallback(() => {
		const root = anchor.current;
		if (!root) return;
		const now = motion.current;
		const move = (vector: Flat, by: number) => "translate(" + (vector[0] * by).toFixed(2) + " " + (vector[1] * by).toFixed(2) + ")";
		root.querySelector("[data-key]")?.setAttribute("transform", "rotate(" + now.key.toFixed(2) + ")");
		root.querySelector("[data-bar]")?.setAttribute("transform", move(ez, now.bar));
		root.querySelector("[data-lever]")?.setAttribute("transform", move(ey, now.lever));
		MODULE_IDS.forEach((id, index) => {
			root.querySelector('[data-flag="' + id + '"]')?.setAttribute("transform", move(ez, now.flags[index]));
			root.querySelector('[data-lamp="' + id + '"] > .iso-solid')?.toggleAttribute("data-lit", moduleState(id, target.current).lit);
		});
		say(benchText(target.current).readout);
	}, [anchor, ey, ez, say]);

	const tick = React.useCallback(
		(dt: number, calm: boolean) => {
			if (!touched.current && !calm) {
				clock.current += dt;
				if (clock.current > TOUR_SECONDS) {
					clock.current = 0;
					step.current = (step.current + 1) % TOUR.length;
					target.current = TOUR[step.current];
					setBench(target.current);
				}
			}
			const goal = goals(target.current);
			const now = motion.current;
			now.key = follow(now.key, goal.key, dt, EASE, calm);
			now.bar = follow(now.bar, goal.bar, dt, EASE * 1.4, calm);
			now.lever = follow(now.lever, goal.lever, dt, EASE, calm);
			now.flags = now.flags.map((value, index) => follow(value, goal.flags[index], dt, EASE * 1.2, calm));
			paint();
			if (calm) return false;
			const settled = now.key === goal.key && now.bar === goal.bar && now.lever === goal.lever && now.flags.every((value, index) => value === goal.flags[index]);
			return !touched.current || !settled;
		},
		[goals, paint],
	);

	const kick = useLoop(tick, inView);

	const set = (next: Bench) => {
		touched.current = true;
		target.current = next;
		setBench(next);
		if (asksForStillness()) {
			motion.current = goals(next);
			paint();
		}
		kick();
	};

	const onKeyDown = (event: React.KeyboardEvent) => {
		const delta = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1 }[event.key];
		const current = target.current;
		if (delta) {
			event.preventDefault();
			set({ ...current, notch: Math.max(0, Math.min(NOTCHES.length - 1, current.notch + delta)) });
		} else if (event.key === "p" || event.key === "P" || event.key === " " || event.key === "Enter") {
			event.preventDefault();
			set({ ...current, plan: !current.plan });
		} else if (event.key === "Home" || event.key === "End") {
			event.preventDefault();
			set({ ...current, notch: event.key === "Home" ? 0 : NOTCHES.length - 1 });
		}
	};

	const onClick = (event: React.MouseEvent) => {
		const current = target.current;
		if ((event.target as Element).closest("[data-lever], [data-bar]")) set({ ...current, plan: !current.plan });
		else set({ ...current, notch: (current.notch + 1) % NOTCHES.length });
	};

	return (
		<div ref={anchor} className="ap-bench" role="slider" tabIndex={0} aria-label="Access level key" aria-valuemin={0} aria-valuemax={NOTCHES.length - 1} aria-valuenow={bench.notch} aria-valuetext={benchText(bench).valuetext} onKeyDown={onKeyDown} onClick={onClick}>
			{children}
		</div>
	);
}
