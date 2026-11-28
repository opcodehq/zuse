"use client";

import * as React from "react";
import { asksForStillness, follow, useInView, useLoop, useReadout } from "@/components/iso/live";
import type { Flat } from "@/components/iso/iso-kit";
import { HERO, HERO_MONTH, heroState } from "./hero";

const HOURS_PER_SECOND = 4;
const HOLD_SECONDS = 3.5;
const MOST = 6;

interface Parts {
	taxi: Element | null;
	clock: Element | null;
	lamps: Element[];
	roof: Element | null;
	clockLamp: Element | null;
	flagUp: HTMLElement | SVGElement | null;
	flagDown: HTMLElement | SVGElement | null;
	rider: Element | null;
}

export function MetersLive({ ex, dayStep, children }: { ex: Flat; dayStep: number; children: React.ReactNode }) {
	const { anchor, say } = useReadout<HTMLDivElement>();
	const inView = useInView(anchor);
	const [label, setLabel] = React.useState(heroState(HERO.agents, HERO_MONTH.hours).valuetext);
	const [agents, setAgents] = React.useState(HERO.agents);
	const state = React.useRef({ agents: HERO.agents, hours: HERO_MONTH.hours, hold: 0, rider: (HERO_MONTH.days - 1) * dayStep, paused: false, started: false });
	const parts = React.useRef<Parts | null>(null);

	const partsOf = React.useCallback(() => {
		if (parts.current) return parts.current;
		const root = anchor.current;
		if (!root) return null;
		const solid = (selector: string) => root.querySelector(`${selector} > .iso-solid`);
		parts.current = {
			taxi: root.querySelector('[data-slot="taxi"]'),
			clock: root.querySelector('[data-slot="clock"]'),
			lamps: Array.from({ length: MOST }, (_, index) => solid(`[data-lamp="${index}"]`)).filter((node): node is Element => Boolean(node)),
			roof: solid('[data-light="roof"]'),
			clockLamp: solid('[data-light="clock"]'),
			flagUp: root.querySelector<SVGElement>('[data-flag="up"]'),
			flagDown: root.querySelector<SVGElement>('[data-flag="down"]'),
			rider: root.querySelector("[data-rider]"),
		};
		return parts.current;
	}, [anchor]);

	const paint = React.useCallback(() => {
		const found = partsOf();
		if (!found) return;
		const { agents: running, hours, rider } = state.current;
		const view = heroState(running, hours);
		if (found.taxi && found.taxi.textContent !== view.taxi) found.taxi.textContent = view.taxi;
		if (found.clock && found.clock.textContent !== view.clock) found.clock.textContent = view.clock;
		found.lamps.forEach((lamp, index) => lamp.toggleAttribute("data-lit", index < running));
		found.roof?.toggleAttribute("data-lit", running > 0 && hours < HERO_MONTH.hours);
		found.clockLamp?.toggleAttribute("data-lit", running > 0 && hours < HERO_MONTH.hours);
		if (found.flagUp) found.flagUp.style.opacity = running ? "0" : "1";
		if (found.flagDown) found.flagDown.style.opacity = running ? "1" : "0";
		found.rider?.setAttribute("transform", `translate(${(ex[0] * rider).toFixed(2)} ${(ex[1] * rider).toFixed(2)})`);
		say(view.readout);
	}, [ex, partsOf, say]);

	const tick = React.useCallback(
		(dt: number, calm: boolean) => {
			const current = state.current;
			if (calm) {
				current.hours = HERO_MONTH.hours;
				current.rider = (HERO_MONTH.days - 1) * dayStep;
				paint();
				return false;
			}
			if (!current.started) {
				current.started = true;
				current.hours = 0;
				current.rider = 0;
			}
			if (!current.paused) {
				if (current.hours >= HERO_MONTH.hours) {
					current.hold += dt;
					if (current.hold > HOLD_SECONDS) {
						current.hold = 0;
						current.hours = 0;
					}
				} else {
					current.hours = Math.min(HERO_MONTH.hours, current.hours + dt * HOURS_PER_SECOND);
				}
			}
			const day = Math.min(HERO_MONTH.days - 1, current.hours / HERO_MONTH.hoursPerDay);
			current.rider = current.hours === 0 ? 0 : follow(current.rider, day * dayStep, dt, 0.35);
			paint();
			return true;
		},
		[dayStep, paint],
	);

	useLoop(tick, inView);

	const change = (next: number) => {
		const value = Math.max(0, Math.min(MOST, next));
		state.current.agents = value;
		setAgents(value);
		setLabel(heroState(value, asksForStillness() ? HERO_MONTH.hours : state.current.hours).valuetext);
		paint();
	};

	const onKeyDown = (event: React.KeyboardEvent) => {
		const step = { ArrowRight: 1, ArrowUp: 1, ArrowLeft: -1, ArrowDown: -1 }[event.key];
		if (step) {
			event.preventDefault();
			change(state.current.agents + step);
		} else if (event.key === " " || event.key === "Enter") {
			event.preventDefault();
			state.current.paused = !state.current.paused;
		} else if (event.key === "Home") {
			event.preventDefault();
			change(0);
		} else if (event.key === "End") {
			event.preventDefault();
			change(MOST);
		}
	};

	return (
		<div
			ref={anchor}
			className="svm"
			role="slider"
			tabIndex={0}
			aria-label="Agents running"
			aria-valuemin={0}
			aria-valuemax={MOST}
			aria-valuenow={agents}
			aria-valuetext={label}
			onKeyDown={onKeyDown}
			onClick={() => change((state.current.agents + 1) % (MOST + 1))}
		>
			{children}
		</div>
	);
}
