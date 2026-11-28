import type { CSSProperties } from "react";
import { Dots, Line, Plate, Solid } from "@/components/iso/draw";
import "@/components/iso/iso.css";
import { BASE, CELLS, DIAL_BEZEL, DIAL_GLASS, DIAL_RIM, DIAL_TICKS, EX, HEIGHT, HOUSING_ART, MANIFOLD_ART, MINUTE_STEP, RIDER, TRAY_ART, WIDTH, angleOf } from "./geometry";
import { IdleLive } from "./live";
import { MOST_AGENTS, QUIET_MINUTES, memoryAt, mib, saving } from "./model";
import "./idle-agent.css";

const START = { open: MOST_AGENTS, native: memoryAt(MOST_AGENTS, "native") ?? 0, reference: memoryAt(MOST_AGENTS, "reference") ?? 0 };
export const START_READOUT = `${START.open} open · ${mib(START.native)}`;

const ART = (
	<svg key="art" className="iso-svg" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label="A rack of twenty small gas burners on a heavy bench, fed by one manifold with a main valve. Each burner is an agent: a pilot flame means it is open and waiting. A meter on the right shows the memory the open agents hold and counts machine minutes while the computer is awake, and a slider on the front counts quiet minutes until the computer sleeps." focusable="false">
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
			{MANIFOLD_ART.saddles.map((paths, index) => (
				<Solid key={index} paths={paths} tone="lo" />
			))}
			<Solid paths={MANIFOLD_ART.valveBody} tone="mid" />
			<Dots points={MANIFOLD_ART.valveScrews} size={0.45} tone="mid" />
			<Solid paths={MANIFOLD_ART.valveStem} tone="mid" />
			<g data-lever="shut" style={{ opacity: 0 }}>
				<Solid paths={MANIFOLD_ART.leverShut} tone="hi" />
			</g>
			<g data-lever="open">
				<Solid paths={MANIFOLD_ART.leverOpen} tone="hi" />
			</g>
			<Solid paths={MANIFOLD_ART.body} tone="mid" />
			<Solid paths={MANIFOLD_ART.cap} tone="mid" />
			{MANIFOLD_ART.bands.map((paths, index) => (
				<Solid key={index} paths={paths} tone="lo" />
			))}
			{MANIFOLD_ART.feeds.map((paths, index) => (
				<Solid key={index} paths={paths} tone="lo" />
			))}
		</g>

		<g>
			<Solid paths={TRAY_ART.body} tone="lo" inner={<Line d={TRAY_ART.seam} tone="faint" />} />
			<Dots points={TRAY_ART.screws} size={0.45} tone="mid" />
		</g>

		{CELLS.map((cell) => (
			<g key={cell.index} data-cell={cell.index} data-state="pilot" style={{ "--cell-delay": `${(cell.index * 37) % 520}ms` } as CSSProperties}>
				<Solid paths={cell.seat} tone="lo" />
				<Solid paths={cell.cup} tone="mid" />
				<Line d={cell.ports} tone="faint" />
				<Solid paths={cell.crown} tone="lo" />
				<g data-part="cap">
					<Solid paths={cell.cap} tone="mid" />
				</g>
				<g data-part="flame">
					<Solid paths={cell.flame} tone="mid" lit />
				</g>
				<g data-part="pilot">
					<Solid paths={cell.pilot} tone="mid" lit />
				</g>
			</g>
		))}

		<g>
			<Solid paths={BASE.strip} tone="lo" crease="none" />
			<Line d={BASE.ticks.minor} tone="lo" />
			<Line d={BASE.ticks.major} tone="mid" />
			<Line d={BASE.ticks.zero} tone="hi" />
			<Solid paths={BASE.stop} tone="mid" />
		</g>
		<g data-rider="">
			<Solid paths={RIDER.body} tone="hi" />
			<Line d={RIDER.index} tone="lit" />
		</g>

		<g>
			<Solid paths={HOUSING_ART.plinth} tone="lo" inner={<Line d={HOUSING_ART.plinthSeam} tone="faint" />} />
			<Solid paths={HOUSING_ART.body} tone="mid" inner={<Line d={HOUSING_ART.seam} tone="faint" />} />
			<Dots points={HOUSING_ART.screws} size={0.5} tone="mid" />
			<g transform={HOUSING_ART.vents.transform}>
				{Array.from({ length: 4 }, (_, index) => (
					<rect key={index} className="ia-slot" x={22 + index * 8} y={4} width={3} height={10} rx={1.5} />
				))}
			</g>
			<g transform={HOUSING_ART.dial.transform}>
				<path className="ia-bezel" d={DIAL_BEZEL} />
				<path className="ia-dial" d={DIAL_GLASS} />
				<Line d={DIAL_RIM} tone="faint" />
				{DIAL_TICKS.map((tick, index) => (
					<Line key={index} d={tick.d} tone={tick.major ? "mid" : "lo"} />
				))}
				<g data-needle="reference" transform={`rotate(${angleOf(START.reference).toFixed(2)})`}>
					<path className="ia-ghost" d="M0 2.4V-16.2" />
				</g>
				<g data-needle="native" transform={`rotate(${angleOf(START.native).toFixed(2)})`}>
					<path className="ia-needle" d="M-0.9 2.6L-0.35 -15.8H0.35L0.9 2.6Z" />
				</g>
				<circle className="ia-hub" r={2} />
			</g>
			<g transform={HOUSING_ART.window.transform}>
				<rect className="ia-bezel" x={-1.8} y={-1.8} width={HOUSING_ART.window.width + 3.6} height={HOUSING_ART.window.height + 3.6} rx={1.8} />
				<rect className="ia-glass" x={0} y={0} width={HOUSING_ART.window.width} height={HOUSING_ART.window.height} rx={0.8} />
				<path className="ia-wall" d={`M0 0H${HOUSING_ART.window.width}L${HOUSING_ART.window.width - 1.2} 1.2H1.2V${HOUSING_ART.window.height - 1.2}L0 ${HOUSING_ART.window.height}Z`} />
				<clipPath id="ia-clip-counter">
					<rect x={0.6} y={0.6} width={HOUSING_ART.window.width - 1.2} height={HOUSING_ART.window.height - 1.2} />
				</clipPath>
				<g clipPath="url(#ia-clip-counter)">
					<text className="ia-digits" data-slot="counter" x={HOUSING_ART.window.width - 2.6} y={HOUSING_ART.window.height / 2 + 2.3} fontSize={6.4} textAnchor="end">
						0
					</text>
				</g>
			</g>
			<Solid paths={HOUSING_ART.lid} tone="mid" />
			<Solid paths={HOUSING_ART.tag} tone="lo" crease="none" />
			<Line d={HOUSING_ART.tagRules} tone="lo" free />
			<Solid paths={HOUSING_ART.knobBoss} tone="lo" />
			<Solid paths={HOUSING_ART.knob} tone="mid" />
			<Line d={HOUSING_ART.knurl} tone="faint" />
			<Solid paths={HOUSING_ART.lampCollar} tone="lo" />
			<g data-light="awake">
				<Solid paths={HOUSING_ART.lamp} tone="mid" lit />
			</g>
		</g>

		<g>
			<Solid paths={BASE.label} tone="lo" crease="none" />
			<Line d={BASE.labelGroove} tone="faint" />
			<Line d={BASE.labelRules} tone="lo" free />
			<Dots points={BASE.labelScrews} size={0.45} tone="mid" />
		</g>
	</svg>
);

export function IdleFigure() {
	return (
		<div className="iso not-prose ia-figure">
			<Plate
				fig="Fig 1"
				title="Twenty agents, one meter"
				hint="Tap or → to wake an agent"
				readout={START_READOUT}
				keys={[
					{ mark: "lit", label: "Pilot: an open, idle agent" },
					{ mark: "edge", label: "Memory the open agents hold" },
					{ mark: "dashed", label: "A reference agent server, same test" },
				]}
				caption={`Each burner is one agent on a shared host. The dial reads idle memory measured with 1, 2, 5, 10 and 20 agents open and interpolates between them: ${mib(START.native)} for twenty, against ${mib(START.reference)} for a reference server in the same test, ${Math.round(saving(MOST_AGENTS) * 100)}% less. Time runs at one minute a second. The counter stamps 4 machine minutes per awake minute of a standard cloud computer, and after ${QUIET_MINUTES} quiet minutes the computer sleeps: the valve shuts, the pilots go out, and nothing is counted.`}
			>
				<IdleLive ex={EX} minuteStep={MINUTE_STEP}>
					{ART}
				</IdleLive>
			</Plate>
		</div>
	);
}
