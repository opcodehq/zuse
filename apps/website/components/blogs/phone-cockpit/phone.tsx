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

const LIVE: Partial<Record<Turn, string>> = {
	editing: "Editing client.ts",
	asking: "Waiting for approval",
	running: "Running bun test api/client.test.ts",
	replying: "Writing…",
};

const SUMMARY: Partial<Record<Turn, string>> = {
	done: "Read 2 files · edited 2 files · ran 1 command",
	denied: "Read 2 files · edited 2 files",
	stopped: "Read 2 files · edited client.ts",
};

const REPLY: Partial<Record<Turn, string>> = {
	done: "The test timed real sleeps, so a slow machine pushed it past its 250 ms budget. Retries now back off from 100 ms up to 2 s inside a private helper, and the test uses fake timers. 12 tests pass.",
	denied: "Understood, I haven't run anything. The fix is in client.ts and the test now uses fake timers. Run bun test api/client.test.ts when you're ready.",
};

const WORKED: Partial<Record<Turn, string>> = { done: "Worked for 1m 5s · 9:41 AM", denied: "Worked for 48s · 9:41 AM", stopped: "Stopped · 9:41 AM" };

const START: Record<Scene, Turn> = { approve: "asking", diff: "done", steer: "editing" };

const NOTE = {
	asking: "The agent stopped at a permission request. The panel above the composer shows the exact command, and nothing runs until you answer.",
	allowed: "Allow once answers this request and no other. The next command that needs permission will ask again.",
	denied: "Deny goes back to the agent as a refusal. It decides what to do next; here it stops and says what it would have run.",
	doneDiff: "The reply is a summary. Tap the changes card under it, or swipe left, to open the Changes pane beside the chat.",
	diff: "One file per section, its header pinned while you scroll, lines wrapped instead of scrolling sideways. Swipe right to go back to the chat.",
	empty: "Nothing here. Each scope is read from the computer when you pick it.",
	picker: "The last page picks more panes: the changes, a terminal or the files. Tiles with nothing behind them are hidden.",
	running: "Type while it works and the stop button turns back into send.",
	typed: "Mid-turn, a written message is a follow-up. The computer queues it, and agents that support it take it at their next step.",
	queued: "Offline, the message waits on the phone, encrypted, marked Queued above the composer until the computer confirms it.",
	confirmed: "The computer confirmed it, so it joins the chat. The agent picked it up without stopping the turn.",
	stopped: "Stop interrupts the turn. Nothing you queued is lost.",
	finished: "The turn is done. Open the chat on any computer and it is the same conversation.",
};

const PAGES = 3;

function Spinner() {
	return <span className="pc-spin" aria-hidden="true" />;
}

function Chevron({ open = false }: { open?: boolean }) {
	return <span className="pc-chevron" data-open={open ? "" : undefined} aria-hidden="true" />;
}

function DocGlyph() {
	return (
		<svg className="pc-doc" viewBox="0 0 12 14" aria-hidden="true">
			<path d="M2.5 1h4.8L10 3.7V12a1 1 0 0 1-1 1H2.5a1 1 0 0 1-1-1V2a1 1 0 0 1 1-1Z M7 1v3h3" />
		</svg>
	);
}

function Counts({ add, del }: { add: number; del: number }) {
	return (
		<span className="pc-counts">
			<span className="pc-add">+{add}</span>
			<span className="pc-del">−{del}</span>
		</span>
	);
}

