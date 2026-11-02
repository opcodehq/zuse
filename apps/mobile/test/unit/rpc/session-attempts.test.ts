import { SessionId } from "@zuse/contracts";
import { Effect } from "effect";
import { beforeEach, expect, it, vi } from "vitest";

const rpc = vi.hoisted(() => ({ list: vi.fn(), failure: vi.fn() }));
vi.mock("../../../src/rpc/connection", () => ({
	getConnectionClient: () =>
		Effect.succeed({ "session.attempts.list": rpc.list }),
	reportConnectionFailure: rpc.failure,
}));

import { listSessionAttempts } from "../../../src/rpc/session-attempts";

const connection = { host: "localhost", port: 4317 };
const sessionId = SessionId.make("session");
beforeEach(() => {
	rpc.list.mockReset();
	rpc.failure.mockReset();
});
it("loads readonly pages against the selected connection and exact cursor", async () => {
	const page = { items: [], nextCursor: null };
	rpc.list.mockReturnValue(Effect.succeed(page));
	expect(
		await Effect.runPromise(listSessionAttempts({ connection, sessionId })),
	).toEqual(page);
	const before = { turnOrdinal: 4, attemptOrdinal: 2 };
	await Effect.runPromise(
		listSessionAttempts({ connection, sessionId, before }),
	);
	expect(rpc.list.mock.calls).toEqual([
		[{ sessionId }],
		[{ sessionId, before }],
	]);
});
it("reports query failures to connection recovery without swallowing the page error", async () => {
	const failure = new Error("disconnected");
	rpc.list.mockReturnValue(Effect.fail(failure));
	await expect(
		Effect.runPromise(listSessionAttempts({ connection, sessionId })),
	).rejects.toThrow("disconnected");
	expect(rpc.failure).toHaveBeenCalledWith(connection, failure);
});
