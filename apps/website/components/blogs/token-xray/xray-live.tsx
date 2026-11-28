"use client";

import * as React from "react";
import { translateAlong } from "@/components/iso/iso-kit";
import { asksForStillness, follow, useInView, useLoop, useReadout } from "@/components/iso/live";
import { ALL_OFF, ALL_ON, BINS, FIXES, cents, session, type FixId, type Fixes } from "./session";
import { FILL_MOST, KNOB_TRAVEL, P, fillPaths } from "./xray-geometry";

const STEP_SECONDS = 2.2;
const HOLD_SECONDS = 4.5;
const FILL_SECONDS = 0.55;
const KNOB_SECONDS = 0.18;

const readoutOf = (fixes: Fixes) => {
	const view = session(fixes);
	return { view, text: `${cents(view.cost)} a task · ${view.requests} requests` };
};

export function XrayLive({ most, children }: { most: number; children: React.ReactNode }) {
	const { anchor, say } = useReadout<HTMLDivElement>();
	const inView = useInView(anchor);
	const [fixes, setFixes] = React.useState<Fixes>(ALL_ON);
	const start = session(ALL_ON);
	const motion = React.useRef({
		fixes: ALL_ON,
		heights: start.rows.map((row) => (row.cost / most) * FILL_MOST),
		costs: start.rows.map((row) => row.cost),
		knobs: FIXES.map(() => 0),
		tokens: start.tokens,
		touched: false,
		clock: 0,
		phase: -1,
	});

	const paint = React.useCallback(() => {
		const root = anchor.current;
		if (!root) return;
		const current = motion.current;
		current.heights.forEach((height, index) => {
			const solid = root.querySelector(`[data-fill="${index}"] > .iso-solid`);
			if (!solid) return;
			const paths = fillPaths(index, height);
			solid.querySelector(".iso-fill")?.setAttribute("d", paths.fill);
			solid.querySelector(".iso-top")?.setAttribute("d", paths.top);
			solid.querySelector(".iso-crease")?.setAttribute("d", paths.crease);
			solid.querySelector(".iso-edge")?.setAttribute("d", paths.outline);
			solid.querySelectorAll<SVGPathElement>(".iso-shade").forEach((node) => {
				const shade = Number(node.dataset.shade);
				node.setAttribute("d", paths.shades[shade] ?? "");
			});
		});
		current.knobs.forEach((offset, index) => {
			root.querySelector(`[data-knob="${index}"]`)?.setAttribute("transform", translateAlong(P, [0, KNOB_TRAVEL * offset, 0]));
			root.querySelector(`[data-lamp="${index}"] > .iso-solid`)?.toggleAttribute("data-lit", current.fixes[FIXES[index].id]);
		});
		const total = current.costs.reduce((sum, cost) => sum + cost, 0);
		const slots: Record<string, string> = {
			total: cents(total),
			tokens: `${(current.tokens / 1000).toFixed(1)}k`,
			requests: String(session(current.fixes).requests),
		};
		for (const [slot, text] of Object.entries(slots)) {
			const node = root.querySelector(`[data-slot="${slot}"]`);
			if (node && node.textContent !== text) node.textContent = text;
		}
		current.costs.forEach((cost, index) => {
			const node = root.querySelector(`[data-bin-cost="${index}"]`);
			const text = cents(cost);
			if (node && node.textContent !== text) node.textContent = text;
		});
		say(readoutOf(current.fixes).text);
	}, [anchor, say]);

	const apply = React.useCallback((next: Fixes) => {
		motion.current.fixes = next;
		setFixes(next);
	}, []);

	const tick = React.useCallback(
		(dt: number, calm: boolean) => {
			const current = motion.current;
			if (!current.touched && !calm) {
				current.clock += dt;
				const length = FIXES.length * STEP_SECONDS + HOLD_SECONDS;
				if (current.phase < 0 || current.clock > length) {
					current.clock = 0;
					current.phase = 0;
					apply(ALL_OFF);
				}
				const on = Math.min(FIXES.length, Math.floor(current.clock / STEP_SECONDS));
				if (on !== current.phase) {
					current.phase = on;
					apply(Object.fromEntries(FIXES.map((fix, index) => [fix.id, index < on])) as Fixes);
				}
			}
			const target = session(current.fixes);
			let moving = false;
			target.rows.forEach((row, index) => {
				const height = (row.cost / most) * FILL_MOST;
				current.heights[index] = follow(current.heights[index], height, dt, FILL_SECONDS, calm);
				current.costs[index] = follow(current.costs[index], row.cost, dt, FILL_SECONDS, calm);
				moving ||= current.heights[index] !== height || current.costs[index] !== row.cost;
			});
			current.tokens = follow(current.tokens, target.tokens, dt, FILL_SECONDS, calm);
			moving ||= current.tokens !== target.tokens;
			FIXES.forEach((fix, index) => {
				const goal = current.fixes[fix.id] ? 0 : 1;
				current.knobs[index] = follow(current.knobs[index], goal, dt, KNOB_SECONDS, calm);
				moving ||= current.knobs[index] !== goal;
			});
			paint();
			return moving || (!current.touched && !calm);
		},
		[apply, most, paint],
	);

	const kick = useLoop(tick, inView);

	const flip = (id: FixId) => {
		const current = motion.current;
		current.touched = true;
		apply({ ...current.fixes, [id]: !current.fixes[id] });
		if (asksForStillness()) {
			tick(1, true);
			return;
		}
		kick();
	};

	const onKeyDown = (event: React.KeyboardEvent) => {
		const index = Number(event.key) - 1;
		if (index >= 0 && index < FIXES.length) {
			event.preventDefault();
			flip(FIXES[index].id);
		}
	};

	const onClick = (event: React.MouseEvent) => {
		const hit = (event.target as Element).closest("[data-switch]");
		const index = Number(hit?.getAttribute("data-switch"));
		if (hit && index < FIXES.length) flip(FIXES[index].id);
	};

	return (
		<div ref={anchor} className="tkx" onKeyDown={onKeyDown}>
			<div className="tkx-art" onClick={onClick}>
				{children}
			</div>
			<div className="tkx-bins" aria-hidden="true">
				{BINS.map((bin, index) => (
					<span key={bin.id}>
						<span className="tkx-bin-name">{bin.short}</span>
						<span className="tkx-bin-cost" data-bin-cost={index}>
							{cents(start.rows[index].cost)}
						</span>
					</span>
				))}
			</div>
			<div className="tkx-switches" role="group" aria-label="Fixes">
				{FIXES.map((fix, index) => (
					<button key={fix.id} type="button" aria-pressed={fixes[fix.id]} onClick={() => flip(fix.id)}>
						<span className="tkx-switch-key">{index + 1}</span>
						{fix.label}
					</button>
				))}
				<button type="button" disabled aria-pressed={false}>
					<span className="tkx-switch-key">5</span>
					Bounded tool output · held back
				</button>
			</div>
		</div>
	);
}