function ChangesPane() {
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
		<div className="pc-pane-body">
			<div className="pc-segmented" role="radiogroup" aria-label="Changes">
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
				{files.length === 0 ? (
					<div className="pc-empty">
						<span className="pc-empty-mark" aria-hidden="true" />
						<p>No changes</p>
					</div>
				) : null}
				{files.map((file) => (
					<section key={file.path}>
						<div className="pc-file">
							<button type="button" className="pc-file-name" onClick={() => toggle(file.path)} aria-expanded={!collapsed.has(file.path)}>
								<Chevron open={!collapsed.has(file.path)} />
								<DocGlyph />
								<span className="pc-path">{file.path}</span>
							</button>
							<span className="pc-sys-add">+{count(file, "add")}</span>
							<span className="pc-sys-del">−{count(file, "del")}</span>
							<span className="pc-open" aria-hidden="true" />
						</div>
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

function PickerPane({ open }: { open: (page: number) => void }) {
	return (
		<div className="pc-pane-body pc-picker">
			<button type="button" className="pc-tile" onClick={() => open(1)}>
				<span className="pc-tile-name">Changes</span>
				<span className="pc-tile-count">2</span>
			</button>
			<div className="pc-tile">
				<span className="pc-tile-name">Terminal</span>
			</div>
			<div className="pc-tile">
				<span className="pc-tile-name">Files</span>
			</div>
		</div>
	);
}

export function PhoneCockpit({ scene, fig, title }: { scene: Scene; fig: string; title: string }) {
	const [turn, setTurn] = React.useState<Turn>(START[scene]);
	const [note, setNote] = React.useState(scene === "approve" ? NOTE.asking : scene === "diff" ? NOTE.doneDiff : NOTE.running);
	const [page, setPage] = React.useState(0);
	const [draft, setDraft] = React.useState("");
	const [sent, setSent] = React.useState<Sent[]>([]);
	const [offline, setOffline] = React.useState(false);
	const [seconds, setSeconds] = React.useState(scene === "steer" ? 12 : 41);
	const [focused, setFocused] = React.useState(false);
	const timers = React.useRef<number[]>([]);
	const swipe = React.useRef<{ x: number; y: number } | null>(null);
	const transcript = React.useRef<HTMLDivElement>(null);
	React.useEffect(() => {
		const node = transcript.current;
		if (node) node.scrollTop = node.scrollHeight;
	}, [turn, sent]);
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
			later(1800, () => finish("done", NOTE.allowed + " The turn finished, and its changes are one tap away."));
		} else {
			setNote(NOTE.denied);
			finish("denied", NOTE.denied);
		}
	};

	const confirm = (id: number) => {
		setSent((list) => list.map((item) => (item.id === id ? { ...item, confirmed: true } : item)));
		if (turnRef.current !== "editing") return;
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

	const reconnect = () => {
		setOffline(false);
		const pending = sent.filter((item) => !item.confirmed);
		pending.forEach((item, index) => later(700 + index * 300, () => confirm(item.id)));
		if (!pending.length) setNote(NOTE.running);
	};

	const remove = (id: number) => setSent((list) => list.filter((item) => item.id !== id));

	const stop = () => {
		setTurn("stopped");
		setNote(NOTE.stopped);
	};

	const go = (next: number) => {
		const target = Math.max(0, Math.min(PAGES - 1, next));
		setPage(target);
		setNote(target === 1 ? NOTE.diff : target === 2 ? NOTE.picker : scene === "diff" ? NOTE.doneDiff : note);
	};

	const reset = () => {
		timers.current.forEach((timer) => window.clearTimeout(timer));
		timers.current = [];
		setTurn(START[scene]);
		setNote(scene === "approve" ? NOTE.asking : scene === "diff" ? NOTE.doneDiff : NOTE.running);
		setPage(0);
		setDraft("");
		setSent([]);
		setOffline(false);
		setSeconds(scene === "steer" ? 12 : 41);
	};

	const stopsTurn = live && turn !== "asking" && !draft.trim();
	const sendEnabled = stopsTurn ? !offline : Boolean(draft.trim());
	const settled = turn === "done" || turn === "denied" || turn === "stopped";
	const expanded = focused || Boolean(draft);
	const confirmed = sent.filter((item) => item.confirmed);
	const pending = sent.filter((item) => !item.confirmed);

	return (
		<figure className="not-prose pc-figure" data-scene={scene}>
			<div className="pc-layout">
				<div className="pc-phone" aria-label={`${title}, an interactive phone`}>
					<div className="pc-screen">
						<div className="pc-status" aria-hidden="true">
							<span>9:41</span>
							<span className="pc-island" />
							<span className="pc-battery" />
						</div>
						<div className="pc-nav">
							<span className="pc-glass-button" aria-hidden="true">
								<span className="pc-back" />
							</span>
							<div className="pc-heading">
								<span className="pc-title">{TITLE}</span>
								<span className="pc-subtitle">api · fix-retry</span>
								<span className="pc-dots" aria-label={`Page ${page + 1} of ${PAGES}`}>
									{Array.from({ length: PAGES }, (_, index) => (
										<span key={index} data-current={index === page ? "" : undefined} />
									))}
								</span>
							</div>
							<span className="pc-glass-button" aria-hidden="true">
								<span className="pc-more" />
							</span>
						</div>
						<div
							className="pc-track"
							style={{ transform: `translateX(${-page * 100}%)` }}
							onPointerDown={(event) => {
								swipe.current = { x: event.clientX, y: event.clientY };
							}}
							onPointerUp={(event) => {
								const start = swipe.current;
								swipe.current = null;
								if (!start) return;
								const dx = event.clientX - start.x;
								if (Math.abs(dx) > 50 && Math.abs(dx) > Math.abs(event.clientY - start.y)) go(page + (dx < 0 ? 1 : -1));
							}}
						>
							<div className="pc-page" aria-hidden={page !== 0}>
								<div ref={transcript} className="pc-transcript">
									<p className="pc-user">{ASK_TEXT}</p>
									{settled ? (
										<p className="pc-summary">
											{SUMMARY[turn]} <Chevron />
										</p>
									) : (
										<div className="pc-live">
											<Spinner />
											<span className="pc-shimmer">{LIVE[turn]}</span>
											{turn === "asking" ? null : <span className="pc-elapsed">{seconds}s</span>}
										</div>
									)}
									{confirmed.map((item) => (
										<p key={item.id} className="pc-user">
											{item.text}
										</p>
									))}
									{REPLY[turn] ? <p className="pc-reply">{REPLY[turn]}</p> : null}
									{turn === "done" ? (
										<button type="button" className="pc-card" onClick={() => go(1)}>
											<span className="pc-card-head">
												<DocGlyph />
												<span className="pc-card-title">2 files changed</span>
												<Counts add={20} del={13} />
												<span className="pc-card-chevron">
													<Chevron />
												</span>
											</span>
											{DIFF.map((file) => (
												<span key={file.path} className="pc-card-row">
													<span>{file.path.split("/").at(-1)}</span>
													<Counts add={count(file, "add")} del={count(file, "del")} />
												</span>
											))}
										</button>
									) : null}
									{WORKED[turn] ? (
										<p className="pc-worked">
											<span className="pc-icon-copy" aria-hidden="true" />
											<span className="pc-icon-share" aria-hidden="true" />
											{WORKED[turn]}
										</p>
									) : null}
								</div>
								{turn === "asking" ? (
									<div className="pc-permission">
										<p className="pc-permission-title">
											<svg className="pc-hand" viewBox="0 0 14 16" aria-hidden="true">
												<path d="M4 8V3.2a1 1 0 0 1 2 0V7m0-.5V2a1 1 0 0 1 2 0v4.5m0-.3V2.8a1 1 0 0 1 2 0V8m0-2.2a1 1 0 0 1 2 0V10a5 5 0 0 1-5 5h-.4a4.6 4.6 0 0 1-3.7-1.9L1.2 10.4a1.1 1.1 0 0 1 1.7-1.3L4 10" />
											</svg>
											Approval required
										</p>
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
							</div>
							<div className="pc-page" aria-hidden={page !== 1}>
								<ChangesPane />
							</div>
							<div className="pc-page" aria-hidden={page !== 2}>
								<PickerPane open={go} />
							</div>
						</div>
						{page === 0 ? (
							<div className="pc-dock">
								{pending.length ? (
									<div className="pc-outbox">
										{pending.map((item) => (
											<div key={item.id} className="pc-outbox-card">
												<p>{item.text}</p>
												<p className="pc-outbox-meta">
													<span>Queued</span>
													<button type="button" onClick={() => remove(item.id)}>
														Remove
													</button>
												</p>
											</div>
										))}
									</div>
								) : null}
								{offline ? (
									<div className="pc-notice">
										<span className="pc-notice-dot" aria-hidden="true" />
										<span>Reconnecting… Messages you send will be delivered when it’s back.</span>
										<button type="button" onClick={reconnect}>
											Retry
										</button>
									</div>
								) : null}
								<form
									className="pc-composer"
									data-expanded={expanded ? "" : undefined}
									onSubmit={(event) => {
										event.preventDefault();
										if (stopsTurn) stop();
										else send();
									}}
								>
									<span className="pc-plus" aria-hidden="true" />
									<input
										value={draft}
										onChange={(event) => setDraft(event.target.value)}
										placeholder={offline ? "Reconnecting to your computer…" : live ? "Send a follow-up…" : "Ask Zuse"}
										aria-label="Message"
										onFocus={() => {
											setFocused(true);
											if (live && turn !== "asking") setNote(NOTE.running);
										}}
										onBlur={() => setFocused(false)}
									/>
									<span className="pc-model" aria-hidden="true">
										<span className="pc-model-mark" />
										Agent
									</span>
									<button type="submit" className="pc-send" data-enabled={sendEnabled ? "" : undefined} aria-label={stopsTurn ? "Stop response" : "Send message"} disabled={!sendEnabled}>
										<span className={stopsTurn ? "pc-stop-glyph" : "pc-send-glyph"} aria-hidden="true" />
									</button>
								</form>
							</div>
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
								<button type="button" aria-pressed={offline} onClick={() => (offline ? reconnect() : setOffline(true))}>
									{offline ? "Reconnect" : "Go offline"}
								</button>
							</>
						) : null}
						{scene === "diff" ? (
							<button type="button" onClick={() => go(page === 0 ? 1 : 0)}>
								{page === 0 ? "Swipe to Changes" : "Back to the chat"}
							</button>
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
