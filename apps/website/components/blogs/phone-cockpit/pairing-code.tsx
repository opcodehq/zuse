"use client";

import * as React from "react";
import "./widgets.css";

const SIZE = 25;
const LIFETIME = 10 * 60;
const DEVICES = 32;

function modules() {
	let seed = 0x5eed1234;
	const random = () => {
		seed ^= seed << 13;
		seed ^= seed >>> 17;
		seed ^= seed << 5;
		return ((seed >>> 0) % 1000) / 1000;
	};
	const finder = (x: number, y: number) =>
		[
			[0, 0],
			[SIZE - 7, 0],
			[0, SIZE - 7],
		].some(([fx, fy]) => x >= fx - 1 && x <= fx + 7 && y >= fy - 1 && y <= fy + 7);
	const cells: Array<[number, number]> = [];
	for (let y = 0; y < SIZE; y++) for (let x = 0; x < SIZE; x++) if (!finder(x, y) && random() > 0.52) cells.push([x, y]);
	return cells;
}

const CELLS = modules();
const FINDERS = [
	[0, 0],
	[SIZE - 7, 0],
	[0, SIZE - 7],
];

type Code = "fresh" | "used" | "expired";
type Phone = "none" | "paired" | "refused" | "revoked";

const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;

export function PairingCode() {
	const [issued, setCode] = React.useState<Code>("fresh");
	const [phone, setPhone] = React.useState<Phone>("none");
	const [left, setLeft] = React.useState(LIFETIME);
	const [devices, setDevices] = React.useState(0);
	const [note, setNote] = React.useState("The computer shows a code. It holds a private link: a random pairing ID and a secret, and nothing else.");

	const code: Code = issued === "fresh" && left === 0 ? "expired" : issued;
	const shown = issued === "fresh" && code === "expired" ? "Expired. A code lasts ten minutes, so one left on a screen stops working on its own." : note;

	React.useEffect(() => {
		if (code !== "fresh") return;
		const timer = window.setInterval(() => setLeft((value) => Math.max(0, value - 1)), 1000);
		return () => window.clearInterval(timer);
	}, [code]);

	const scan = () => {
		if (code === "fresh") {
			setCode("used");
			setPhone("paired");
			setDevices((value) => Math.min(DEVICES, value + 1));
			setNote("Paired. The phone keeps the link; every connection after this starts with an encrypted handshake that only a holder of the secret can finish.");
		} else if (code === "used") {
			setNote("Nothing happens. A code works once, so a photo of it taken after you paired is worth nothing.");
		} else {
			setCode("expired");
			setNote("The code has expired. Make a new one on the computer.");
		}
	};

	const fresh = () => {
		setCode("fresh");
		setLeft(LIFETIME);
		setNote("A new code, good for ten minutes and one phone.");
	};

	const revoke = () => {
		if (phone !== "paired") return;
		setPhone("revoked");
		setDevices((value) => Math.max(0, value - 1));
		setNote("Revoked on the computer. The phone still holds the link, and it no longer opens anything.");
	};

	return (
		<div className="not-prose pw-panel" role="group" aria-label="Pairing with a code">
			<p className="editorial-label">Fig 6 · Pair once</p>
			<div className="pw-pair">
				<div className="pw-col">
					<p className="pw-head">On the computer · Connect device</p>
					<div className="pw-qr" data-state={code}>
						<svg viewBox={`-2 -2 ${SIZE + 4} ${SIZE + 4}`} role="img" aria-label="A sample pairing code">
							<rect x={-2} y={-2} width={SIZE + 4} height={SIZE + 4} className="pw-qr-paper" />
							{FINDERS.map(([x, y], index) => (
								<g key={index}>
									<rect x={x} y={y} width={7} height={7} className="pw-qr-ink" />
									<rect x={x + 1} y={y + 1} width={5} height={5} className="pw-qr-paper" />
									<rect x={x + 2} y={y + 2} width={3} height={3} className="pw-qr-ink" />
								</g>
							))}
							{CELLS.map(([x, y]) => (
								<rect key={`${x}-${y}`} x={x} y={y} width={1.02} height={1.02} className="pw-qr-ink" />
							))}
						</svg>
						{code !== "fresh" ? <span className="pw-qr-stamp">{code === "used" ? "Used" : "Expired"}</span> : null}
					</div>
					<p className="pw-readout">
						{code === "fresh" ? `Expires in ${clock(left)}` : code === "used" ? "Used once" : "Expired"} · {devices} of {DEVICES} devices
					</p>
				</div>
				<div className="pw-col">
					<p className="pw-head">On the phone</p>
					<div className="pw-device" data-state={phone}>
						{phone === "none" ? "Add computer" : phone === "paired" ? "Connected · Local network · 12 ms" : "Pairing revoked by the computer"}
					</div>
					<p className="pw-note" aria-live="polite">
						{shown}
					</p>
					<div className="pw-actions">
						<button type="button" onClick={scan}>
							Scan
						</button>
						<button type="button" onClick={() => setLeft(0)} disabled={code !== "fresh"}>
							Wait ten minutes
						</button>
						<button type="button" onClick={fresh} disabled={code === "fresh"}>
							New code
						</button>
						<button type="button" onClick={revoke} disabled={phone !== "paired"}>
							Revoke
						</button>
					</div>
				</div>
			</div>
		</div>
	);
}
