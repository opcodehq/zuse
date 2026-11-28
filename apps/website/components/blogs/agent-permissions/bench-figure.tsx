import { Dots, Line, Plate, Solid } from "@/components/iso/draw";
import "@/components/iso/iso.css";
import { BAR, BASE, CONTROL, EY, EZ, FLAG_RISE, GUIDES, HEIGHT, KEY_BEZEL, KEY_BLADE, KEY_BOW, KEY_HOLE, KEY_PLUG, KEY_RIM, KEY_TICKS, LEVER_TRAVEL, LOCKBOX, MODULES, RAIL_ART, WIDTH, BAR_Z } from "./geometry";
import { CAPTION, REST, benchText, moduleState } from "./hero";
import { BenchLive } from "./bench-live";
import "./permissions.css";

const shift = (vector: [number, number], by: number) => "translate(" + (vector[0] * by).toFixed(2) + " " + (vector[1] * by).toFixed(2) + ")";
const LIFT = BAR_Z.up - BAR_Z.down;

const ART = (
	<svg key="art" className="iso-svg" viewBox={"0 0 " + WIDTH + " " + HEIGHT} role="img" aria-label="A lockout board on a bench: four switch modules for reading, edits, shell and external tools, each with a lamp and a flag, a lockout bar on two guide posts above them, a padlocked box for sensitive actions, and a control box with a three-notch key switch and a lever." focusable="false">
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
		<Solid paths={BASE.label} tone="lo" crease="none" />
		<Line d={BASE.labelGroove} tone="faint" />
		<Line d={BASE.labelRules} tone="lo" free />
		<Dots points={BASE.labelScrews} size={0.45} tone="mid" />

		<Solid paths={RAIL_ART.bus} tone="lo" crease="none" />
		<Dots points={RAIL_ART.busScrews} size={0.45} tone="mid" />
		<Solid paths={RAIL_ART.rail} tone="lo" />
		<Line d={RAIL_ART.lips} tone="faint" />
		{RAIL_ART.stops.map((paths, index) => (
			<Solid key={index} paths={paths} tone="mid" />
		))}

		{GUIDES.map((guide, index) => (
			<g key={"guide-" + index}>
				<Solid paths={guide.base} tone="lo" />
				<Solid paths={guide.post} tone="mid" />
				<Solid paths={guide.cap} tone="mid" />
			</g>
		))}

		{MODULES.map((module) => (
			<g key={"body-" + module.id}>
				<Solid paths={module.foot} tone="lo" />
				<Solid paths={module.body} tone="mid" inner={<Line d={module.seam} tone="faint" />} />
				<Line d={module.lid} tone="faint" />
				<Dots points={module.screws} size={0.5} tone="mid" />
				<g transform={module.vents}>
					{Array.from({ length: 4 }, (_, index) => (
						<rect key={index} className="ap-slot" x={index * 4.4} y={0} width={2.2} height={11} rx={1.1} />
					))}
				</g>
			</g>
		))}

		<g data-bar="" transform={shift(EZ, LIFT)}>
			<Solid paths={BAR.sleeves[0]} tone="mid" />
			<Solid paths={BAR.body} tone="hi" />
			<Solid paths={BAR.sleeves[1]} tone="mid" />
			<Solid paths={BAR.hasp} tone="mid" />
			<Dots points={[BAR.haspHole]} size={0.9} tone="lo" />
		</g>

		{MODULES.map((module) => (
			<g key={"lamp-" + module.id}>
				<Solid paths={module.collar} tone="lo" />
				<g data-lamp={module.id}>
					<Solid paths={module.lamp} tone="mid" lit={moduleState(module.id, REST).lit} />
				</g>
				<Line d={module.lampRing} tone="faint" />
				<Solid paths={module.slot} tone="lo" flat />
				<clipPath id={"ap-flag-" + module.id}>
					<path d={module.clip} />
				</clipPath>
				<g clipPath={"url(#ap-flag-" + module.id + ")"}>
					<g data-flag={module.id} transform={shift(EZ, FLAG_RISE[moduleState(module.id, REST).flag])}>
						<Solid paths={module.flag} tone="hi" />
					</g>
				</g>
			</g>
		))}

		<g>
			<Solid paths={LOCKBOX.plinth} tone="lo" />
			<Solid paths={LOCKBOX.body} tone="mid" inner={<Line d={LOCKBOX.seam} tone="faint" />} />
			<Line d={LOCKBOX.slot} tone="lo" />
			<Dots points={LOCKBOX.screws} size={0.5} tone="mid" />
		</g>

		<g data-bar="" transform={shift(EZ, LIFT)}>
			<Solid paths={BAR.sleeves[0]} tone="mid" />
			<Solid paths={BAR.body} tone="hi" />
			<Solid paths={BAR.sleeves[1]} tone="mid" />
			<Solid paths={BAR.hasp} tone="mid" />
			<Dots points={[BAR.haspHole]} size={0.9} tone="lo" />
		</g>

		{MODULES.map((module) => (
			<g key={"lamp-" + module.id}>
				<Solid paths={module.collar} tone="lo" />
				<g data-lamp={module.id}>
					<Solid paths={module.lamp} tone="mid" lit={moduleState(module.id, REST).lit} />
				</g>
				<Line d={module.lampRing} tone="faint" />
				<Solid paths={module.slot} tone="lo" flat />
				<clipPath id={"ap-flag-" + module.id}>
					<path d={module.clip} />
				</clipPath>
				<g clipPath={"url(#ap-flag-" + module.id + ")"}>
					<g data-flag={module.id} transform={shift(EZ, FLAG_RISE[moduleState(module.id, REST).flag])}>
						<Solid paths={module.flag} tone="hi" />
					</g>
				</g>
			</g>
		))}

		<g>
			<Solid paths={LOCKBOX.plinth} tone="lo" />
			<Solid paths={LOCKBOX.body} tone="mid" inner={<Line d={LOCKBOX.seam} tone="faint" />} />
			<Line d={LOCKBOX.slot} tone="lo" />
			<Dots points={LOCKBOX.screws} size={0.5} tone="mid" />
		</g>

		<g>
			<Solid paths={LOCKBOX.boss} tone="lo" />
			<Solid paths={LOCKBOX.post} tone="mid" />
			<Solid paths={LOCKBOX.flag} tone="hi" />
			<Solid paths={LOCKBOX.sleeve} tone="mid" />
			<Solid paths={LOCKBOX.cap} tone="mid" />
		</g>

		<g>
			{LOCKBOX.staples.map((paths, index) => (
				<Solid key={index} paths={paths} tone="lo" />
			))}
			<Solid paths={LOCKBOX.shackle[0]} tone="mid" />
			<Solid paths={LOCKBOX.shackle[1]} tone="mid" />
			<Solid paths={LOCKBOX.shackle[2]} tone="mid" />
			<Solid paths={LOCKBOX.pad} tone="hi" />
			<Dots points={[LOCKBOX.keyhole]} size={1} tone="lo" />
		</g>

		<g>
			<Solid paths={CONTROL.plinth} tone="lo" />
			<Solid paths={CONTROL.body} tone="mid" inner={<Line d={CONTROL.seam} tone="faint" />} />
			<Dots points={CONTROL.screws} size={0.5} tone="mid" />
			<g transform={CONTROL.key}>
				<path className="ap-bezel" d={KEY_BEZEL} />
				{KEY_TICKS.map((d, index) => (
					<Line key={index} d={d} tone={index === 0 ? "hi" : "mid"} />
				))}
				<path className="ap-plug" d={KEY_PLUG} />
				<Line d={KEY_RIM} tone="faint" />
				<g data-key="" transform="rotate(-52)">
					<path className="ap-key" d={KEY_BOW} />
					<path className="ap-key-hole" d={KEY_HOLE} transform="translate(0 -10.2)" />
					<path className="ap-blade" d={KEY_BLADE} />
				</g>
			</g>
			<g transform={CONTROL.plate}>
				<rect className="ap-plate" x={0} y={0} width={26} height={9} rx={1} />
				<path className="iso-line" data-tone="lo" data-free="" d="M3 3H17M3 6H13" />
			</g>
			<Solid paths={CONTROL.leverBoss} tone="lo" />
			<Line d={CONTROL.leverSlot} tone="lo" />
			<Line d={CONTROL.leverTicks} tone="mid" />
			<g data-lever="" transform={shift(EY, 0)}>
				<Solid paths={CONTROL.lever.stem} tone="mid" />
				<Solid paths={CONTROL.lever.knob} tone="hi" />
			</g>
		</g>
	</svg>
);

export function BenchFigure() {
	const text = benchText(REST);
	return (
		<div className="iso not-prose ap-figure">
			<Plate
				fig="Fig 1"
				title="The lockout board"
				hint="Tap to turn the key · P for Plan"
				readout={text.readout}
				keys={[
					{ mark: "lit", label: "Runs without asking" },
					{ mark: "raised", label: "Flag up: a card comes to you" },
				]}
				caption={CAPTION}
			>
				<BenchLive ez={EZ} ey={EY} lift={LIFT} travel={LEVER_TRAVEL} rises={FLAG_RISE}>
					{ART}
				</BenchLive>
			</Plate>
		</div>
	);
}
