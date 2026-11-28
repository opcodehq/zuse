"use client";

import * as React from "react";
import { Plate } from "@/components/iso/draw";
import "@/components/iso/iso.css";
import { LEVELS, QUEUE, REACHES, VERDICT_LABEL, matchOf, noteOf, sensitivePart, verdictOf, type Rule, type Scope } from "./model";
import "./permissions.css";

function Toggle<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: { id: T; label: string }[]; onChange: (value: T) => void }) {
	return (
		<div className="ap-toggle" role="radiogroup" aria-label={label}>
			<span className="ap-label">{label}</span>
			{options.map((option) => (
				<button key={option.id} type="button" role="radio" aria-checked={value === option.id} onClick={() => onChange(option.id)}>
					{option.label}
				</button>
			))}
		</div>
	);
}

type AgentKind = "zuse" | "reviewer";

export function AccessSlider() {
	const [index, setIndex] = React.useState(1);
	const [agent, setAgent] = React.useState<AgentKind>("zuse");
	const level = LEVELS[index];
	const reviewer = agent === "reviewer";
	const rows = REACHES.map((reach) => ({ reach, verdict: verdictOf(level.id, reach.id, reviewer), note: noteOf(level.id, reach.id, reviewer) }));
	const free = rows.filter((row) => row.verdict === "runs").length;
	const readout = level.id === "auto" && !reviewer ? "Not available for this agent" : free + " of " + rows.length + " run without a card";
	return (
		<div className="iso ap-widget ap-slider not-prose">
			<Plate fig="Fig 2" title="What each level lets through" hint="Drag the slider" readout={readout} caption="Rows follow Zuse's own agent, read from its tool policy. Approve for me hands each decision to the agent's own reviewer, so it only exists on agents that ship one. A sensitive path asks at every level.">
				<div className="ap-panel">
					<input className="ap-range" type="range" min={0} max={LEVELS.length - 1} step={1} value={index} onChange={(event) => setIndex(Number(event.target.value))} aria-label="Access level" aria-valuetext={level.label} />
					<div className="ap-levels" role="radiogroup" aria-label="Access level">
						{LEVELS.map((entry, at) => (
							<button key={entry.id} type="button" role="radio" aria-checked={at === index} onClick={() => setIndex(at)}>
								{entry.label}
							</button>
						))}
					</div>
					<Toggle
						label="Agent"
						value={agent}
						options={[
							{ id: "zuse", label: "Zuse's agent" },
							{ id: "reviewer", label: "Ships its own reviewer" },
						]}
						onChange={setAgent}
					/>
					<div className="ap-rows" aria-live="polite">
						{rows.map((row) => (
							<div key={row.reach.id} className="ap-row" data-verdict={row.verdict}>
								<p className="ap-strong">{row.reach.label}</p>
								<p className="ap-verdict">
									<span className="ap-mark" data-verdict={row.verdict} aria-hidden="true" />
									{VERDICT_LABEL[row.verdict]}
								</p>
								<p>{row.note}</p>
							</div>
						))}
					</div>
					{reviewer && level.id !== "auto" ? <p>Outside Approve for me, other agents map each level onto their own permission modes and sandboxes. One, for example, runs Ask for approval inside a workspace-write sandbox with the network restricted.</p> : null}
				</div>
			</Plate>
		</div>
	);
}

const SAMPLES = ["apps/api/.env.local", "src/env.ts", "~/.ssh/config", "deploy/tls.key", "config/credentials.json", ".envrc"];

export function PathCheck() {
	const [path, setPath] = React.useState(SAMPLES[0]);
	const hit = sensitivePart(path);
	const parts = path.split(/([\\/]+)/);
	return (
		<div className="iso ap-widget ap-paths not-prose">
			<Plate fig="Fig 3" title="Does it always ask?" hint="Type a path" readout={path.trim() ? (hit ? "Always asks" : "Follows the level") : "Waiting for a path"} caption="The same rule the engine runs: any part of the path named .env or .env.*, .aws, .ssh, .netrc, .pgpass, credentials or credentials.*, an SSH key name (id_rsa, id_ed25519, id_ecdsa, id_dsa, or its .pub), or a name ending in .pem, .key, .p12 or .pfx.">
				<div className="ap-panel">
					<input className="ap-input" value={path} onChange={(event) => setPath(event.target.value)} aria-label="File path" spellCheck={false} autoComplete="off" />
					<div className="ap-samples">
						{SAMPLES.map((sample) => (
							<button key={sample} type="button" onClick={() => setPath(sample)}>
								{sample}
							</button>
						))}
					</div>
					<p className="ap-path" aria-hidden="true">
						{parts.map((part, at) => (part && part === hit ? <mark key={at}>{part}</mark> : <span key={at}>{part}</span>))}
					</p>
					<p className="ap-strong" aria-live="polite">
						{!path.trim() ? "Type a path to check it." : hit ? hit + " is sensitive. Reading or writing it asks you at every access level, Full access included, and the card can't be saved as a rule." : "Nothing here is sensitive, so the access level decides."}
					</p>
				</div>
			</Plate>
		</div>
	);
}

