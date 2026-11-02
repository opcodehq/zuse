import type { SessionAttemptCursor, SessionId } from "@zuse/contracts";
import { Effect } from "effect";
import { getConnectionClient, reportConnectionFailure } from "./connection";
import type { WsProtocolOptions } from "./ws-protocol";

export const listSessionAttempts = (options: {
	connection: WsProtocolOptions;
	sessionId: SessionId;
	before?: SessionAttemptCursor;
}) =>
	Effect.gen(function* () {
		const client = yield* getConnectionClient(options.connection);
		return yield* client["session.attempts.list"]({
			sessionId: options.sessionId,
			...(options.before === undefined ? {} : { before: options.before }),
		});
	}).pipe(
		Effect.tapError((cause) =>
			Effect.sync(() => reportConnectionFailure(options.connection, cause)),
		),
	);
