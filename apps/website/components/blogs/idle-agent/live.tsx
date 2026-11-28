"use client";

import * as React from "react";
import { asksForStillness, follow, useInView, useLoop, useReadout } from "@/components/iso/live";
import type { Flat } from "@/components/iso/iso-kit";
import { angleOf } from "./geometry";
import { MOST_AGENTS, QUIET_MINUTES, STANDARD_RATE, count, memoryAt, mib } from "./model";

const MINUTES_PER_SECOND = 1;
const FLARE_SECONDS = 2.4;
const TOUR_STEP = 0.55;
const TOUR_ASLEEP = 3;

type CellState = "off" | "pilot" | "flare";

interface Parts {
	cells: Element[];
	native: Element | null;
	reference: Element | null;
	counter: Element | null;
	rider: Element | null;
	lamp: Element | null;
	leverOpen: SVGElement | null;
	leverShut: SVGElement | null;
}

interface Sim {
	cells: CellState[];
	flare: number[];
	awake: boolean;
	quiet: number;
	minutes: number;
	native: number;
	reference: number;
	rider: number;
	touched: boolean;
	tour: number;
	started: boolean;
}

const openOf = (cells: CellState[]) => cells.filter((cell) => cell !== "off").length;

function describe(sim: Sim) {
	const open = openOf(sim.cells);
	if (!sim.awake) return { readout: `Asleep · ${count(sim.minutes)} machine min`, valuetext: `Computer asleep. No agents open, no memory held, no machine minutes counted. ${count(sim.minutes)} machine minutes so far.` };
	const memory = memoryAt(open, "native") ?? 0;
	const quiet = Math.floor(sim.quiet);
	return {
		readout: `${open} open · ${mib(memory)} · ${quiet} min quiet`,
		valuetext: `${open} of ${MOST_AGENTS} agents open, holding ${mib(memory)}. Quiet for ${quiet} of ${QUIET_MINUTES} minutes before the computer sleeps. ${count(sim.minutes)} machine minutes so far.`,
	};
}

function startOf(): Sim {
	return {
		cells: Array.from({ length: MOST_AGENTS }, () => "pilot"),
		flare: Array.from({ length: MOST_AGENTS }, () => 0),
		awake: true,
		quiet: 0,
		minutes: 0,
		native: memoryAt(MOST_AGENTS, "native") ?? 0,
		reference: memoryAt(MOST_AGENTS, "reference") ?? 0,
		rider: 0,
		touched: false,
		tour: 0,
		started: false,
	};
}

