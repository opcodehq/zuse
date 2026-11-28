export type RouteName = "lan" | "tail" | "relay";
export type Mode = "racing" | "connected" | "reattaching" | "lost" | "unreachable";

export const ROUTES: RouteName[] = ["lan", "tail", "relay"];
export const LABEL: Record<RouteName, string> = { lan: "Local network", tail: "Tailnet", relay: "Relay" };
export const SAMPLE_RTT: Record<RouteName, number> = { lan: 4, tail: 11, relay: 46 };

export const RULE = {
	stagger: 200,
	dialTimeout: 10_000,
	ratio: 0.7,
	margin: 15,
	streak: 2,
	rest: 30_000,
	cooldown: 300_000,
	probeInterval: 60_000,
	probeConfirm: 3_000,
	probeTick: 2_000,
	resumeWindow: 50_000,
	pathCheck: 3_000,
	backoffStart: 100,
	backoffMax: 2_000,
};

const HEARTBEAT: Record<RouteName, { interval: number; timeout: number }> = {
	lan: { interval: 10_000, timeout: 5_000 },
	tail: { interval: 15_000, timeout: 15_000 },
	relay: { interval: 15_000, timeout: 15_000 },
};

export const vis = (ms: number) => (ms <= 0 ? 0 : 0.8 * Math.pow(ms / 200, 0.45));
export const real = (seconds: number) => (seconds <= 0 ? 0 : 200 * Math.pow(seconds / 0.8, 1 / 0.45));

export const tripOf = (route: RouteName) => 0.55 + SAMPLE_RTT[route] * 0.022;
export const trainTripOf = (route: RouteName) => 1.3 + SAMPLE_RTT[route] * 0.035;

export function beats(current: RouteName, currentRtt: number | null, candidate: RouteName, rtt: number) {
	const rank = (route: RouteName) => ROUTES.indexOf(route);
	if (currentRtt === null) return rank(candidate) < rank(current);
	const clearlyFaster = rtt + RULE.margin <= currentRtt && rtt <= currentRtt * RULE.ratio;
	const clearlySlower = currentRtt + RULE.margin <= rtt && currentRtt <= rtt * RULE.ratio;
	return clearlyFaster || (rank(candidate) < rank(current) && !clearlySlower);
}

export interface Pellet {
	id: number;
	route: RouteName;
	role: "dial" | "probe";
	start: number;
	ends: number;
	fails: boolean;
	at: number;
}

export interface Sim {
	now: number;
	up: Record<RouteName, boolean>;
	awake: boolean;
	known: Record<RouteName, boolean>;
	mode: Mode;
	session: boolean;
	current: RouteName | null;
	best: Partial<Record<string, RouteName>>;
	network: string;
	queue: RouteName[];
	nextStagger: number;
	pellets: Pellet[];
	streak: Record<RouteName, number>;
	nextProbe: Record<RouteName, number>;
	failures: Record<RouteName, number>;
	cooldown: Record<RouteName, number>;
	measured: Record<RouteName, number | null>;
	restUntil: number;
	probeTick: number;
	detectAt: number | null;
	deadline: number;
	reattachFrom: number;
	backoff: number;
	nextAttempt: number;
	retryAt: number | null;
	note: string;
	noteUntil: number;
	moves: number;
	serial: number;
}

const each = <T,>(value: (route: RouteName) => T) => ({ lan: value("lan"), tail: value("tail"), relay: value("relay") });

export function createSim(learned: boolean): Sim {
	const sim: Sim = {
		now: 0,
		up: each(() => true),
		awake: true,
		known: each((route) => learned || route === "relay"),
		mode: "racing",
		session: false,
		current: null,
		best: {},
		network: "wifi",
		queue: [],
		nextStagger: 0,
		pellets: [],
		streak: each(() => 0),
		nextProbe: each(() => 0),
		failures: each(() => 0),
		cooldown: each(() => 0),
		measured: each(() => null),
		restUntil: 0,
		probeTick: 0,
		detectAt: null,
		deadline: 0,
		reattachFrom: 0,
		backoff: RULE.backoffStart,
		nextAttempt: 0,
		retryAt: null,
		note: "",
		noteUntil: 0,
		moves: 0,
		serial: 1,
	};
	startRace(sim);
	return sim;
}

const reachable = (sim: Sim, route: RouteName) => sim.up[route] && sim.awake;

function note(sim: Sim, text: string, seconds = 2.4) {
	sim.note = text;
	sim.noteUntil = sim.now + seconds;
}

function dialOrder(sim: Sim): RouteName[] {
	const best = sim.best[sim.network];
	return ROUTES.filter((route) => sim.known[route]).sort((a, b) => Number(a !== best) - Number(b !== best) || ROUTES.indexOf(a) - ROUTES.indexOf(b));
}