const SCOPE_LABEL: Record<Scope, string> = { chat: "this chat", project: "this project" };

interface Entry {
	id: string;
	text: string;
}

export function ApprovalCards() {
	const [at, setAt] = React.useState(0);
	const [rules, setRules] = React.useState<Rule[]>([]);
	const [log, setLog] = React.useState<Entry[]>([]);
	const request = QUEUE[at];
	const matched = request ? matchOf(request, rules) : null;
	const asked = log.filter((entry) => !entry.text.startsWith("Matched")).length;

	const answer = (choice: "once" | Scope | "deny") => {
		if (!request) return;
		const text = choice === "deny" ? "Denied: control goes back to the agent to explain or try another way." : choice === "once" ? "Allowed once." : "Allowed, and saved for " + SCOPE_LABEL[choice] + ".";
		if (choice === "chat" || choice === "project") setRules((list) => [...list, { tool: request.tool, resource: request.resource, scope: choice }]);
		setLog((list) => [...list, { id: request.id, text }]);
		setAt((value) => value + 1);
	};

	const pass = () => {
		if (!request || !matched) return;
		setLog((list) => [...list, { id: request.id, text: "Matched your " + SCOPE_LABEL[matched.scope] + " rule, so no card." }]);
		setAt((value) => value + 1);
	};

	const reset = () => {
		setAt(0);
		setRules([]);
		setLog([]);
	};

	return (
		<div className="iso ap-widget ap-cards not-prose">
			<Plate fig="Fig 4" title="The card" hint="Answer the cards" readout={request ? "Request " + (at + 1) + " of " + QUEUE.length + " · " + rules.length + (rules.length === 1 ? " rule" : " rules") : asked + " of " + QUEUE.length + " needed you"} caption="A session in Ask for approval. Saving a rule binds the exact tool, the exact command or file and the same checkout. A chat rule is checked before a project rule. Sensitive paths, sign-ins and leaving Plan mode never offer to save.">
				<div className="ap-panel">
					{request ? (
						matched ? (
							<div className="ap-auto" aria-live="polite">
								<p>
									{request.verb}: <code>{request.resource}</code>
								</p>
								<p>Matches your {SCOPE_LABEL[matched.scope]} rule exactly, so it runs without a card.</p>
								<div className="ap-answers">
									<button type="button" data-primary="" onClick={pass}>
										Next request
									</button>
								</div>
							</div>
						) : (
							<div className="ap-card" role="group" aria-label={"Permission: " + request.verb}>
								<p className="ap-card-head">
									<span>Permission</span>
									<span>{request.verb}</span>
								</p>
								<code className="ap-resource">{request.resource}</code>
								{request.forced ? <p>{request.forced}</p> : null}
								<div className="ap-answers">
									<button type="button" data-primary="" onClick={() => answer("once")}>
										Allow once
									</button>
									{request.forced ? null : (
										<>
											<button type="button" onClick={() => answer("chat")}>
												Always allow in this chat
											</button>
											<button type="button" onClick={() => answer("project")}>
												Always allow in project
											</button>
										</>
									)}
									<button type="button" onClick={() => answer("deny")}>
										Deny
									</button>
								</div>
							</div>
						)
					) : (
						<div className="ap-auto">
							<p>
								{asked} of {QUEUE.length} requests needed you. Try again and save the first two as rules: the edit comes back without a card, and the --watch run still asks, because it isn&apos;t the same command.
							</p>
							<div className="ap-answers">
								<button type="button" data-primary="" onClick={reset}>
									Start over
								</button>
							</div>
						</div>
					)}
					<div className="ap-ledger">
						<p className="ap-label">Saved rules</p>
						{rules.length ? (
							<ul className="ap-ledger">
								{rules.map((rule, index) => (
									<li key={index}>
										<code>{rule.resource}</code>
										<span>{SCOPE_LABEL[rule.scope]}</span>
									</li>
								))}
							</ul>
						) : (
							<p>None yet.</p>
						)}
					</div>
					{log.length ? (
						<ol className="ap-ledger" aria-label="What happened">
							{log.map((entry, index) => (
								<li key={index}>
									<span>{QUEUE.find((item) => item.id === entry.id)?.resource}</span>
									<span>{entry.text}</span>
								</li>
							))}
						</ol>
					) : null}
				</div>
			</Plate>
		</div>
	);
}