export function IdleLive({ ex, minuteStep, children }: { ex: Flat; minuteStep: number; children: React.ReactNode }) {
	const { anchor, say } = useReadout<HTMLDivElement>();
	const inView = useInView(anchor);
	const sim = React.useRef<Sim>(startOf());
	const parts = React.useRef<Parts | null>(null);
	const [label, setLabel] = React.useState(() => describe(startOf()).valuetext);
	const [open, setOpen] = React.useState(MOST_AGENTS);

	const partsOf = React.useCallback(() => {
		if (parts.current) return parts.current;
		const root = anchor.current;
		if (!root) return null;
		parts.current = {
			cells: Array.from({ length: MOST_AGENTS }, (_, index) => root.querySelector(`[data-cell="${index}"]`)).filter((node): node is Element => Boolean(node)),
			native: root.querySelector('[data-needle="native"]'),
			reference: root.querySelector('[data-needle="reference"]'),
			counter: root.querySelector('[data-slot="counter"]'),
			rider: root.querySelector("[data-rider]"),
			lamp: root.querySelector('[data-light="awake"] > .iso-solid'),
			leverOpen: root.querySelector<SVGElement>('[data-lever="open"]'),
			leverShut: root.querySelector<SVGElement>('[data-lever="shut"]'),
		};
		return parts.current;
	}, [anchor]);

	const paint = React.useCallback(() => {
		const found = partsOf();
		if (!found) return;
		const current = sim.current;
		found.cells.forEach((node) => {
			const state = current.cells[Number(node.getAttribute("data-cell"))];
			if (node.getAttribute("data-state") !== state) node.setAttribute("data-state", state);
		});
		found.native?.setAttribute("transform", `rotate(${angleOf(current.native).toFixed(2)})`);
		found.reference?.setAttribute("transform", `rotate(${angleOf(current.reference).toFixed(2)})`);
		const minutes = count(current.minutes);
		if (found.counter && found.counter.textContent !== minutes) found.counter.textContent = minutes;
		found.rider?.setAttribute("transform", `translate(${(ex[0] * current.rider).toFixed(2)} ${(ex[1] * current.rider).toFixed(2)})`);
		found.lamp?.toggleAttribute("data-lit", current.awake);
		if (found.leverOpen) found.leverOpen.style.opacity = current.awake ? "1" : "0";
		if (found.leverShut) found.leverShut.style.opacity = current.awake ? "0" : "1";
		say(describe(current).readout);
	}, [ex, partsOf, say]);

	const sleep = React.useCallback(() => {
		const current = sim.current;
		current.awake = false;
		current.quiet = 0;
		current.cells = current.cells.map(() => "off");
		current.flare = current.flare.map(() => 0);
	}, []);

	const wake = React.useCallback((calm: boolean) => {
		const current = sim.current;
		const next = current.cells.indexOf("off");
		if (next === -1) {
			sleep();
			return;
		}
		current.awake = true;
		current.quiet = 0;
		current.cells[next] = calm ? "pilot" : "flare";
		current.flare[next] = calm ? 0 : FLARE_SECONDS;
	}, [sleep]);

	const settle = React.useCallback(() => {
		const current = sim.current;
		const open = openOf(current.cells);
		current.native = current.awake ? (memoryAt(open, "native") ?? 0) : 0;
		current.reference = current.awake && open ? (memoryAt(open, "reference") ?? 0) : 0;
		current.rider = Math.min(QUIET_MINUTES, current.quiet) * minuteStep;
	}, [minuteStep]);

	const tick = React.useCallback(
		(dt: number, calm: boolean) => {
			const current = sim.current;
			if (calm) {
				settle();
				paint();
				return false;
			}
			if (!current.started) {
				current.started = true;
				sleep();
			}
			if (!current.touched) {
				current.tour += dt;
				if (!current.awake && current.tour > TOUR_ASLEEP) {
					current.tour = 0;
					wake(false);
				} else if (current.awake && openOf(current.cells) < MOST_AGENTS && current.tour > TOUR_STEP) {
					current.tour = 0;
					wake(false);
				}
			}
			if (current.awake) {
				const minutes = dt * MINUTES_PER_SECOND;
				current.minutes += minutes * STANDARD_RATE;
				current.quiet += minutes;
				if (current.quiet >= QUIET_MINUTES) {
					sleep();
					current.tour = 0;
				}
			}
			current.flare = current.flare.map((left, index) => {
				if (left <= 0) return 0;
				const rest = left - dt;
				if (rest <= 0 && current.cells[index] === "flare") current.cells[index] = "pilot";
				return Math.max(0, rest);
			});
			const open = openOf(current.cells);
			current.native = follow(current.native, current.awake ? (memoryAt(open, "native") ?? 0) : 0, dt, 0.35);
			current.reference = follow(current.reference, current.awake && open ? (memoryAt(open, "reference") ?? 0) : 0, dt, 0.35);
			current.rider = follow(current.rider, Math.min(QUIET_MINUTES, current.quiet) * minuteStep, dt, 0.12);
			paint();
			return true;
		},
		[minuteStep, paint, settle, sleep, wake],
	);

	const kick = useLoop(tick, inView);

	const act = (change: (calm: boolean) => void) => {
		const calm = asksForStillness();
		const current = sim.current;
		if (!current.touched) {
			current.touched = true;
			if (!current.started) current.started = true;
		}
		change(calm);
		const description = describe(current);
		setLabel(description.valuetext);
		setOpen(openOf(current.cells));
		if (calm) {
			settle();
			paint();
		} else kick();
	};

	const onKeyDown = (event: React.KeyboardEvent) => {
		if (event.key === "ArrowRight" || event.key === "ArrowUp" || event.key === "Enter" || event.key === " ") {
			event.preventDefault();
			act(wake);
		} else if (event.key === "Home" || event.key === "Escape" || event.key === "ArrowLeft" || event.key === "ArrowDown") {
			event.preventDefault();
			act(sleep);
		}
	};

	return (
		<div
			ref={anchor}
			className="ia"
			role="slider"
			tabIndex={0}
			aria-label="Open agents"
			aria-valuemin={0}
			aria-valuemax={MOST_AGENTS}
			aria-valuenow={open}
			aria-valuetext={label}
			onKeyDown={onKeyDown}
			onClick={() => act(wake)}
		>
			{children}
		</div>
	);
}
