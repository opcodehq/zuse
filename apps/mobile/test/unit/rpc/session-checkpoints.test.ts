import { SessionId } from "@zuse/contracts";
import { Effect } from "effect";
import { beforeEach, expect, it, vi } from "vitest";

const rpc = vi.hoisted(() => ({
	list: vi.fn(),
	preview: vi.fn(),
	restore: vi.fn(),
	retry: vi.fn(),
	failure: vi.fn(),
}));
vi.mock("../../../src/rpc/connection", () => ({
	getConnectionClient: () =>
		Effect.succeed({
			"session.checkpoints.list": rpc.list,
			"session.checkpoints.preview": rpc.preview,
			"session.checkpoints.restore": rpc.restore,
			"session.checkpoints.retry": rpc.retry,
		}),
	reportConnectionFailure: rpc.failure,
}));

import { sessionCheckpointActions } from "../../../src/rpc/session-checkpoints";

const connection = { host: "localhost", port: 4317 };
const sessionId = SessionId.make("chosen-session");
beforeEach(() => {
	for (const mock of Object.values(rpc)) mock.mockReset();
});
it("pins review, restore and retry to the selected session and server-issued preview", async () => {
	rpc.list.mockReturnValue(Effect.succeed([]));
	rpc.preview.mockReturnValue(
		Effect.succeed({ previewId: "review", files: ["file.txt"], expiresAt: 42 }),
	);
	rpc.restore.mockReturnValue(Effect.void);
	rpc.retry.mockReturnValue(Effect.void);
	const actions = sessionCheckpointActions(connection, sessionId);
	await Effect.runPromise(actions.list());
	const preview = await Effect.runPromise(actions.preview("checkpoint"));
	expect(rpc.restore).not.toHaveBeenCalled();
	await Effect.runPromise(actions.restore(preview.previewId));
	await Effect.runPromise(actions.retry());
	expect(rpc.list).toHaveBeenCalledWith({ sessionId });
	expect(rpc.preview).toHaveBeenCalledWith({
		sessionId,
		checkpointId: "checkpoint",
	});
	expect(rpc.restore).toHaveBeenCalledWith({ sessionId, previewId: "review" });
	expect(rpc.retry).toHaveBeenCalledWith({ sessionId });
});
it("surfaces restore failure without locally retrying or fabricating completion", async () => {
	const error = new Error("working tree changed");
	rpc.restore.mockReturnValue(Effect.fail(error));
	await expect(
		Effect.runPromise(
			sessionCheckpointActions(connection, sessionId).restore("review"),
		),
	).rejects.toThrow("working tree changed");
	expect(rpc.restore).toHaveBeenCalledTimes(1);
	expect(rpc.failure).toHaveBeenCalledWith(connection, error);
});
