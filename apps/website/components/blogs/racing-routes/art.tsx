import * as React from "react";
import { Dots, Line, Solid } from "@/components/iso/react/draw";
import { ROUTES, type RouteName } from "./model";
import {
	BASE_ART,
	BOARD_ART,
	COMPUTER_ART,
	DECKS,
	FRAMES,
	HOUSE_ART,
	HOUSE_KEY,
	PHONE_ART,
	SCREEN_GLOW,
	SIGNALS,
	TRACK_ART,
	TUNNEL_ART,
	TUNNEL_KEY,
	depthKey,
	type bufferOf,
} from "./geometry";
import { RoutesLive, type Structure } from "./live";

const Lamp = ({ at, route, kind, r = 1.1 }: { at: [number, number]; route?: RouteName; kind: string; r?: number }) => (
	<circle className="rr-lamp" data-kind={kind} data-route={route} cx={at[0]} cy={at[1]} r={r} />
);

function Buffer({ art }: { art: ReturnType<typeof bufferOf> }) {
	return (
		<>
			<Solid paths={art.block} tone="mid" crease="faint" />
			<Dots points={art.bolts} size={0.45} tone="mid" />
			{art.pads.map((paths, index) => (
				<Solid key={index} paths={paths} tone="mid" crease="none" />
			))}
			<Solid paths={art.beam} tone="mid" crease="faint" />
		</>
	);
}

const BASE = (
	<g key="base">
		<path className="iso-halo" d={BASE_ART.halo} />
		{BASE_ART.feet.map((parts, index) => (
			<g key={index}>
				{parts.map((paths, part) => (
					<Solid key={part} paths={paths} tone={part ? "mid" : "lo"} crease="faint" />
				))}
			</g>
		))}
		<Solid paths={BASE_ART.slab} tone="mid" crease="faint" />
		<Line d={BASE_ART.groove} tone="lo" />
		<Line d={BASE_ART.screwRings} tone="lo" />
		<Dots points={BASE_ART.screws} size={0.5} tone="mid" />
		<Solid paths={BASE_ART.cork} tone="lo" crease="none" flat />
		<Line d={BASE_ART.corkEdge} tone="faint" />
		<Solid paths={BASE_ART.plate} tone="lo" crease="none" />
		<Line d={BASE_ART.plateLine} tone="faint" />
		<Line d={BASE_ART.plateRule} tone="lo" free />
		<Dots points={BASE_ART.plateScrews} size={0.45} tone="mid" />
		<Solid paths={BASE_ART.strip} tone="lo" crease="none" />
		<Line d={BASE_ART.stripMinor} tone="lo" />
		<Line d={BASE_ART.stripMajor} tone="mid" />
	</g>
);

const BOARD = (
	<g key="board">
		<Solid paths={BOARD_ART.cabinet} tone="mid" crease="faint" inner={<Line d={BOARD_ART.topSeam} tone="faint" />} />
		<Dots points={BOARD_ART.screws} size={0.5} tone="mid" />
		{BOARD_ART.dials.map((dial) => (
			<g key={dial.route}>
				<Line d={dial.bezel} tone="mid" />
				<Line d={dial.face} tone="faint" />
				<Line d={dial.minor} tone="lo" />
				<Line d={dial.major} tone="mid" />
				<Line d={dial.lampRing} tone="lo" />
				<Lamp at={dial.lamp} route={dial.route} kind="board" r={1.2} />
			</g>
		))}
		<Solid paths={BOARD_ART.sill} tone="mid" crease="faint" />
		<Line d={BOARD_ART.sillEdge} tone="faint" />
		{BOARD_ART.dials.map((dial) => (
			<g key={dial.route}>
				<Solid paths={dial.toggle} tone="mid" crease="none" />
				<Line d={dial.toggleCap} tone="lo" />
			</g>
		))}
	</g>
);

