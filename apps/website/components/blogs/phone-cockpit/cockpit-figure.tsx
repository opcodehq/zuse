import { Dots, FaceText, Line, Plate, Solid } from "@/components/iso/draw";
import "@/components/iso/iso.css";
import { BASE, BUS_ART, CONSOLE, CRADLE_ART, EX, HEIGHT, LIFT, LOG, PHONE_ART, RIDER, RULER_STEP, UP, WIDTH } from "./cockpit-geometry";
import { COMMAND, HERO, PHASES } from "./cockpit-model";
import { CockpitLive } from "./cockpit-live";
import "./cockpit.css";

const still = PHASES[HERO.still];

function ConsoleScreen() {
	const { transform, width, height } = CONSOLE.screen;
	return (
		<g transform={transform}>
			<rect className="cp-bezel" x={-2.4} y={-2.4} width={width + 4.8} height={height + 4.8} rx={2.4} />
			<rect className="cp-glass" x={0} y={0} width={width} height={height} rx={1} />
			<path className="cp-wall" d={`M0 0H${width}L${width - 1.4} 1.4H1.4V${height - 1.4}L0 ${height}Z`} />
			<g>
				{LOG.map((line, index) => (
					<rect key={index} className="cp-log" data-log={index} data-shown={index < still.log ? "" : undefined} data-ask={index === still.log - 1 ? "" : undefined} x={6} y={line.y} width={line.width} height={1.5} rx={0.4} />
				))}
				<rect className="cp-cursor" data-cursor="" x={6} y={LOG[Math.min(still.log, LOG.length - 1)].y - 0.4} width={2.2} height={2.3} />
			</g>
		</g>
	);
}

