"use client";

import * as React from "react";
import { ASK_TEXT, COMMAND, DIFF, STEER, TITLE, count, type FileDiff } from "./phone-data";
import "./phone.css";

type Scene = "approve" | "diff" | "steer";
type Turn = "editing" | "asking" | "running" | "replying" | "done" | "denied" | "stopped";

interface Sent {
	id: number;
	text: string;
	confirmed: boolean;
}

const LIVE: Record<Turn, string> = {
	editing: "Editing client.ts",
	asking: "Waiting for you",
	running: "Running bun test api/client.test.ts",
	replying: "Writing…",
	done: "Read 2 files · edited 2 files · ran 1 command",
	denied: "Read 2 files · edited 2 files",
	stopped: "Read 2 files · edited client.ts",
};

const REPLY: Partial<Record<Turn, string>> = {
	done: "The test timed real sleeps, so a slow machine pushed it past its 250 ms budget. Retries now back off from 100 ms up to 2 s inside a private helper, and the test uses fake timers. 12 tests pass.",
	denied: "Understood, I haven't run anything. The fix is in client.ts and the test now uses fake timers. Run bun test api/client.test.ts when you're ready.",
	stopped: "Stopped.",
};

const START: Record<Scene, Turn> = { approve: "asking", diff: "done", steer: "editing" };

const NOTE: Record<string, string> = {
	asking: "The agent stopped at a permission request. The card shows the exact command, and nothing runs until you answer.",
	allowed: "Allow once answers this request and no other. The next command that needs permission will ask again.",
	denied: "Deny goes back to the agent as a refusal. It decides what to do next; here it stops and says what it would have run.",
	doneDiff: "The reply is a summary. Changes asks the computer for the real diff.",
	diff: "One file per section, its header pinned while you scroll, lines wrapped instead of scrolling sideways.",
	empty: "Nothing staged here. Each scope is read from the computer when you pick it.",
	running: "Type while it works and the stop button turns back into send.",
	typed: "Mid-turn, a written message is a follow-up. The computer queues it, and agents that support it take it at their next step.",
	queued: "Offline, the message waits on the phone, encrypted, marked Queued until the computer confirms it.",
	confirmed: "The computer confirmed it. The agent picked it up without stopping the turn.",
	stopped: "Stop interrupts the turn. Nothing you queued is lost.",
	finished: "The turn is done. Open the chat on any computer and it is the same conversation.",
};

function Spinner({ on }: { on: boolean }) {
	return <span className={`pc-spin ${on ? "pc-spin-on" : ""}`} aria-hidden="true" />;
}

function DiffSheet({ onClose }: { onClose: () => void }) {
	const [scope, setScope] = React.useState<"unstaged" | "staged" | "branch">("unstaged");
	const [collapsed, setCollapsed] = React.useState<Set<string>>(new Set());
	const files: FileDiff[] = scope === "unstaged" ? DIFF : [];
	const toggle = (path: string) =>
		setCollapsed((before) => {
			const next = new Set(before);
			if (!next.delete(path)) next.add(path);
			return next;
		});
	return (
		<div className="pc-sheet" role="dialog" aria-label="Changes">
			<div className="pc-sheet-bar">
				<span className="pc-sheet-title">Changes</span>
				<button type="button" className="pc-link" onClick={onClose}>
					Done
				</button>
			</div>
			<div className="pc-segmented" role="radiogroup" aria-label="Scope">
				{(
					[
						["unstaged", "Working tree"],
						["staged", "Staged"],
						["branch", "Branch"],
					] as const
				).map(([id, label]) => (
					<button key={id} type="button" role="radio" aria-checked={scope === id} onClick={() => setScope(id)}>
						{label}
					</button>
				))}
			</div>
			<div className="pc-diff">
				{files.length === 0 ? <p className="pc-empty">No changes</p> : null}
				{files.map((file) => (
					<section key={file.path}>
						<button type="button" className="pc-file" onClick={() => toggle(file.path)} aria-expanded={!collapsed.has(file.path)}>
							<span className="pc-chevron" data-open={collapsed.has(file.path) ? undefined : ""} aria-hidden="true" />
							<span className="pc-path">{file.path}</span>
							<span className="pc-add">+{count(file, "add")}</span>
							<span className="pc-del">−{count(file, "del")}</span>
						</button>
						{collapsed.has(file.path) ? null : (
							<div>
								{file.rows.map((row, index) => (
									<div key={index} className="pc-row" data-kind={row.kind}>
										{row.kind === "hunk" ? null : (
											<span className="pc-nums" aria-hidden="true">
												<span>{row.old ?? ""}</span>
												<span>{row.new ?? ""}</span>
											</span>
										)}
										<span className="pc-code">{row.text || " "}</span>
									</div>
								))}
							</div>
						)}
					</section>
				))}
			</div>
		</div>
	);
}

