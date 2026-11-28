import { Dots, Line, Plate, Solid } from "@/components/iso/draw";
import "@/components/iso/iso.css";
import { BANK_ART, BASE, CLOCK_ART, DAY_STEP, DIAL_BEZEL, DIAL_GLASS, DIAL_RIM, DIAL_TICKS, EX, HEIGHT, METER_ART, RIDER, SHAFTS, TAXI_ART, WIDTH } from "./meters-geometry";
import { MetersLive } from "./meters-live";
import { HERO } from "./hero";
import "./meters.css";

function Readout({ transform, width, height, slot, size, initial }: { transform: string; width: number; height: number; slot: string; size: number; initial: string }) {
	return (
		<g transform={transform}>
			<rect className="svm-bezel" x={-1.8} y={-1.8} width={width + 3.6} height={height + 3.6} rx={1.8} />
			<rect className="svm-glass" x={0} y={0} width={width} height={height} rx={0.8} />
			<path className="svm-wall" d={`M0 0H${width}L${width - 1.2} 1.2H1.2V${height - 1.2}L0 ${height}Z`} />
			<clipPath id={`svm-clip-${slot}`}>
				<rect x={0.6} y={0.6} width={width - 1.2} height={height - 1.2} />
			</clipPath>
			<g clipPath={`url(#svm-clip-${slot})`}>
				<text className="svm-digits" data-slot={slot} x={width - 2.6} y={height / 2 + size * 0.36} fontSize={size} textAnchor="end">
					{initial}
				</text>
			</g>
		</g>
	);
}

const ART = (
	<svg key="art" className="iso-svg" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label="A bench with three meters driven by one bank of agent lamps: a taxi meter that charges per token, a flat-rate meter with a fixed dial, and a time clock that stamps machine minutes while a cloud computer is awake." focusable="false">
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
		<Solid paths={BASE.strip} tone="lo" crease="none" />
		<Line d={BASE.ticks.minor} tone="lo" />
		<Line d={BASE.ticks.major} tone="mid" />
		<Solid paths={BASE.label} tone="lo" crease="none" />
		<Line d={BASE.labelGroove} tone="faint" />
		<Line d={BASE.labelRules} tone="lo" free />
		<Dots points={BASE.labelScrews} size={0.45} tone="mid" />

		<g>
			<Solid paths={TAXI_ART.plinth} tone="lo" />
			<Solid paths={TAXI_ART.body} tone="mid" inner={<Line d={TAXI_ART.seam} tone="faint" />} />
			<Dots points={TAXI_ART.screws} size={0.5} tone="mid" />
			<Readout {...TAXI_ART.window} slot="taxi" size={10.5} initial={HERO.taxi} />
			<Solid paths={TAXI_ART.roofBase} tone="lo" crease="none" />
			<g data-light="roof">
				<Solid paths={TAXI_ART.roof} tone="hi" />
			</g>
			{TAXI_ART.buttons.map((paths, index) => (
				<Solid key={index} paths={paths} tone="mid" />
			))}
			<Solid paths={TAXI_ART.flagPost} tone="mid" />
			<g data-flag="up" style={{ opacity: HERO.agents ? 0 : 1 }}>
				<Solid paths={TAXI_ART.flagUp} tone="hi" />
			</g>
			<g data-flag="down" style={{ opacity: HERO.agents ? 1 : 0 }}>
				<Solid paths={TAXI_ART.flagDown} tone="hi" />
			</g>
		</g>

		<g>
			{SHAFTS.left.map((paths, index) => (
				<Solid key={index} paths={paths} tone={index ? "mid" : "lo"} />
			))}
			<Dots points={SHAFTS.leftScrews} size={0.45} tone="mid" />
		</g>

		<g>
			<Solid paths={BANK_ART.body} tone="mid" inner={<Line d={BANK_ART.seam} tone="faint" />} />
			<Dots points={BANK_ART.screws} size={0.5} tone="mid" />
			<g transform={BANK_ART.vents.transform}>
				{Array.from({ length: 3 }, (_, index) => (
					<rect key={index} className="svm-slot" x={76 + index * 4.6} y={6} width={2.4} height={10} rx={1.2} />
				))}
			</g>
			{BANK_ART.lamps.map((lamp) => (
				<g key={lamp.index}>
					<Solid paths={lamp.collar} tone="lo" />
					<g data-lamp={lamp.index}>
						<Solid paths={lamp.lamp} tone="mid" lit={lamp.index < HERO.agents} />
					</g>
					<Line d={lamp.ring} tone="faint" />
				</g>
			))}
		</g>

		<g>
			{SHAFTS.front.map((paths, index) => (
				<Solid key={index} paths={paths} tone="mid" />
			))}
		</g>

		<g>
			{SHAFTS.right.map((paths, index) => (
				<Solid key={index} paths={paths} tone={index ? "mid" : "lo"} />
			))}
			<Dots points={SHAFTS.rightScrews} size={0.45} tone="mid" />
		</g>

		<g>
			<Solid paths={CLOCK_ART.body} tone="mid" inner={<Line d={CLOCK_ART.seam} tone="faint" />} />
			<Line d={CLOCK_ART.slot} tone="lo" />
			<Readout {...CLOCK_ART.window} slot="clock" size={6.6} initial={HERO.clock} />
			<Solid paths={CLOCK_ART.lampCollar} tone="lo" />
			<g data-light="clock">
				<Solid paths={CLOCK_ART.lamp} tone="mid" />
			</g>
			<Solid paths={CLOCK_ART.leverBoss} tone="lo" />
			<Solid paths={CLOCK_ART.lever} tone="mid" />
			<Solid paths={CLOCK_ART.knob} tone="mid" />
		</g>

		<g>
			<Solid paths={METER_ART.plinth} tone="lo" inner={<Line d={METER_ART.plinthSeam} tone="faint" />} />
			<Solid paths={METER_ART.body} tone="mid" />
			<Dots points={METER_ART.screws} size={0.5} tone="mid" />
			<g transform={METER_ART.dial.transform}>
				<path className="svm-bezel" d={DIAL_BEZEL} />
				<path className="svm-dial" d={DIAL_GLASS} />
				<Line d={DIAL_RIM} tone="faint" />
				{DIAL_TICKS.map((tick, index) => (
					<Line key={index} d={tick.d} tone={tick.major ? "mid" : "lo"} />
				))}
				<path className="svm-needle" d="M-0.8 0.6L-8.2 -6.4L-7.6 -7L0.6 -0.8Z" />
				<circle className="svm-hub" r={1.6} />
			</g>
			<Readout {...METER_ART.window} slot="plan" size={6} initial={HERO.plan} />
			<Solid paths={METER_ART.lid} tone="mid" />
			{METER_ART.lugs.map((paths, index) => (
				<Solid key={index} paths={paths} tone="mid" />
			))}
			<Solid paths={METER_ART.seal} tone="hi" />
		</g>

		<g data-rider="">
			<Solid paths={RIDER.body} tone="hi" />
			<Line d={RIDER.index} tone="lit" />
		</g>
	</svg>
);

export function MetersFigure() {
	return (
		<div className="iso not-prose svm-figure">
		<Plate
			fig="Fig 1"
			title="One workload, three meters"
			hint="Tap or ← → to run agents"
			readout={HERO.readout}
			keys={[
				{ mark: "lit", label: "An agent at work" },
				{ mark: "raised", label: "Per token · flat plan · machine time" },
			]}
			caption={HERO.caption}
		>
			<MetersLive ex={EX} dayStep={DAY_STEP}>
				{ART}
			</MetersLive>
		</Plate>
		</div>
	);
}
