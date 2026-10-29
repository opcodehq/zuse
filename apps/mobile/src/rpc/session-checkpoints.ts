import type { SessionId } from "@zuse/contracts";
import { Effect } from "effect";
import { getConnectionClient, reportConnectionFailure } from "./connection";
import type { WsProtocolOptions } from "./ws-protocol";

/** Checkpoint authority, expiry, quiescence and retry policy stay on the server. */
export const sessionCheckpointActions = (
	connection: WsProtocolOptions,
	sessionId: SessionId,
) => {
	const client = getConnectionClient(connection);
	const report = <A, E>(effect: Effect.Effect<A, E>) =>
		effect.pipe(
			Effect.tapError((cause) =>
				Effect.sync(() => reportConnectionFailure(connection, cause)),
			),
		);
	return {
		list: () =>
			report(
				client.pipe(
					Effect.flatMap((c) => c["session.checkpoints.list"]({ sessionId })),
				),
			),
		preview: (checkpointId: string) =>
			report(
				client.pipe(
					Effect.flatMap((c) =>
						c["session.checkpoints.preview"]({ sessionId, checkpointId }),
					),
				),
			),
		restore: (previewId: string) =>
			report(
				client.pipe(
					Effect.flatMap((c) =>
						c["session.checkpoints.restore"]({ sessionId, previewId }),
					),
				),
			),
		retry: () =>
			report(
				client.pipe(
					Effect.flatMap((c) => c["session.checkpoints.retry"]({ sessionId })),
				),
			),
	};
};