export function PhoneCockpit({ scene, fig, title }: { scene: Scene; fig: string; title: string }) {
	const [turn, setTurn] = React.useState<Turn>(START[scene]);
	const [note, setNote] = React.useState(scene === "approve" ? NOTE.asking : scene === "diff" ? NOTE.doneDiff : NOTE.running);
	const [sheet, setSheet] = React.useState(false);
	const [draft, setDraft] = React.useState("");
	const [sent, setSent] = React.useState<Sent[]>([]);
	const [offline, setOffline] = React.useState(false);
	const [seconds, setSeconds] = React.useState(scene === "steer" ? 12 : 41);
	const [steered, setSteered] = React.useState(false);
	const timers = React.useRef<number[]>([]);
	const turnRef = React.useRef(turn);
	React.useEffect(() => {
		turnRef.current = turn;
	}, [turn]);
	const live = turn === "editing" || turn === "running" || turn === "replying" || turn === "asking";

	React.useEffect(() => () => timers.current.forEach((timer) => window.clearTimeout(timer)), []);
	React.useEffect(() => {
		if (!live || turn === "asking") return;
		const timer = window.setInterval(() => setSeconds((value) => value + 1), 1000);
		return () => window.clearInterval(timer);
	}, [live, turn]);

	const later = (ms: number, run: () => void) => {
		timers.current.push(window.setTimeout(run, ms));
	};

	const finish = (end: Turn, message: string) => {
		setTurn("replying");
		later(1100, () => {
			setTurn(end);
			setNote(message);
		});
	};

	const decide = (allow: boolean) => {
		if (turn !== "asking") return;
		if (allow) {
			setTurn("running");
			setNote(NOTE.allowed);
			later(1800, () => finish("done", NOTE.allowed + " The turn finished; its changes are one tap away."));
		} else {
			setNote(NOTE.denied);
			finish("denied", NOTE.denied);
		}
	};

	const confirm = (id: number) => {
		setSent((list) => list.map((item) => (item.id === id ? { ...item, confirmed: true } : item)));
		if (turnRef.current !== "editing") return;
		setSteered(true);
		setNote(NOTE.confirmed);
		later(2200, () => {
			if (turnRef.current !== "editing") return;
			setTurn("running");
			later(1800, () => {
				if (turnRef.current === "running") finish("done", NOTE.finished);
			});
		});
	};

	const send = () => {
		const text = draft.trim();
		if (!text) return;
		const id = Date.now();
		setSent((list) => [...list, { id, text, confirmed: false }]);
		setDraft("");
		if (offline) {
			setNote(NOTE.queued);
			return;
		}
		setNote(NOTE.typed);
		later(900, () => confirm(id));
	};

	const goOnline = (next: boolean) => {
		setOffline(!next);
		if (next) {
			const pending = sent.filter((item) => !item.confirmed);
			pending.forEach((item, index) => later(700 + index * 300, () => confirm(item.id)));
			if (!pending.length) setNote(NOTE.running);
		}
	};

	const stop = () => {
		setTurn("stopped");
		setNote(NOTE.stopped);
	};

	const reset = () => {
		timers.current.forEach((timer) => window.clearTimeout(timer));
		timers.current = [];
		setTurn(START[scene]);
		setNote(scene === "approve" ? NOTE.asking : scene === "diff" ? NOTE.doneDiff : NOTE.running);
		setSheet(false);
		setDraft("");
		setSent([]);
		setOffline(false);
		setSteered(false);
		setSeconds(scene === "steer" ? 12 : 41);
	};

	const stopsTurn = live && turn !== "asking" && !draft.trim();
	const reply = REPLY[turn];
	const settled = turn === "done" || turn === "denied" || turn === "stopped";

	return (
		<figure className="not-prose pc-figure" data-scene={scene}>
			<div className="pc-layout">
				<div className="pc-phone" aria-label={`${title}, an interactive phone`}>
					<div className="pc-screen">
						<div className="pc-status">
							<span>9:41</span>
							<span className="pc-conn" data-offline={offline ? "" : undefined}>
								{offline ? "Reconnecting…" : "Connected · Local network · 12 ms"}
							</span>
						</div>
						<div className="pc-header">
							<span className="pc-title">{TITLE}</span>
							<button type="button" className="pc-changes" data-ready={settled && turn !== "stopped" ? "" : undefined} onClick={() => {
								setSheet(true);
								setNote(NOTE.diff);
							}}>
								Changes
							</button>
						</div>
						<div className="pc-transcript">
							<p className="pc-user">{ASK_TEXT}</p>
							<div className="pc-activity">
								<Spinner on={live && turn !== "asking"} />
								<span className="pc-live">{LIVE[turn]}</span>
								<span className="pc-time">{settled ? (turn === "done" ? "1m 5s" : turn === "denied" ? "48s" : `${seconds}s`) : turn === "asking" ? "" : `${seconds}s`}</span>
							</div>
							{sent.map((item) => (
								<div key={item.id} className="pc-sent">
									<p className="pc-user">{item.text}</p>
									{item.confirmed ? null : <span className="pc-delivery">Queued</span>}
								</div>
							))}
							{steered ? <p className="pc-narration">Keeping retry() public; the backoff goes in a private helper.</p> : null}
							{turn === "asking" ? (
								<div className="pc-permission">
									<p className="pc-permission-title">Approval required</p>
									<code>{COMMAND}</code>
									<div className="pc-permission-keys">
										<button type="button" className="pc-deny" onClick={() => decide(false)}>
											Deny
										</button>
										<button type="button" className="pc-allow" onClick={() => decide(true)}>
											Allow once
										</button>
									</div>
								</div>
							) : null}
							{reply ? <p className="pc-reply">{reply}</p> : null}
							{turn === "done" ? <p className="pc-footer">Worked for 1m 5s · <span className="pc-add">+20</span> <span className="pc-del">−13</span></p> : null}
						</div>
						<form
							className="pc-composer"
							onSubmit={(event) => {
								event.preventDefault();
								if (stopsTurn) stop();
								else send();
							}}
						>
							<input
								value={draft}
								onChange={(event) => setDraft(event.target.value)}
								placeholder={offline ? "Reconnecting to your computer…" : live ? "Send a follow-up…" : "Ask Zuse"}
								aria-label="Message"
								onFocus={() => (live && turn !== "asking" ? setNote(NOTE.running) : null)}
							/>
							<button type="submit" className="pc-send" aria-label={stopsTurn ? "Stop response" : "Send message"} disabled={stopsTurn ? offline : !draft.trim()}>
								<span className={stopsTurn ? "pc-stop-glyph" : "pc-send-glyph"} aria-hidden="true" />
							</button>
						</form>
						{sheet ? (
							<DiffSheet
								onClose={() => {
									setSheet(false);
									setNote(NOTE.doneDiff);
								}}
							/>
						) : null}
					</div>
				</div>
				<div className="pc-side">
					<p className="editorial-label">
						{fig} · {title}
					</p>
					<p className="pc-note" aria-live="polite">
						{note}
					</p>
					<div className="pc-actions">
						{scene === "steer" ? (
							<>
								<button type="button" onClick={() => setDraft(STEER)} disabled={!live || turn === "asking"}>
									Write a steer
								</button>
								<button type="button" aria-pressed={offline} onClick={() => goOnline(offline)}>
									{offline ? "Reconnect" : "Go offline"}
								</button>
							</>
						) : null}
						<button type="button" onClick={reset}>
							Start over
						</button>
					</div>
				</div>
			</div>
		</figure>
	);
}
