"use client";

import * as React from "react";
import { asksForStillness, follow, useInView, useLoop, useReadout } from "@/components/iso/live";
import type { Flat } from "@/components/iso/iso-kit";
import { ASK, HERO, PHASES, stepOf, type Lamp } from "./cockpit-model";

const LAMPS: Lamp[] = ["working", "needs", "done"];
const LIFT_AT = 1;
const DOCK_AT = PHASES.length - 1;

type Decision = "allow" | "deny" | null;

interface Parts {
	lamps: Element[];
	dock: Element | null;
	logs: Element[];
	cursor: SVGRectElement | null;
	live: Element | null;
	card: SVGGElement | null;
	summary: SVGGElement | null;
	changes: Element | null;
	spin: Element | null;
	body: Element | null;
	halo: SVGPathElement | null;
	rider: Element | null;
}

export function CockpitLive({ ex, up, step, lift, children }: { ex: Flat; up: Flat; step: number; lift: number; children: React.ReactNode }) {
	const { anchor, say } = useReadout<HTMLDivElement>();
	const inView = useInView(anchor);
	const [spoken, setSpoken] = React.useState("");
	const state = React.useRef({ index: HERO.still, clock: 0, decision: null as Decision, lifted: false, height: 0, rider: HERO.still * step, touched: false, started: false });
	const parts = React.useRef<Parts | null>(null);

	const partsOf = React.useCallback(() => {
		if (parts.current) return parts.current;
		const root = anchor.current;
		if (!root) return null;
		const solid = (selector: string) => root.querySelector(`${selector} > .iso-solid`);
		parts.current = {
			lamps: LAMPS.map((_, index) => solid(`[data-lamp="${index}"]`)).filter((node): node is Element => Boolean(node)),
			dock: solid("[data-dock-lamp]"),
			logs: Array.from(root.querySelectorAll("[data-log]")),
			cursor: root.querySelector<SVGRectElement>("[data-cursor]"),
			live: root.querySelector('[data-slot="phone-live"]'),
			card: root.querySelector<SVGGElement>("[data-card]"),
			summary: root.querySelector<SVGGElement>("[data-summary]"),
			changes: root.querySelector('[data-slot="phone-changes"]'),
			spin: root.querySelector("[data-spin]"),
			body: root.querySelector("[data-phone-body]"),
			halo: root.querySelector<SVGPathElement>("[data-lift-halo]"),
			rider: root.querySelector("[data-rider]"),
		};
		return parts.current;
	}, [anchor]);

	const paint = React.useCallback(() => {
		const found = partsOf();
		if (!found) return;
		const current = state.current;
		const phase = stepOf(current.index, current.decision);
		const lamp = LAMPS.indexOf(phase.lamp);
		found.lamps.forEach((node, index) => node.toggleAttribute("data-lit", index === lamp));
		found.dock?.toggleAttribute("data-lit", phase.lamp === "needs");
		found.logs.forEach((node, index) => {
			node.toggleAttribute("data-shown", index < phase.log);
			node.toggleAttribute("data-ask", phase.lamp === "needs" && index === phase.log - 1);
		});
		const next = found.logs[Math.min(phase.log, found.logs.length - 1)];
		if (found.cursor && next) found.cursor.setAttribute("y", String(Number(next.getAttribute("y")) - 0.4));
		if (found.live && found.live.textContent !== phase.phone) found.live.textContent = phase.phone;
		if (found.card) found.card.style.opacity = phase.lamp === "needs" ? "1" : "0";
		if (found.summary) found.summary.style.opacity = phase.lamp === "done" ? "1" : "0";
		const denied = current.decision === "deny";
		const changes = denied ? "Test run denied" : "+20 −13 · 2 files";
		if (found.changes && found.changes.textContent !== changes) found.changes.textContent = changes;
		found.spin?.toggleAttribute("data-on", phase.lamp === "working");
		const rise = current.height * lift;
		found.body?.setAttribute("transform", rise ? `translate(${(up[0] * rise).toFixed(2)} ${(up[1] * rise).toFixed(2)})` : "");
		if (found.halo) found.halo.style.opacity = String(current.height * 0.9);
		found.rider?.setAttribute("transform", `translate(${(ex[0] * current.rider).toFixed(2)} ${(ex[1] * current.rider).toFixed(2)})`);
		const text = phase.readout + (current.lifted ? " · in hand" : " · docked");
		say(text);
		setSpoken((before) => (before === phase.phone ? before : phase.phone));
	}, [ex, up, lift, partsOf, say]);

	const tick = React.useCallback(
		(dt: number, calm: boolean) => {
			const current = state.current;
			if (calm) {
				current.height = current.lifted ? 1 : 0;
				current.rider = Math.min(current.index, PHASES.length - 1) * step;
				paint();
				return false;
			}
			if (!current.started) {
				current.started = true;
				current.index = 0;
				current.clock = 0;
				current.rider = 0;
			}
			const phase = stepOf(current.index, current.decision);
			const waiting = current.index === ASK && current.touched;
			if (!waiting) current.clock += dt;
			if (current.clock >= phase.seconds) {
				current.clock = 0;
				if (current.index === ASK && !current.touched) current.decision = "allow";
				if (current.index >= PHASES.length - 1 || phase.id === "denied") {
					current.index = 0;
					current.decision = null;
				} else {
					current.index += 1;
				}
				if (!current.touched && current.index === LIFT_AT) current.lifted = true;
				if (!current.touched && current.index === DOCK_AT) current.lifted = false;
			}
			current.height = follow(current.height, current.lifted ? 1 : 0, dt, 0.28);
			current.rider = follow(current.rider, Math.min(current.index, PHASES.length - 1) * step, dt, 0.3);
			paint();
			return true;
		},
		[paint, step],
	);

	const kick = useLoop(tick, inView);

	const act = (action: "allow" | "deny" | "lift") => {
		const current = state.current;
		current.touched = true;
		if (action === "lift") current.lifted = !current.lifted;
		else if (current.index === ASK && current.decision === null) {
			current.decision = action;
			current.index = action === "allow" ? ASK + 1 : PHASES.length;
			current.clock = 0;
		} else if (current.index !== ASK) return act("lift");
		if (asksForStillness()) {
			current.height = current.lifted ? 1 : 0;
			current.rider = Math.min(current.index, PHASES.length - 1) * step;
			paint();
		} else kick();
	};

	const onKeyDown = (event: React.KeyboardEvent) => {
		const key = event.key.toLowerCase();
		const action = key === "a" || key === "enter" ? "allow" : key === "d" ? "deny" : key === "l" || key === " " ? "lift" : null;
		if (!action) return;
		event.preventDefault();
		act(action);
	};

	const onClick = (event: React.MouseEvent) => {
		const target = (event.target as Element).closest("[data-key]")?.getAttribute("data-key");
		act(target === "deny" ? "deny" : target === "allow" ? "allow" : state.current.index === ASK ? "allow" : "lift");
	};

	return (
		<div ref={anchor} className="cp" role="group" tabIndex={0} aria-label="Cockpit figure. Press A to allow, D to deny, L to lift the handheld." onKeyDown={onKeyDown} onClick={onClick}>
			{children}
			<span className="sr-only" aria-live="polite">
				{spoken}
			</span>
		</div>
	);
}
