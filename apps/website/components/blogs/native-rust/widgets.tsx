"use client";

import * as React from "react";
import { Plate } from "@/components/iso/draw";
import { useStillness } from "@/components/iso/live";
import { IDLE_BUG, SCROLL_1500, TYPING } from "./bench";
import "@/components/iso/iso.css";
import "./native-rust.css";

function Choice<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: [T, string][]; onChange: (value: T) => void }) {
	return (
		<div className="nr-row" role="radiogroup" aria-label={label}>
			<p>{label}</p>
			{options.map(([key, text]) => (
				<button key={key} type="button" role="radio" aria-checked={value === key} onClick={() => onChange(key)}>
					{text}
				</button>
			))}
		</div>
	);
}

const FRAMES = 64;

export function IdleLoop() {
	const [dated, setDated] = React.useState<"past" | "future">("future");
	const [build, setBuild] = React.useState<"before" | "after">("before");
	const still = useStillness();
	const cells = React.useRef<(HTMLSpanElement | null)[]>([]);
	const looping = dated === "future" && build === "before";
	const state = build === "after" ? IDLE_BUG.fixed : looping ? IDLE_BUG.skewed : IDLE_BUG.normal;
	const lit = React.useCallback((frame: number) => (looping ? true : frame === 0 || frame === FRAMES / 2), [looping]);

	React.useEffect(() => {
		if (still) {
			cells.current.forEach((node, frame) => node?.toggleAttribute("data-on", lit(frame)));
			return;
		}
		let raf = 0;
		const start = performance.now();
		const step = (now: number) => {
			const head = Math.floor(((now - start) / 1000) * FRAMES) % FRAMES;
			cells.current.forEach((node, frame) => {
				const age = (head - frame + FRAMES) % FRAMES;
				node?.toggleAttribute("data-on", lit(frame) && (looping || age < 12));
				node?.toggleAttribute("data-head", age < 4);
			});
			raf = requestAnimationFrame(step);
		};
		raf = requestAnimationFrame(step);
		return () => cancelAnimationFrame(raf);
	}, [still, looping, lit]);

	const message = dated === "future" ? 82 : 30;
	const now = 56;
	const seen = build === "after" ? Math.max(now, message) : now;
	const verdict = looping
		? "The last message is dated after this computer's clock. Marking the chat seen stamps now, which is still before the message, so the chat stays unseen and gets marked again on the next frame. Each mark rebuilds the whole window."
		: build === "after"
			? "The seen stamp never lands before the last message, so one mark is enough and the window goes quiet."
			: "The message is dated in the past, so one mark settles it. The bug only shows with clock skew from another device or with imported history.";

	return (
		<div className="iso nr-widget nr-idle not-prose">
			<Plate
				fig="Fig 2"
				title="An idle window"
				hint="Change the date or the build"
				readout={state.renders + " rebuilds / s"}
				caption="20 chats, idle, window in front. One square is one frame of a second. Measured on the same Mac as Fig 1; the CPU of the selected build is the app's total."
			>
				<div className="nr-panel">
					<Choice label="Last message" value={dated} options={[["past", "Dated in the past"], ["future", "Dated in the future"]]} onChange={setDated} />
					<Choice label="Build" value={build} options={[["before", "Before the fix"], ["after", "After the fix"]]} onChange={setBuild} />
					<div className="nr-clock" aria-hidden="true">
						<span className="nr-clock-mark" data-kind="message" style={{ left: message + "%" }}>
							last message
						</span>
						<span className="nr-clock-mark" data-kind="now" style={{ left: now + "%" }}>
							now
						</span>
						<span className="nr-clock-mark" data-kind="seen" data-below="" style={{ left: seen + "%" }}>
							seen stamp
						</span>
					</div>
					<div className="nr-frames" aria-hidden="true">
						{Array.from({ length: FRAMES }, (_, frame) => (
							<span
								key={frame}
								ref={(node) => {
									cells.current[frame] = node;
								}}
							/>
						))}
					</div>
					<div className="nr-figures">
						<div>
							<small>Window rebuilds / s</small>
							<b>{state.renders}</b>
						</div>
						<div>
							<small>State notifications / s</small>
							<b>{state.notifications}</b>
						</div>
						<div>
							<small>CPU</small>
							<b>{state.cpu}</b>
						</div>
					</div>
					<p className="nr-verdict" aria-live="polite">
						{verdict}
					</p>
				</div>
			</Plate>
		</div>
	);
}

