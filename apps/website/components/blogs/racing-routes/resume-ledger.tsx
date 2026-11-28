"use client";

import * as React from "react";
import { Plate } from "@/components/iso/react/draw";
import "@/components/iso/iso.css";
import "./racing-routes.css";

interface Ledger {
	sent: number;
	hostHas: number;
	acked: number;
	dropped: boolean;
	resent: number[];
}

const START: Ledger = { sent: 9, hostHas: 9, acked: 6, dropped: false, resent: [] };
const CAP = 16;

export function ResumeLedger() {
	const [ledger, setLedger] = React.useState<Ledger>(START);
	const send = () =>
		setLedger((state) => {
			if (state.sent >= CAP) return state;
			const sent = state.sent + 1;
			if (state.dropped) return { ...state, sent };
			return { ...state, sent, hostHas: sent, acked: sent - 3 >= state.acked + 4 ? sent - 3 : state.acked, resent: [] };
		});
	const drop = () => setLedger((state) => (state.dropped ? state : { ...state, dropped: true, hostHas: Math.max(state.acked, state.sent - 2), resent: [] }));
	const reattach = () =>
		setLedger((state) => {
			if (!state.dropped) return state;
			const resent = Array.from({ length: state.sent - state.hostHas }, (_, index) => state.hostHas + index + 1);
			return { sent: state.sent, hostHas: state.sent, acked: state.hostHas, dropped: false, resent };
		});
	const reset = () => setLedger(START);
	const readout = ledger.dropped
		? "socket lost · keeping " + (ledger.sent - ledger.acked) + " unacknowledged"
		: ledger.resent.length
			? "host had " + (ledger.resent[0]! - 1) + " · resent " + ledger.resent.join(", ")
			: "host has " + ledger.hostHas + " · acked " + ledger.acked;
	return (
		<div className="iso rr-widget not-prose">
			<Plate fig="Fig 3" title="Resume ledger" hint="Send · drop · reattach" readout={readout}>
				<div className="rr-ledger">
					<ol aria-label="Messages from the phone">
						{Array.from({ length: CAP }, (_, index) => {
							const n = index + 1;
							const state =
								n > ledger.sent ? "empty" : ledger.resent.includes(n) ? "resent" : n <= ledger.acked ? "acked" : n <= ledger.hostHas ? "held" : ledger.dropped ? "lost" : "held";
							return (
								<li key={n} data-state={state}>
									{n}
								</li>
							);
						})}
					</ol>
					<div className="rr-legend">
						<span data-state="acked">acknowledged, freed</span>
						<span data-state="held">delivered, still kept</span>
						<span data-state="lost">lost with the socket</span>
						<span data-state="resent">resent on reattach</span>
					</div>
					<div className="rr-actions">
						<button type="button" onClick={send} disabled={ledger.sent >= CAP}>
							Send a message
						</button>
						<button type="button" onClick={drop} disabled={ledger.dropped}>
							Drop the socket
						</button>
						<button type="button" onClick={reattach} disabled={!ledger.dropped}>
							Reattach
						</button>
						<button type="button" onClick={reset}>
							Reset
						</button>
					</div>
				</div>
			</Plate>
		</div>
	);
}
