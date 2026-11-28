import { Dots, Line, Plate, Solid } from "@/components/iso/draw";
import "@/components/iso/iso.css";
import { ALL_ON, ALL_OFF, BINS, FIXES, cents, session } from "./session";
import { BASE, BINS_ART, CHUTE_ART, FILL_MOST, HEIGHT, HOPPER_ART, KNOB_OFF, PANEL_ART, SWITCH_ART, WIDTH, fillPaths, levelLine } from "./xray-geometry";
import { XrayLive } from "./xray-live";
import "./token-xray.css";

const ON = session(ALL_ON);
const OFF = session(ALL_OFF);
const MOST = Math.max(...OFF.rows.map((row) => row.cost));
const heightOf = (cost: number) => (cost / MOST) * FILL_MOST;

function Readout({ transform, width, height, slot, size, initial }: { transform: string; width: number; height: number; slot: string; size: number; initial: string }) {
	return (
		<g transform={transform}>
			<rect className="tkx-bezel" x={-1.6} y={-1.6} width={width + 3.2} height={height + 3.2} rx={1.6} />
			<rect className="tkx-glass" x={0} y={0} width={width} height={height} rx={0.8} />
			<path className="tkx-wall" d={`M0 0H${width}L${width - 1.1} 1.1H1.1V${height - 1.1}L0 ${height}Z`} />
			<clipPath id={`tkx-clip-${slot}`}>
				<rect x={0.6} y={0.6} width={width - 1.2} height={height - 1.2} />
			</clipPath>
			<g clipPath={`url(#tkx-clip-${slot})`}>
				<text className="tkx-digits" data-slot={slot} x={width - 2.4} y={height / 2 + size * 0.36} fontSize={size} textAnchor="end">
					{initial}
				</text>
			</g>
		</g>
	);
}

const ART = (
	<svg className="iso-svg" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label="A sorting bench. A hopper takes in the requests of one agent session, a chute drops every token into one of five glass bins, and each bin fills to what its stream costs. A panel of switches in front turns each fix on or off, and its window shows the bill for the task." focusable="false">
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
			<Solid paths={HOPPER_ART.plinth} tone="lo" />
			<Solid paths={HOPPER_ART.body} tone="mid" inner={<><Line d={HOPPER_ART.seamLow} tone="faint" /><Line d={HOPPER_ART.seamHigh} tone="faint" /></>} />
			<Dots points={HOPPER_ART.screws} size={0.5} tone="mid" />
			<g transform={HOPPER_ART.vents.transform}>
				{Array.from({ length: 4 }, (_, index) => (
					<rect key={index} className="tkx-slot" x={3 + index * 5.4} y={0} width={2.4} height={12} rx={1.2} />
				))}
			</g>
			<Readout {...HOPPER_ART.window} slot="requests" size={7} initial={String(ON.requests)} />
			<Solid paths={HOPPER_ART.collar} tone="mid" />
			<path className="tkx-mouth" d={HOPPER_ART.mouth} />
			<Line d={HOPPER_ART.mouthRim} tone="lo" />
		</g>

		{BINS_ART.map((entry, index) => (
			<g key={entry.index} data-bin={index}>
				<Solid paths={entry.foot} tone="lo" />
				<Line d={entry.floor} tone="faint" />
				<Solid paths={entry.back} tone="lo" crease="none" />
				<g data-fill={index}>
					<Solid paths={fillPaths(index, heightOf(ON.rows[index].cost))} tone="hi" lit />
				</g>
				<path className="tkx-level" data-level={index} d={levelLine(index, heightOf(OFF.rows[index].cost))} />
				<Solid paths={entry.side} tone="mid" crease="none" />
				<Solid paths={entry.glass} tone="faint" crease="none" className="tkx-pane" />
				{entry.frontPosts.map((paths, post) => (
					<Solid key={post} paths={paths} tone="mid" />
				))}
				<Solid paths={entry.scale} tone="lo" crease="none" />
				<Line d={entry.ticks.minor} tone="lo" />
				<Line d={entry.ticks.major} tone="mid" />
				<Solid paths={entry.cap} tone="mid" />
				<path className="tkx-mouth" d={entry.capMouth} />
				<Dots points={entry.capScrews} size={0.45} tone="mid" />
			</g>
		))}

		<g>
			<Solid paths={CHUTE_ART.flange} tone="mid" />
			<Dots points={CHUTE_ART.flangeScrews} size={0.45} tone="mid" />
			<Solid paths={CHUTE_ART.trough} tone="mid" />
			<Solid paths={CHUTE_ART.lip} tone="mid" crease="none" />
			{BINS_ART.map((entry) => (
				<g key={entry.index}>
					<Solid paths={entry.gate} tone="lo" />
					<Dots points={entry.gateScrews} size={0.4} tone="mid" />
				</g>
			))}
		</g>

		<g>
			<Solid paths={PANEL_ART.plinth} tone="lo" />
			<Solid paths={PANEL_ART.body} tone="mid" inner={<Line d={PANEL_ART.seam} tone="faint" />} />
			<Line d={PANEL_ART.inset} tone="faint" />
			<Dots points={PANEL_ART.screws} size={0.5} tone="mid" />
			<Dots points={PANEL_ART.faceScrews} size={0.5} tone="mid" />
			<Readout {...PANEL_ART.tokens} slot="tokens" size={6.2} initial={`${(ON.tokens / 1000).toFixed(1)}k`} />
			<Readout {...PANEL_ART.total} slot="total" size={7.4} initial={cents(ON.cost)} />
			{SWITCH_ART.map((entry) => {
				const locked = entry.index === FIXES.length;
				return (
					<g key={entry.index} data-switch={entry.index}>
						<Solid paths={entry.bezel} tone="lo" crease="none" />
						<Line d={entry.marks} tone="lo" />
						<path className="tkx-mouth" d={entry.slot} />
						<Solid paths={entry.lampCollar} tone="lo" />
						<g data-lamp={entry.index}>
							<Solid paths={entry.lamp} tone="mid" lit={!locked} />
						</g>
						<g data-knob={entry.index} transform={locked ? KNOB_OFF : undefined}>
							<Solid paths={entry.knob} tone="hi" />
							<Line d={entry.grip} tone="lo" />
						</g>
						{locked ? (
							<g>
								<Solid paths={entry.pin} tone="mid" />
								<Solid paths={entry.pinHead} tone="hi" />
							</g>
						) : null}
					</g>
				);
			})}
		</g>
	</svg>
);

export function XrayFigure() {
	return (
		<div className="iso not-prose tkx-figure">
			<Plate
				fig="Fig 1"
				title="One task, sorted by what it costs"
				hint="Tap a switch, or press 1–4"
				readout={`${cents(ON.cost)} a task · ${ON.requests} requests`}
				keys={[
					{ mark: "lit", label: "What the stream costs now" },
					{ mark: "dotted", label: "Its level with no fixes" },
				]}
				caption={`One real coding task from our benchmark suite: five requests, ${ON.tokens.toLocaleString("en-US")} tokens, priced at a mid-tier model's published rate. Left to right, the bins hold ${BINS.map((bin) => bin.label.toLowerCase()).join(", ")}. The fifth switch, bounded tool output, is pinned off.`}
			>
				<XrayLive most={MOST}>{ART}</XrayLive>
			</Plate>
		</div>
	);
}