function PhoneScreen() {
	const { transform, width, height } = PHONE_ART.screen;
	const pad = 3;
	const mid = width / 2;
	return (
		<g transform={transform}>
			<rect className="cp-bezel" x={-1.2} y={-1.2} width={width + 2.4} height={height + 2.4} rx={4.4} />
			<rect className="cp-phone-glass" x={0} y={0} width={width} height={height} rx={3.6} />
			<text className="cp-ui cp-strong" x={pad + 1} y={3.6} fontSize={2.1}>
				9:41
			</text>
			<rect className="cp-island" x={mid - 5} y={1.4} width={10} height={2.8} rx={1.4} />
			<circle className="cp-glass-button" cx={pad + 2.2} cy={9.4} r={2.2} />
			<circle className="cp-glass-button" cx={width - pad - 2.2} cy={9.4} r={2.2} />
			<text className="cp-ui cp-strong" x={mid} y={8.6} fontSize={2.5} textAnchor="middle">
				Fix the flaky retry
			</text>
			<text className="cp-ui" x={mid} y={11.4} fontSize={1.7} textAnchor="middle">
				api · fix-retry
			</text>
			<circle className="cp-dot" data-current="" cx={mid - 1.6} cy={13.2} r={0.42} />
			<circle className="cp-dot" cx={mid} cy={13.2} r={0.34} />
			<circle className="cp-dot" cx={mid + 1.6} cy={13.2} r={0.34} />
			<rect className="cp-bubble" x={13} y={16} width={width - pad - 13} height={9.6} rx={2.4} />
			<text className="cp-ui cp-bubble-text" x={15} y={19.8} fontSize={2.2}>
				The retry test fails
			</text>
			<text className="cp-ui cp-bubble-text" x={15} y={23} fontSize={2.2}>
				one run in five. Fix it.
			</text>
			<circle className="cp-spin" data-spin="" cx={pad + 1.2} cy={30} r={1.1} />
			<text className="cp-ui" data-slot="phone-live" x={pad + 3.6} y={30.8} fontSize={2.3}>
				{still.phone}
			</text>
			<g data-summary="" style={{ opacity: still.lamp === "done" ? 1 : 0 }}>
				<rect className="cp-reply" x={pad} y={35} width={width - pad * 2} height={1.1} rx={0.4} />
				<rect className="cp-reply" x={pad} y={37.8} width={width - pad * 2 - 4} height={1.1} rx={0.4} />
				<rect className="cp-reply" x={pad} y={40.6} width={width - pad * 2 - 12} height={1.1} rx={0.4} />
				<rect className="cp-changes" x={pad} y={44} width={width - pad * 2} height={7} rx={1.6} />
				<text className="cp-ui cp-strong" x={pad + 2} y={48.3} fontSize={2.1}>
					2 files changed
				</text>
				<text className="cp-ui cp-mono cp-counts" data-slot="phone-changes" x={width - pad - 2} y={48.3} fontSize={2} textAnchor="end">
					+20 −13
				</text>
				<text className="cp-ui" x={pad} y={55.2} fontSize={1.8}>
					Worked for 1m 5s · 9:41 AM
				</text>
			</g>
			<g data-card="" style={{ opacity: still.lamp === "needs" ? 1 : 0 }}>
				<rect className="cp-panel" x={0} y={height - 33} width={width} height={33} />
				<rect className="cp-rule" x={0} y={height - 33} width={width} height={0.3} />
				<text className="cp-ui cp-strong" x={pad + 0.4} y={height - 30} fontSize={2.4}>
					Approval required
				</text>
				<text className="cp-ui cp-mono" x={pad + 0.4} y={height - 26.4} fontSize={1.85}>
					{COMMAND}
				</text>
				<g data-key="deny">
					<rect className="cp-hit" x={pad} y={height - 24.8} width={14} height={6} />
					<text className="cp-ui cp-deny" x={pad + 0.4} y={height - 20.8} fontSize={2.4}>
						Deny
					</text>
				</g>
				<g data-key="allow">
					<rect className="cp-allow" x={width - pad - 17} y={height - 24.8} width={17} height={6} rx={3} />
					<text className="cp-ui cp-on-accent" x={width - pad - 8.5} y={height - 20.9} fontSize={2.3} textAnchor="middle">
						Allow once
					</text>
				</g>
			</g>
			<rect className="cp-composer" x={pad - 1.4} y={height - 11.4} width={width - pad * 2 + 2.8} height={7.6} rx={3.8} />
			<text className="cp-ui cp-plus" x={pad + 1.4} y={height - 6.6} fontSize={3.4} textAnchor="middle">
				+
			</text>
			<text className="cp-ui cp-faint" x={pad + 4.2} y={height - 6.8} fontSize={2.2}>
				Send a follow-up…
			</text>
			<circle className="cp-send" cx={width - pad - 2} cy={height - 7.6} r={2.6} />
			<rect className="cp-home" x={mid - 8} y={height - 2} width={16} height={0.8} rx={0.4} />
		</g>
	);
}

