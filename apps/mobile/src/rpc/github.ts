import {
	GitHubObserver,
	githubTargetFromSummary,
} from "@zuse/client-runtime/github-observer";
import type { FolderId, GitHubTarget, WorktreeId } from "@zuse/contracts";
import { Effect } from "effect";
import { readGithubTarget, writeGithubTarget } from "../offline/cache";
import { cloudSummary } from "../store/cloud-catalog";
import { cloudControlClient } from "./api-client";
import { getConnectionClient, getConnectionSnapshot } from "./connection";
import type { WsProtocolOptions } from "./ws-protocol";

const observer = new GitHubObserver((input, signal) =>
	Effect.runPromise(cloudControlClient["cloud.github.observe"](input), {
		signal,
	}),
);
const contextRefreshes = new Map<string, Promise<GitHubTarget | null>>();
let epoch = 0;
export const resetGithubContext = () => {
	epoch++;
	observer.clear();
	contextRefreshes.clear();
};

/** Only identity discovery talks to a checkout; all GitHub reads use the signed-in account. */
export const loadGithubObservation = (
	options: {
		connection: WsProtocolOptions;
		folderId: FolderId;
		worktreeId?: WorktreeId | null;
	},
	details = false,
) =>
	Effect.gen(function* () {
		const generation = epoch;
		const connection = options.connection;
		const connKey =
			connection.key ??
			connection.environmentId ??
			`${connection.host}:${connection.port}`;
		const key = `${options.folderId}:${options.worktreeId ?? "main"}`;
		let target: GitHubTarget | null = yield* readGithubTarget(connKey, key);
		const summary = connection.cloudWorkspaceId
			? cloudSummary(connection.cloudWorkspaceId)
			: undefined;
		target ??= githubTargetFromSummary(summary);
		// Refresh facts only when already connected. Never wake a sandbox to read GitHub.
		if (
			getConnectionSnapshot(connection).status === "connected" ||
			(!target && !connection.cloudWorkspaceId)
		) {
			const identity = JSON.stringify([connKey, key]);
			let pending = contextRefreshes.get(identity);
			if (!pending) {
				pending = Effect.runPromise(
					Effect.gen(function* () {
						const client = yield* getConnectionClient(connection, false);
						const observed = yield* client["git.githubContext"]({
							folderId: options.folderId,
							worktreeId: options.worktreeId,
						});
						if (observed && generation === epoch)
							yield* writeGithubTarget(connKey, key, observed).pipe(
								Effect.catch(() => Effect.void),
							);
						return observed;
					}).pipe(Effect.timeout("5 seconds")),
				).catch(() => null);
				contextRefreshes.set(identity, pending);
				const current = pending;
				void pending.finally(() => {
					if (contextRefreshes.get(identity) === current)
						contextRefreshes.delete(identity);
				});
			}
			// Retained identity makes GitHub reads independent of a hung metadata RPC.
			if (!target) target = yield* Effect.promise(() => pending);
		}
		if (generation !== epoch)
			return yield* Effect.fail(new Error("Account changed."));
		if (!target)
			return yield* Effect.fail(
				new Error("GitHub repository information is not available yet."),
			);
		const selected = target;
		const result = yield* Effect.tryPromise({
			try: async (signal) => {
				if (details) return observer.details(selected, signal);
				await observer.refresh(selected);
				const observation = observer.snapshot(selected).value;
				if (!observation) throw new Error("GitHub status is unavailable.");
				return observation;
			},
			catch: (error) => error,
		});
		if (generation !== epoch)
			return yield* Effect.fail(new Error("Account changed."));
		return result;
	});
