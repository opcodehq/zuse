"use client";

import * as React from "react";
import "./widgets.css";

const COLUMNS = 80;
const CHAR = 0.6;
const READABLE = 13;

const LOG = [
	"$ agent run --task 'fix the flaky retry test in api/client.test.ts'",
	"> reading api/client.ts (214 lines)",
	"> reading api/client.test.ts (96 lines)",
	"  the test measures wall time around retry(); retry() sleeps a fixed 100 ms",
	"  between attempts, so three attempts on a busy runner overrun the 250 ms limit",
	"> editing api/client.ts",
	"  @@ -18,16 +18,20 @@ export async function retry<T>(run: () => Promise<T>, a",
	"  -  for (let i = 0; i < attempts; i++) {",
	"  +  return withBackoff(run, attempts, { base: 100, cap: 2_000 });",
	"> editing api/client.test.ts",
	"  +    vi.useFakeTimers();",
	"? run 'bun test api/client.test.ts' [y/N/always] _",
];

const STEPS = [
	["Read", "client.ts"],
	["Read", "client.test.ts"],
	["Edited", "client.ts  +14 −10"],
	["Edited", "client.test.ts  +6 −3"],
	["Ran", "bun test api/client.test.ts"],
];

export function TerminalOrTurn() {
	const frame = React.useRef<HTMLDivElement>(null);
	const [width, setWidth] = React.useState(358);
	const [zoom, setZoom] = React.useState(1);
	const [open, setOpen] = React.useState(false);
	React.useEffect(() => {
		const node = frame.current;
		if (!node) return;
		const observer = new ResizeObserver(([entry]) => setWidth(Math.min(358, entry.contentRect.width)));
		observer.observe(node);
		return () => observer.disconnect();
	}, []);
	const fit = width / (COLUMNS * CHAR);
	const size = fit * zoom;
	const visible = Math.min(COLUMNS, Math.floor(width / (size * CHAR)));
	const readableAt = READABLE / fit;
	return (
		<div className="not-prose pw-panel" role="group" aria-label="The same turn as a terminal and as a turn">
			<p className="editorial-label">Fig 2 · Pinch the terminal</p>
			<div className="pw-split">
				<div className="pw-col">
					<p className="pw-head">As a terminal, on a 390 pt phone</p>
					<div ref={frame} className="pw-term-frame">
						<div className="pw-term" style={{ width: Math.max(width, COLUMNS * CHAR * size + 16), fontSize: size }}>
							{LOG.map((line, index) => (
								<div key={index} data-ask={line.startsWith("?") ? "" : undefined}>
									{line}
								</div>
							))}
						</div>
					</div>
					<label className="pw-slider">
						<span>Zoom</span>
						<input type="range" min={1} max={3} step={0.05} value={zoom} onChange={(event) => setZoom(Number(event.target.value))} />
						<output>{zoom.toFixed(1)}×</output>
					</label>
					<p className="pw-readout" aria-live="polite">
						{size.toFixed(1)} px text · {visible} of {COLUMNS} columns on screen
						{size < READABLE ? ` · readable from ${readableAt.toFixed(1)}×` : visible < COLUMNS ? " · now scroll sideways for the rest" : ""}
					</p>
				</div>
				<div className="pw-col">
					<p className="pw-head">As a turn</p>
					<div className="pw-turn">
						<button type="button" className="pw-fold" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
							<span className="pw-chev" data-open={open ? "" : undefined} aria-hidden="true" />
							<span className="pw-fold-title">Read 2 files · edited 2 files · ran 1 command</span>
						</button>
						{open ? (
							<ul className="pw-steps">
								{STEPS.map(([verb, what], index) => (
									<li key={index}>
										<span>{verb}</span> {what}
									</li>
								))}
							</ul>
						) : null}
						<p className="pw-done">Worked for 1m 5s</p>
						<div className="pw-card">
							<p className="pw-card-title">Approval required</p>
							<code>bun test api/client.test.ts</code>
							<div className="pw-card-keys">
								<span className="pw-deny">Deny</span>
								<span className="pw-allow">Allow once</span>
							</div>
						</div>
					</div>
					<p className="pw-readout">13 px and up, nothing to scroll sideways. Tap the line for the steps.</p>
				</div>
			</div>
		</div>
	);
}