const PITS = (
	<g key="pits">
		{TRACK_ART.pits.map((d, index) => (
			<Line key={index} d={d} tone="mid" />
		))}
	</g>
);

const SLEEPERS = (
	<g key="sleepers">
		{TRACK_ART.sleepers.map((paths, index) => (
			<Solid key={index} paths={paths} tone="mid" crease="none" />
		))}
	</g>
);

const RAILS = Object.fromEntries(ROUTES.map((route) => [route, <React.Fragment key={route}>
				<Line d={TRACK_ART.rails[route].d} tone="hi" />
				<Dots points={TRACK_ART.rails[route].ends} size={0.7} tone="mid" />
			</React.Fragment>])) as Record<RouteName, React.ReactNode>;

const DECK_ART = Object.fromEntries(
	ROUTES.map((route) => {
		const deck = DECKS[route];
		return [
			route,
			<g key={route}>
				<Solid paths={deck.plate} tone="hi" crease="faint" />
				{deck.sleepers.map((paths, index) => (
					<Solid key={index} paths={paths} tone="mid" crease="none" />
				))}
				<Line d={deck.rails} tone="mid" />
				<Dots points={deck.bolts} size={0.4} tone="mid" />
				<Solid paths={deck.handle} tone="mid" crease="none" />
			</g>,
		];
	}),
) as Record<RouteName, React.ReactNode>;

const signal = (route: RouteName) => {
	const art = SIGNALS[route];
	return (
		<g key={"signal-" + route}>
			<Solid paths={art.pad} tone="lo" crease="faint" />
			<Solid paths={art.post} tone="mid" crease="none" />
			<Solid paths={art.head} tone="mid" crease="faint" />
			<Line d={art.rings} tone="lo" />
			<Lamp at={art.lamps[0]!} route={route} kind="go" />
			<Lamp at={art.lamps[1]!} route={route} kind="stop" />
			<Solid paths={art.hood} tone="mid" crease="none" />
		</g>
	);
};

const frame = (index: number) => {
	const art = FRAMES[index]!;
	return (
		<g key={"frame-" + index}>
			<Solid paths={art.base} tone="mid" crease="faint" />
			<Line d={art.slot} tone="lo" />
			<Dots points={art.bolts} size={0.45} tone="mid" />
			{art.levers.map((lever) => (
				<g key={lever.route} className="rr-lever" data-route={lever.route}>
					<g data-pose="normal">
						<Solid paths={lever.normal} tone="mid" crease="none" />
						<Dots points={[lever.normalGrip]} size={0.7} tone="mid" />
					</g>
					<g data-pose="pulled">
						<Solid paths={lever.pulled} tone="hi" crease="none" />
						<Lamp at={lever.pulledGrip} kind="grip" r={0.9} />
					</g>
				</g>
			))}
		</g>
	);
};

const TUNNEL = (
	<g key="tunnel">
		<Solid paths={TUNNEL_ART.body} tone="mid" crease="faint" inner={<Line d={TUNNEL_ART.seams} tone="faint" />} />
		<Solid paths={TUNNEL_ART.vent} tone="mid" crease="none" />
		<Line d={TUNNEL_ART.ventRing} tone="lo" />
		<Solid paths={TUNNEL_ART.portal} tone="mid" crease="faint" />
		<Solid paths={TUNNEL_ART.coping} tone="mid" crease="faint" />
		<path className="rr-mouth" d={TUNNEL_ART.archInner + "Z"} />
		<Line d={TUNNEL_ART.arch} tone="mid" />
		<Line d={TUNNEL_ART.archInner} tone="lo" />
	</g>
);