function launch(sim: Sim, route: RouteName, role: Pellet["role"]) {
	const trip = tripOf(route);
	const fails = !reachable(sim, route);
	const ends = fails ? sim.now + (sim.up[route] ? vis(RULE.dialTimeout) : trip * 0.8) : sim.now + trip * 2;
	sim.pellets.push({ id: sim.serial++, route, role, start: sim.now, ends, fails, at: 0 });
}

function startRace(sim: Sim) {
	sim.queue = dialOrder(sim);
	sim.pellets = sim.pellets.filter((pellet) => pellet.role === "probe" && sim.mode === "connected");
	const first = sim.queue.shift();
	if (first) launch(sim, first, "dial");
	sim.nextStagger = sim.now + vis(RULE.stagger);
}

function becomeCurrent(sim: Sim, route: RouteName, takeover: boolean) {
	const resumed = sim.session;
	const first = !sim.session;
	sim.current = route;
	sim.mode = "connected";
	sim.session = true;
	sim.detectAt = null;
	sim.queue = [];
	sim.best[sim.network] = route;
	sim.measured[route] = SAMPLE_RTT[route];
	sim.failures[route] = 0;
	sim.streak[route] = 0;
	sim.nextProbe[route] = sim.now + vis(RULE.probeInterval);
	sim.probeTick = sim.now + vis(RULE.probeTick);
	sim.pellets = sim.pellets.filter((pellet) => pellet.role === "probe" && !takeover);
	if (first) {
		const wasKnown = sim.known.lan && sim.known.tail;
		sim.known = each(() => true);
		note(sim, wasKnown ? "connected" : "connected · learned 2 routes", 3);
	} else if (takeover) {
		sim.restUntil = sim.now + vis(RULE.rest);
		sim.moves += 1;
		note(sim, "moved · resting 30 s", 3);
	} else if (resumed) {
		note(sim, "resumed · nothing lost or repeated", 3);
	}
}

function loseCurrent(sim: Sim) {
	sim.current = null;
	sim.detectAt = null;
	sim.pellets = [];
	sim.mode = "reattaching";
	sim.reattachFrom = sim.now;
	sim.deadline = sim.now + vis(RULE.resumeWindow);
	sim.backoff = RULE.backoffStart;
	startRace(sim);
}

function raceFailed(sim: Sim) {
	if (sim.mode === "racing") {
		sim.mode = "unreachable";
		sim.retryAt = sim.now + 3;
		return;
	}
	if (sim.mode !== "reattaching") return;
	if (sim.now >= sim.deadline) return endLost(sim);
	sim.nextAttempt = sim.now + vis(sim.backoff);
	sim.backoff = Math.min(sim.backoff * 2, RULE.backoffMax);
}

function endLost(sim: Sim) {
	sim.mode = "lost";
	sim.session = false;
	sim.current = null;
	sim.pellets = [];
	sim.queue = [];
	sim.retryAt = null;
}

function judge(sim: Sim, route: RouteName) {
	const current = sim.current;
	if (!current) return;
	const rtt = SAMPLE_RTT[route];
	sim.measured[route] = rtt;
	const resting = sim.now < sim.restUntil;
	if (resting || !beats(current, sim.measured[current], route, rtt)) {
		sim.streak[route] = 0;
		sim.nextProbe[route] = sim.now + vis(RULE.probeInterval);
		return;
	}
	sim.streak[route] += 1;
	if (sim.streak[route] >= RULE.streak) {
		becomeCurrent(sim, route, true);
		return;
	}
	sim.nextProbe[route] = sim.now + vis(RULE.probeConfirm);
	note(sim, LABEL[route] + " faster · 1 of 2", 2);
}

function probeFailed(sim: Sim, route: RouteName) {
	sim.failures[route] += 1;
	sim.streak[route] = 0;
	sim.nextProbe[route] = sim.now + vis(Math.min(RULE.probeInterval * 2 ** Math.min(sim.failures[route], 8), 900_000));
}

function probesDue(sim: Sim): RouteName[] {
	const current = sim.current;
	if (!current || current === "lan" || sim.now < sim.restUntil) return [];
	const currentRtt = sim.measured[current];
	return ROUTES.filter((route) => route !== current && sim.known[route])
		.filter((route) => sim.now >= sim.cooldown[route] && sim.now >= sim.nextProbe[route])
		.filter((route) => ROUTES.indexOf(route) < ROUTES.indexOf(current) || currentRtt === null || currentRtt > RULE.margin);
}

