"use client";

import * as React from "react";
import { Plate } from "@/components/iso/react/draw";
import "@/components/iso/iso.css";
import "./racing-routes.css";

const ENDS = [
	{
		id: "unreachable",
		button: "Nothing answers",
		readout: "unreachable",
		what: "Every route timed out or closed before finishing a handshake.",
		then: "Nothing was wrong with the pairing, so trying again later is the right move. The computer may be asleep, offline or on a network this device can't see.",
	},
	{
		id: "authentication",
		button: "Wrong secret",
		readout: "rejected",
		what: "Something answered on a route, but the Noise handshake failed: whatever is there doesn't hold this pairing's secret.",
		then: "Retrying can't fix it. The pairing was revoked, expired or never valid, so the device has to be paired again. There is no fallback to an unencrypted connection.",
	},
	{
		id: "ended",
		button: "Host ends it",
		readout: "ended by host",
		what: "The computer sent an end frame inside the encrypted session, as it does on revocation or when the engine shuts down.",
		then: "The device stops at once instead of spending the resume window knocking on a door that has closed.",
	},
	{
		id: "lost",
		button: "Window runs out",
		readout: "lost",
		what: "The session dropped and no route came back within the 50-second resume window, or the host no longer held the session.",
		then: "Reads can be asked again. Actions that were in flight are reported as uncertain and never sent a second time.",
	},
] as const;

export function LinkEndings() {
	const [chosen, setChosen] = React.useState<(typeof ENDS)[number]["id"]>("unreachable");
	const end = ENDS.find((item) => item.id === chosen)!;
	return (
		<div className="iso rr-widget not-prose">
			<Plate fig="Fig 4" title="How a link ends" hint="Pick an ending" readout={end.readout}>
				<div className="rr-ends">
					<div className="rr-pick" role="radiogroup" aria-label="How the link ends">
						{ENDS.map((item) => (
							<button key={item.id} type="button" role="radio" aria-checked={item.id === chosen} onClick={() => setChosen(item.id)}>
								{item.button}
							</button>
						))}
					</div>
					<dl aria-live="polite">
						<dt>What the link saw</dt>
						<dd>{end.what}</dd>
						<dt>What happens next</dt>
						<dd>{end.then}</dd>
					</dl>
				</div>
			</Plate>
		</div>
	);
}