const HOUSE = (
	<g key="house">
		<Solid paths={HOUSE_ART.body} tone="mid" crease="faint" inner={<Line d={HOUSE_ART.plinth} tone="faint" />} />
		<path className="rr-mouth" d={HOUSE_ART.doorInner} />
		<Line d={HOUSE_ART.door} tone="mid" />
		<Line d={HOUSE_ART.doorInner} tone="lo" />
		<Line d={HOUSE_ART.windows} tone="lo" />
		<Line d={HOUSE_ART.sills} tone="faint" />
		<Solid paths={HOUSE_ART.roof} tone="mid" crease="faint" />
		<Solid paths={HOUSE_ART.ridge} tone="mid" crease="faint" />
		<Solid paths={HOUSE_ART.mastBase} tone="mid" crease="none" />
		<Solid paths={HOUSE_ART.mast} tone="mid" crease="none" />
		<Lamp at={HOUSE_ART.mastTip} kind="relay" r={1} />
	</g>
);

const PHONE = (
	<g key="phone">
		<Solid paths={PHONE_ART.platform} tone="mid" crease="faint" />
		<Line d={PHONE_ART.platformEdge} tone="faint" />
		<Dots points={PHONE_ART.platformScrews} size={0.45} tone="mid" />
		<Solid paths={PHONE_ART.dock} tone="mid" crease="faint" />
		<Line d={PHONE_ART.slot} tone="lo" />
		<Solid paths={PHONE_ART.phone} tone="hi" crease="faint" lit />
		<path className="rr-screen" d={PHONE_ART.screen} />
		<Line d={PHONE_ART.rows} tone="lo" free />
		<Line d={PHONE_ART.button} tone="lo" />
		<Dots points={[PHONE_ART.camera]} size={0.4} tone="mid" />
		<Buffer art={PHONE_ART.buffer} />
	</g>
);

const COMPUTER = (
	<g key="computer">
		<Solid paths={COMPUTER_ART.platform} tone="mid" crease="faint" />
		<Line d={COMPUTER_ART.platformEdge} tone="faint" />
		<Dots points={COMPUTER_ART.platformScrews} size={0.45} tone="mid" />
		<Buffer art={COMPUTER_ART.buffer} />
		<Solid paths={COMPUTER_ART.lid} tone="mid" crease="faint" />
		<path className="rr-screen" d={COMPUTER_ART.screen} />
		<path className="rr-glow" d={SCREEN_GLOW} />
		<Solid paths={COMPUTER_ART.base} tone="hi" crease="faint" />
		<Dots points={COMPUTER_ART.keys} size={0.32} tone="lo" />
		<Line d={COMPUTER_ART.pad} tone="lo" />
		<Line d={COMPUTER_ART.hinge} tone="faint" />
		<Lamp at={COMPUTER_ART.led} kind="led" r={0.8} />
	</g>
);

const STRUCTURES: Structure[] = [
	{ id: "phone", key: depthKey([-232, 30]), node: PHONE },
	{ id: "computer", key: depthKey([232, 30]), node: COMPUTER },
	{ id: "house", key: HOUSE_KEY, node: HOUSE },
	{ id: "tunnel", key: TUNNEL_KEY, node: TUNNEL },
	...ROUTES.map((route) => ({ id: "signal-" + route, key: SIGNALS[route].key, node: signal(route) })),
	...FRAMES.map((art, index) => ({ id: "frame-" + index, key: art.key, node: frame(index) })),
].sort((a, b) => a.key - b.key);

const LABEL =
	"A model railway switchyard on a baseboard. A phone stands on a platform at the left, a laptop on a platform at the right. Three tracks run between them: the front track is the local network, the middle one runs through a tunnel and stands for a tailnet, and the back one stops inside a relay house. A dispatcher's board behind the yard has a latency dial and a lamp for each track. Small probe pellets race along the tracks, a car carrying the session shuttles on the chosen track, and a lift-out section on each track can be raised to cut it.";

export function RoutesArt() {
	return <RoutesLive label={LABEL} base={BASE} board={BOARD} pits={PITS} sleepers={SLEEPERS} rails={RAILS} decks={DECK_ART} structures={STRUCTURES} />;
}
