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
	const pad = 3.2;
	return (
		<g transform={transform}>
			<rect className="cp-bezel" x={-1.2} y={-1.2} width={width + 2.4} height={height + 2.4} rx={4.4} />
			<rect className="cp-phone-glass" x={0} y={0} width={width} height={height} rx={3.6} />
			<text className="cp-ui" x={pad} y={4.2} fontSize={2.4}>
				9:41
			</text>
			<text className="cp-ui cp-strong" x={pad} y={11.6} fontSize={3.3}>
				Fix the flaky retry
			</text>
			<text className="cp-ui" x={pad} y={15.8} fontSize={2.2}>
				Connected · Local network · 12 ms
			</text>
			<rect className="cp-rule" x={pad} y={18.4} width={width - pad * 2} height={0.35} />
			<circle className="cp-spin" data-spin="" cx={pad + 1.4} cy={26.2} r={1.3} />
			<text className="cp-ui cp-strong" data-slot="phone-live" x={pad + 4.2} y={27.4} fontSize={3.3}>
				{still.phone}
			</text>
			<g data-card="" style={{ opacity: still.lamp === "needs" ? 1 : 0 }}>
				<rect className="cp-card" x={pad - 0.6} y={34} width={width - pad * 2 + 1.2} height={29} rx={2.2} />
				<text className="cp-ui cp-strong" x={pad + 2} y={40.4} fontSize={2.8}>
					Approval required
				</text>
				<text className="cp-ui cp-mono" x={pad + 2} y={45.6} fontSize={2.2}>
					{COMMAND}
				</text>
				<g data-key="deny">
					<rect className="cp-button" x={pad + 2} y={51} width={15.5} height={7.6} rx={1.8} />
					<text className="cp-ui cp-deny" x={pad + 9.75} y={55.8} fontSize={2.6} textAnchor="middle">
						Deny
					</text>
				</g>
				<g data-key="allow">
					<rect className="cp-button" data-primary="" x={pad + 19.5} y={51} width={21.4} height={7.6} rx={1.8} />
					<text className="cp-ui cp-on-primary" x={pad + 30.2} y={55.8} fontSize={2.6} textAnchor="middle">
						Allow once
					</text>
				</g>
			</g>
			<g data-summary="" style={{ opacity: still.lamp === "done" ? 1 : 0 }}>
				<text className="cp-ui" data-slot="phone-summary" x={pad} y={38} fontSize={2.6}>
					Read 2 files · edited client.ts
				</text>
				<text className="cp-ui" x={pad} y={43} fontSize={2.6}>
					ran 1 command
				</text>
				<rect className="cp-card" x={pad - 0.6} y={48} width={width - pad * 2 + 1.2} height={10} rx={2.2} />
				<text className="cp-ui cp-mono" data-slot="phone-changes" x={pad + 2} y={54.2} fontSize={2.4}>
					+20 −13 · 2 files
				</text>
			</g>
			<rect className="cp-composer" x={pad - 0.6} y={height - 11} width={width - pad * 2 + 1.2} height={7.4} rx={3.7} />
			<text className="cp-ui" x={pad + 2} y={height - 6.4} fontSize={2.4}>
				Send a follow-up…
			</text>
			<circle className="cp-send" cx={width - pad - 3.4} cy={height - 7.3} r={2.6} />
			<rect className="cp-home" x={width / 2 - 8} y={height - 2} width={16} height={0.8} rx={0.4} />
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