export function step(sim: Sim, dt: number) {
	sim.now += dt;
	const now = sim.now;
	for (const pellet of sim.pellets) pellet.at = Math.min(1, (now - pellet.start) / Math.max(0.001, pellet.ends - pellet.start));
	if (sim.mode === "unreachable" || sim.mode === "lost") {
		if (sim.retryAt !== null && now >= sim.retryAt && ROUTES.some((route) => sim.known[route] && reachable(sim, route))) {
			sim.mode = "racing";
			sim.retryAt = null;
			startRace(sim);
		}
		return;
	}
	if (sim.queue.length && now >= sim.nextStagger) {
		const next = sim.queue.shift();
		if (next) launch(sim, next, "dial");
		sim.nextStagger = now + vis(RULE.stagger);
	}
	const done = sim.pellets.filter((pellet) => now >= pellet.ends);
	sim.pellets = sim.pellets.filter((pellet) => now < pellet.ends);
	for (const pellet of done) {
		if (pellet.role === "dial") {
			if (!pellet.fails && (sim.mode === "racing" || sim.mode === "reattaching")) {
				becomeCurrent(sim, pellet.route, false);
				sim.pellets = sim.pellets.filter((other) => other.role === "probe");
			} else if (pellet.fails) {
				if (!sim.queue.length && !sim.pellets.some((other) => other.role === "dial")) raceFailed(sim);
			}
		} else if (sim.mode === "connected") {
			if (pellet.fails) probeFailed(sim, pellet.route);
			else judge(sim, pellet.route);
		}
	}
	if (sim.mode === "connected" && sim.current) {
		if (sim.detectAt === null && !reachable(sim, sim.current)) {
			const beat = HEARTBEAT[sim.current];
			sim.detectAt = now + vis(beat.interval / 2 + beat.timeout);
		}
		if (sim.detectAt !== null && reachable(sim, sim.current)) sim.detectAt = null;
		if (sim.detectAt !== null && now >= sim.detectAt) {
			note(sim, "heartbeat missed", 2);
			loseCurrent(sim);
			return;
		}
		if (now >= sim.probeTick && !sim.pellets.length) {
			sim.probeTick = now + vis(RULE.probeTick);
			for (const route of probesDue(sim)) launch(sim, route, "probe");
		}
	}
	if (sim.mode === "reattaching" && !sim.queue.length && !sim.pellets.length && now >= sim.nextAttempt) {
		if (now >= sim.deadline) endLost(sim);
		else startRace(sim);
	}
}

export function setRoute(sim: Sim, route: RouteName, up: boolean) {
	if (sim.up[route] === up) return;
	sim.up[route] = up;
	if (route === "lan") networkChanged(sim, up ? "wifi" : "cellular");
	for (const pellet of sim.pellets) if (pellet.route === route && !up && !pellet.fails) {
		pellet.fails = true;
		pellet.ends = Math.max(sim.now + 0.3, pellet.start + tripOf(route) * 0.8);
	}
	wake(sim);
}

export function setAwake(sim: Sim, awake: boolean) {
	if (sim.awake === awake) return;
	sim.awake = awake;
	for (const pellet of sim.pellets) if (!awake && !pellet.fails) {
		pellet.fails = true;
		pellet.ends = pellet.start + vis(RULE.dialTimeout);
	}
	wake(sim);
}

function wake(sim: Sim) {
	if ((sim.mode === "unreachable" || sim.mode === "lost") && sim.retryAt === null) sim.retryAt = sim.now + 1;
}

function networkChanged(sim: Sim, network: string) {
	sim.network = network;
	for (const route of ROUTES) {
		sim.measured[route] = null;
		sim.streak[route] = 0;
		sim.failures[route] = 0;
		sim.cooldown[route] = 0;
		sim.nextProbe[route] = 0;
	}
	sim.restUntil = 0;
	sim.probeTick = sim.now;
	if (sim.mode === "connected" && sim.current) {
		sim.measured[sim.current] = SAMPLE_RTT[sim.current];
		if (!reachable(sim, sim.current)) sim.detectAt = sim.now + vis(RULE.pathCheck);
	}
}

export function readoutOf(sim: Sim): string {
	if (sim.mode === "unreachable") return "unreachable · no route answered";
	if (sim.mode === "lost") return "session lost · actions not replayed";
	if (sim.mode === "reattaching") {
		const elapsed = Math.min(50, Math.round(real(sim.now - sim.reattachFrom) / 1000));
		return "reattaching · " + elapsed + " s of 50";
	}
	if (sim.mode === "racing") return "racing " + sim.pellets.filter((p) => p.role === "dial").length + " of " + ROUTES.filter((r) => sim.known[r]).length + " routes";
	if (sim.now < sim.noteUntil && sim.note) return sim.note;
	const route = sim.current;
	if (!route) return "";
	const probing = sim.pellets.find((pellet) => pellet.role === "probe");
	if (probing) return LABEL[route] + " " + SAMPLE_RTT[route] + " ms · probing " + LABEL[probing.route].toLowerCase();
	return LABEL[route] + " · " + SAMPLE_RTT[route] + " ms";
}
