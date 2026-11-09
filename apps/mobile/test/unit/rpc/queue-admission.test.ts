import { SessionId } from "@zuse/contracts";
import { Effect } from "effect";
import { expect, it, vi } from "vitest";

const bus = vi.hoisted(() => ({
	dispatch: vi.fn(() => Promise.resolve({ id: "queued" })),
}));
vi.mock("../../../src/store/mobile-client-bus", () => ({
	dispatchMobileSessionCommandResult: bus.dispatch,
	nextMobileCommandId: () => "queue-command",
}));
vi.mock("../../../src/rpc/connection", () => ({
	getConnectionClient: () => Effect.void,
	reportConnectionFailure: vi.fn(),
}));
vi.mock("../../../src/rpc/terminal-actions", () => ({
	mobileTerminalOpenOwnership: {},
}));

// Command tests do not use account-backed GitHub reads or native authentication.
vi.mock("../../../src/rpc/github", () => ({
	loadGithubObservation: vi.fn(),
}));

import { makeTextInput, queueMessage } from "../../../src/rpc/actions";

it("marks ordinary auto-queue admission while leaving explicit queue opt-in unchanged", async () => {
	const options = {
		connection: { host: "localhost", port: 4000 },
		sessionId: SessionId.make("s"),
		input: makeTextInput("draft"),
	};
	await Effect.runPromise(queueMessage({ ...options, rejectIfBlocked: true }));
	expect(bus.dispatch).toHaveBeenLastCalledWith(
		expect.objectContaining({
			kind: "messages.queue.add",
			payload: expect.objectContaining({ rejectIfBlocked: true }),
		}),
	);
	await Effect.runPromise(queueMessage(options));
	expect(bus.dispatch).toHaveBeenLastCalledWith(
		expect.objectContaining({
			payload: expect.objectContaining({ rejectIfBlocked: undefined }),
		}),
	);
});