const MESSAGES = 1500;
const LINE = 20;
const CHROME = 26;
const OVERSCAN = 4;
const PROMPTS = [
	"Can you find why the settings test fails on Linux only?",
	"Run the full suite again and tell me what's still red.",
	"Looks good. Now split the parser change into its own commit, keep the fixture update separate and push both.",
	"Try the smaller fix first.",
	"What changed in the build script since yesterday?",
];
const REPLIES = [
	"I read the failing test and the fixture it loads. The path separator is hard-coded, so the lookup misses on Linux.",
	"I changed the lookup to join the path with the platform separator and added a case for nested folders.",
	"Ran the settings tests: 48 passed. The full suite is running now and I'll report anything that fails.",
	"Two tests are still red, both in the importer. They expect the old date format, which the last change made stricter.",
	"The build script now skips the icon step when nothing in the assets folder changed, which saves about a minute on a clean run.",
	"Committed the parser change and the fixture update separately and pushed both to the branch.",
];

function seeded(index: number) {
	let value = (index + 1) * 2654435761;
	value = (value ^ (value >>> 13)) >>> 0;
	return (value % 1000) / 1000;
}

const CHAT = Array.from({ length: MESSAGES }, (_, index) => {
	const role = index % 2 === 0 ? "user" : "agent";
	const lines = role === "user" ? 1 + Math.floor(seeded(index) * 2) : 1 + Math.floor(seeded(index) * 5);
	const text =
		role === "user"
			? PROMPTS[(index / 2) % PROMPTS.length]!
			: Array.from({ length: lines }, (_, line) => REPLIES[(Math.floor(index / 2) * 5 + line) % REPLIES.length]).join(" ");
	return { role, lines, text, height: CHROME + lines * LINE };
});
const OFFSETS = CHAT.reduce<number[]>((list, entry, index) => {
	list.push(index ? list[index - 1]! + CHAT[index - 1]!.height : 0);
	return list;
}, []);
const TOTAL = OFFSETS[MESSAGES - 1]! + CHAT[MESSAGES - 1]!.height;

function firstAt(top: number) {
	let low = 0;
	let high = MESSAGES - 1;
	while (low < high) {
		const mid = (low + high + 1) >> 1;
		if (OFFSETS[mid]! <= top) low = mid;
		else high = mid - 1;
	}
	return low;
}

function Row({ index }: { index: number }) {
	const entry = CHAT[index]!;
	return (
		<div className="nr-chat-row" data-role={entry.role} style={{ top: OFFSETS[index], height: entry.height }}>
			<small>
				{entry.role === "user" ? "You" : "Agent"} · #{index + 1}
			</small>
			<div style={{ WebkitLineClamp: entry.lines, maxHeight: entry.lines * LINE + (entry.role === "user" ? 8 : 0) }}>{entry.text}</div>
		</div>
	);
}

export function ScrollTest() {
	const [mode, setMode] = React.useState<"visible" | "all">("visible");
	const [range, setRange] = React.useState<[number, number]>([0, 20]);
	const [result, setResult] = React.useState<string>("Scroll the chat, or run the test.");
	const box = React.useRef<HTMLDivElement>(null);
	const running = React.useRef(0);
	const still = useStillness();

	const measure = React.useCallback(() => {
		const node = box.current;
		if (!node) return;
		const first = Math.max(0, firstAt(node.scrollTop) - OVERSCAN);
		const last = Math.min(MESSAGES - 1, firstAt(node.scrollTop + node.clientHeight) + OVERSCAN);
		setRange((current) => (current[0] === first && current[1] === last ? current : [first, last]));
	}, []);

	React.useEffect(() => {
		measure();
		return () => cancelAnimationFrame(running.current);
	}, [measure]);

	const run = () => {
		const node = box.current;
		if (!node) return;
		cancelAnimationFrame(running.current);
		const steps = 120;
		const distance = still ? 0 : TOTAL - node.clientHeight;
		if (!distance) {
			node.scrollTop = node.scrollTop > 0 ? 0 : TOTAL;
			measure();
			setResult("Reduced motion is on, so the test jumps instead of scrolling.");
			return;
		}
		node.scrollTop = 0;
		let step = 0;
		let last = 0;
		let longest = 0;
		let slow = 0;
		const frame = (now: number) => {
			if (last) {
				const gap = now - last;
				longest = Math.max(longest, gap);
				if (gap > 33) slow++;
			}
			last = now;
			step++;
			node.scrollTop = (distance * step) / steps;
			if (step < steps) running.current = requestAnimationFrame(frame);
			else setResult("Your browser: longest frame " + Math.round(longest) + " ms, " + slow + " of " + (steps - 1) + " frames over 33 ms.");
		};
		running.current = requestAnimationFrame(frame);
	};

	const switchTo = (next: "visible" | "all") => {
		const started = performance.now();
		setMode(next);
		requestAnimationFrame(() => requestAnimationFrame(() => setResult("Switched in " + Math.round(performance.now() - started) + " ms. Now run the test.")));
	};

	const drawn = mode === "all" ? MESSAGES : range[1] - range[0] + 1;
	const rows = [];
	const [from, to] = mode === "all" ? [0, MESSAGES - 1] : range;
	for (let index = from; index <= to; index++) rows.push(<Row key={index} index={index} />);

	return (
		<div className="iso nr-widget nr-scroll not-prose">
			<Plate
				fig="Fig 3"
				title="1,500 messages"
				hint="Scroll it, or run the test"
				readout={drawn.toLocaleString("en-US") + " of 1,500 rows drawn"}
				caption={
					"This chat runs in your browser, not in Zuse. In the app, on the same Mac and chat as Fig 1: opening the chat took " +
					SCROLL_1500.native.open +
					" ms against " +
					SCROLL_1500.electron.open +
					" ms, and the longest stall in a 120-step scroll was " +
					SCROLL_1500.native.longest +
					" ms against " +
					SCROLL_1500.electron.longest +
					" ms. Zuse had more frames over 33 ms in that scroll (" +
					SCROLL_1500.native.slow +
					" against " +
					SCROLL_1500.electron.slow +
					")."
				}
			>
				<div className="nr-panel">
					<Choice label="Draw" value={mode} options={[["visible", "What's on screen"], ["all", "All 1,500 rows"]]} onChange={switchTo} />
					<div ref={box} className="nr-chat" onScroll={measure} tabIndex={0} aria-label="A chat with 1,500 messages">
						<div style={{ height: TOTAL, position: "relative" }}>{rows}</div>
					</div>
					<div className="nr-row">
						<button type="button" onClick={run}>
							Run the test: 120 steps, top to bottom
						</button>
					</div>
					<p className="nr-verdict" aria-live="polite">
						{result}
					</p>
				</div>
			</Plate>
		</div>
	);
}

