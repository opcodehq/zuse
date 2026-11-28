"use client";

import * as React from "react";
import { Plate } from "@/components/iso/draw";
import "@/components/iso/iso.css";
import "./racing-routes.css";

type Kind = "lan" | "tailnet" | "direct" | "relay";
const KINDS: Kind[] = ["lan", "tailnet", "direct", "relay"];
const NAME: Record<Kind, string> = { lan: "Local network", tailnet: "Tailnet", direct: "Direct", relay: "Relay" };
const RATIO = 0.7;
const MARGIN = 15;
const rank = (kind: Kind) => KINDS.indexOf(kind);

function verdict(current: Kind, now: number, candidate: Kind, rtt: number) {
	if (current === "lan") return { moves: false, text: "Not probed: nothing replaces a healthy local network." };
	if (candidate === current) return { moves: false, text: "Same route class: only a clear win would move it." };
	if (rank(candidate) > rank(current) && now <= MARGIN) return { moves: false, text: "Not probed: " + now + " ms is already under the 15 ms margin, so only a better class is measured." };
	const clearlyFaster = rtt + MARGIN <= now && rtt <= now * RATIO;
	const clearlySlower = now + MARGIN <= rtt && now <= rtt * RATIO;
	if (clearlyFaster) return { moves: true, text: "Clearly faster: at least 30% and 15 ms. It moves if the next probe, 3 s later, agrees." };
	if (rank(candidate) < rank(current) && !clearlySlower) return { moves: true, text: "Better class and not clearly slower. It moves if the next probe, 3 s later, agrees." };
	const need = Math.floor(Math.min(now - MARGIN, now * RATIO));
	return { moves: false, text: need >= 1 ? "Stays. To win it would need " + need + " ms or less." : "Stays. Nothing this fast could win by both margins." };
}

function Picker({ value, onChange, label }: { value: Kind; onChange: (kind: Kind) => void; label: string }) {
	return (
		<div className="rr-pick" role="radiogroup" aria-label={label}>
			{KINDS.map((kind) => (
				<button key={kind} type="button" role="radio" aria-checked={value === kind} onClick={() => onChange(kind)}>
					{NAME[kind]}
				</button>
			))}
		</div>
	);
}

function Slider({ value, onChange, label }: { value: number; onChange: (value: number) => void; label: string }) {
	return (
		<label className="rr-slider">
			<span>{label}</span>
			<input type="range" min={1} max={150} value={value} onChange={(event) => onChange(Number(event.target.value))} />
			<output>{value} ms</output>
		</label>
	);
}

export function SwitchRule() {
	const [current, setCurrent] = React.useState<Kind>("relay");
	const [now, setNow] = React.useState(46);
	const [candidate, setCandidate] = React.useState<Kind>("tailnet");
	const [rtt, setRtt] = React.useState(38);
	const result = verdict(current, now, candidate, rtt);
	const scale = (ms: number) => (Math.min(150, ms) / 150) * 100;
	const line = Math.max(0, Math.min(now - MARGIN, now * RATIO));
	return (
		<div className="iso rr-widget not-prose">
			<Plate
				fig="Fig 2"
				title="Would it move?"
				hint="Pick two routes"
				readout={result.moves ? "moves after 2 of 2" : "stays"}
				keys={[{ mark: "dotted", label: "The most the probed route may measure to win on speed alone" }]}
			>
				<div className="rr-rule">
					<div className="rr-row">
						<p>Carrying the session</p>
						<Picker value={current} onChange={setCurrent} label="Current route" />
						<Slider value={now} onChange={setNow} label="Measured" />
					</div>
					<div className="rr-row">
						<p>Probed</p>
						<Picker value={candidate} onChange={setCandidate} label="Candidate route" />
						<Slider value={rtt} onChange={setRtt} label="Measured" />
					</div>
					<svg className="rr-bars" viewBox="0 0 100 16" preserveAspectRatio="none" aria-hidden="true">
						<rect x="0" y="2" width={scale(now)} height="4" data-bar="current" />
						<rect x="0" y="10" width={scale(rtt)} height="4" data-bar="candidate" data-moves={result.moves ? "" : undefined} />
						{line > 0 ? <line x1={scale(line)} x2={scale(line)} y1="0" y2="16" /> : null}
					</svg>
					<p className="rr-verdict" aria-live="polite">
						{result.text}
					</p>
				</div>
			</Plate>
		</div>
	);
}