const ART = (
	<svg key="art" className="iso-svg" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label="A bench with a large console on the left that shows every line an agent prints, and a handheld unit standing in a cradle on the right that shows the same turn as one status line, a lamp and two keys: Deny and Allow once." focusable="false">
		<path className="iso-halo" d={BASE.halo} />
		{BASE.feet.map((foot, index) => (
			<g key={index}>
				<Solid paths={foot.flange} tone="lo" />
				<Solid paths={foot.foot} tone="mid" />
			</g>
		))}
		<Solid paths={BASE.slab} tone="mid" />
		<Line d={BASE.groove} tone="lo" />
		<Line d={BASE.screwRings} tone="lo" />
		<Dots points={BASE.screws} size={0.5} tone="mid" />

		<g>
			<Solid paths={CONSOLE.housing} tone="mid" inner={<Line d={CONSOLE.housingSeam} tone="faint" />} />
			<Solid paths={CONSOLE.lid} tone="lo" />
			<Line d={CONSOLE.vents} tone="lo" />
			<Solid paths={CONSOLE.handle} tone="mid" />
			<Dots points={CONSOLE.faceScrews} size={0.5} tone="mid" />
			<ConsoleScreen />
			<FaceText transform={CONSOLE.badge.transform} size={3} tone="lo">
				ZUSE
			</FaceText>
		</g>

		<g>
			<Solid paths={CONSOLE.deck} tone="mid" inner={<Line d={CONSOLE.deckSeam} tone="faint" />} />
			<Line d={CONSOLE.well} tone="lo" />
			{CONSOLE.keys.map((key, index) => (
				<Solid key={index} paths={key.paths} tone="mid" />
			))}
			{CONSOLE.lamps.map((lamp) => (
				<g key={lamp.index}>
					<Solid paths={lamp.collar} tone="lo" />
					<g data-lamp={lamp.index}>
						<Solid paths={lamp.lamp} tone="mid" lit={(still.lamp === "working" ? 0 : still.lamp === "needs" ? 1 : 2) === lamp.index} />
					</g>
					<Line d={lamp.ring} tone="faint" />
				</g>
			))}
		</g>

		<g>
			<Solid paths={BUS_ART.ribbon} tone="lo" flat />
			<Line d={BUS_ART.traces} tone="faint" />
		</g>

		<g>
			<Solid paths={CRADLE_ART.base} tone="mid" inner={<Line d={CRADLE_ART.baseSeam} tone="faint" />} />
			<Dots points={CRADLE_ART.screws} size={0.5} tone="mid" />
			<Solid paths={CRADLE_ART.rest} tone="mid" inner={<Line d={CRADLE_ART.restSeam} tone="faint" />} />
			<Solid paths={CRADLE_ART.pin} tone="lo" />
			<g data-phone="">
				<path className="iso-halo cp-lift-halo" d={PHONE_ART.halo} data-lift-halo="" />
				<g data-phone-body="">
					<Solid paths={PHONE_ART.body} tone="hi" inner={<Line d={PHONE_ART.band} tone="faint" />} />
					<Solid paths={PHONE_ART.button} tone="mid" />
					<PhoneScreen />
				</g>
			</g>
			<Solid paths={CRADLE_ART.lip} tone="mid" />
			{CRADLE_ART.pads.map((pad, index) => (
				<Solid key={index} paths={pad} tone="lo" />
			))}
			<Solid paths={CRADLE_ART.lampCollar} tone="lo" />
			<g data-dock-lamp="">
				<Solid paths={CRADLE_ART.lamp} tone="mid" lit={still.lamp === "needs"} />
			</g>
			<Line d={CRADLE_ART.lampRing} tone="faint" />
		</g>

		<Solid paths={BASE.label} tone="lo" crease="none" />
		<Line d={BASE.labelGroove} tone="faint" />
		<Line d={BASE.labelRules} tone="lo" free />
		<Dots points={BASE.labelScrews} size={0.45} tone="mid" />
		<Solid paths={BASE.strip} tone="lo" crease="none" />
		<Line d={BASE.ticks.minor} tone="lo" />
		<Line d={BASE.ticks.major} tone="mid" />
		<Line d={BASE.stations} tone="mid" />
		<g data-rider="" transform={`translate(${(EX[0] * RULER_STEP * HERO.still).toFixed(2)} ${(EX[1] * RULER_STEP * HERO.still).toFixed(2)})`}>
			<Solid paths={RIDER.body} tone="hi" />
			<Line d={RIDER.index} tone="lit" />
		</g>
	</svg>
);

export function CockpitFigure() {
	return (
		<div className="iso not-prose cp-figure">
			<Plate
				fig="Fig 1"
				title="One turn, two instruments"
				hint="Tap, or keys A · D · L"
				readout={still.readout}
				keys={[
					{ mark: "lit", label: "A lamp that is on" },
					{ mark: "raised", label: "Console · cradle · handheld" },
				]}
				caption={HERO.caption}
			>
				<CockpitLive ex={EX} up={UP} step={RULER_STEP} lift={LIFT}>
					{ART}
				</CockpitLive>
			</Plate>
		</div>
	);
}