const SLOW = 10;
const SCALE = 60;

export function KeystrokeLag() {
	const still = useStillness();
	const [text, setText] = React.useState("");
	const marks = React.useRef<Record<string, HTMLElement | null>>({});
	const timers = React.useRef<number[]>([]);

	React.useEffect(() => () => timers.current.forEach((timer) => cancelAnimationFrame(timer)), []);

	const play = () => {
		if (still) return;
		const started = performance.now();
		for (const id of ["native", "electron"] as const) {
			const node = marks.current[id];
			if (!node) continue;
			const target = TYPING[id].p50;
			const step = (now: number) => {
				const elapsed = (now - started) / SLOW;
				const at = Math.min(elapsed, target);
				node.style.left = (at / SCALE) * 100 + "%";
				node.style.opacity = elapsed >= target ? "1" : "0.45";
				if (elapsed < target) timers.current.push(requestAnimationFrame(step));
			};
			node.style.left = "0%";
			timers.current.push(requestAnimationFrame(step));
		}
	};

	const lane = (id: "native" | "electron", label: string) => (
		<div className="nr-lane">
			<span>{label}</span>
			<div className="nr-track">
				<i style={{ left: (TYPING[id].p50 / SCALE) * 100 + "%" }} />
				<i data-kind="p95" style={{ left: (TYPING[id].p95 / SCALE) * 100 + "%" }} />
				<b
					ref={(node) => {
						marks.current[id] = node;
					}}
					style={{ left: (TYPING[id].p50 / SCALE) * 100 + "%" }}
				>
					{text.slice(-1) || "a"}
				</b>
			</div>
		</div>
	);

	return (
		<div className="iso nr-widget nr-keys not-prose">
			<Plate
				fig="Fig 4"
				title="Where we're slower"
				hint={still ? "Type a letter" : "Type: shown 10× slower"}
				readout={"p50 " + TYPING.native.p50 + " vs " + TYPING.electron.p50 + " ms"}
				caption={
					"Time from a keystroke to the character on screen, 40 keystrokes per run, on the same Mac as Fig 1. The dark line is the median, the light one p95: Zuse " +
					TYPING.native.p50 +
					" / " +
					TYPING.native.p95 +
					" ms, the Electron-based app " +
					TYPING.electron.p50 +
					" / " +
					TYPING.electron.p95 +
					" ms."
				}
			>
				<div className="nr-panel">
					<input
						className="nr-input"
						value={text}
						placeholder="Type here"
						aria-label="Type to replay the measured keystroke delay"
						onChange={(event) => {
							setText(event.target.value.slice(-40));
							play();
						}}
					/>
					<div className="nr-lanes">
						{lane("native", "Zuse")}
						{lane("electron", "Electron-based app")}
					</div>
					<div className="nr-scale" aria-hidden="true">
						<span>0 ms</span>
						<span>20</span>
						<span>40</span>
						<span>60 ms</span>
					</div>
				</div>
			</Plate>
		</div>
	);
}
