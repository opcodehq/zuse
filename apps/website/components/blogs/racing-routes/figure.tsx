import { Plate } from "@/components/iso/draw";
import { RoutesArt } from "./art";
import "@/components/iso/iso.css";
import "./racing-routes.css";

export function RacingRoutesFigure() {
	return (
		<div className="iso rr-figure not-prose">
			<Plate
				fig="Fig 1"
				title="Racing routes"
				hint="Tap a track · 1 2 3 S"
				readout="racing 1 of 1 routes"
				keys={[
					{ mark: "lit", label: "The session, carried by the car" },
					{ mark: "dotted", label: "Probes racing a route" },
					{ mark: "raised", label: "A lifted section is a cut route" },
				]}
				caption="Front to back: the local network, the tailnet through its tunnel, and the relay through its house. The board reads each route's last measurement. The rules are the ones in the link; the latencies (4, 11 and 46 ms) are sample values, and time is stretched during races and compressed during waits."
			>
				<RoutesArt />
			</Plate>
		</div>
	);
}
