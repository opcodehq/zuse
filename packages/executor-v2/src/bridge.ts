import * as BrowserCrypto from "@effect/platform-browser/BrowserCrypto";
import * as PgClient from "@effect/sql-pg/PgClient";
import {
	AccountConnectionId,
	AccountId,
	AppId,
	AppName,
	type AppSourceStorage,
	aesGcmCredentials,
	type BlobStorage,
	BlobStoreError,
	BuildId,
	createExecutor,
	Json as JsonValue,
	makeExecutorStorage,
	OwnerId,
	ProfileId,
	type Runtime,
	RuntimeBuildFailed,
	RuntimeProtocolFailed,
	runtimeAdapter,
	SourceError,
	SourceFiles,
	SourceSnapshot,
	ToolName,
} from "@executor-js/sdk/core";
import {
	defineApp,
	defineProvider,
	oauth2,
	object,
	secrets,
	string,
} from "apps";
import {
	DeclaredRequirements,
	type HostContext,
	HostedCatalog,
	HostedCatalogSummary,
	HostResponse,
	HostInspectError,
	HostCallError,
	ToolResultObservation,
} from "apps/contracts";
import { createAppHandler, hostContext } from "apps/host";
import { mcpRouter } from "apps/mcp";
import {
	Effect,
	Layer,
	ManagedRuntime,
	Option,
	Redacted,
	Schema,
	Stream,
} from "effect";
import { FetchHttpClient, HttpClient } from "effect/unstable/http";
import type {
	EngineAuth,
	EngineConnection,
	EngineOptions,
	EnginePlugin,
	PluginEngine,
	PluginEngineErrorCode,
} from "./types.ts";

/** A failure the caller can explain; other failures stay opaque. */
export class PluginEngineError extends Error {
	constructor(
		readonly code: PluginEngineErrorCode,
		/** The SDK failure kind, for logs; never provider responses. */
		readonly detail?: { readonly tag: string; readonly reason?: string },
	) {
		super(code);
		this.name = "PluginEngineError";
	}
}
const CLIENT_REGISTRATION_REASONS = new Set([
	"client_registration_required",
	"client_not_approved",
	"client_metadata_rejected",
	"registration_rejected",
]);

const oauthSetupError = (error: unknown) => {
	if (typeof error !== "object" || error === null || !("_tag" in error))
		return error;
	const reason = "reason" in error ? error.reason : undefined;
	const detail = {
		tag: String(error._tag),
		...(typeof reason === "string" ? { reason } : {}),
	};
	if (
		error._tag === "OAuthClientUnavailable" ||
		(error._tag === "OAuthSetupFailed" &&
			typeof reason === "string" &&
			CLIENT_REGISTRATION_REASONS.has(reason))
	)
		return new PluginEngineError("client_registration_unsupported", detail);
	if (error._tag === "OAuthSetupFailed")
		return new PluginEngineError(
			reason === "service_unavailable" ? "unreachable" : "auth_unsupported",
			detail,
		);
	return error;
};

/**
 * Only these bundled declarations can execute: a remote MCP endpoint with one
 * auth method. Endpoints come from the host's lookup; no user source, eval, or
 * dynamic deployment.
 */
export async function createPluginEngine(
	options: EngineOptions,
): Promise<PluginEngine> {
	const sql = PgClient.layer({
		url: Redacted.make(options.databaseUrl),
		maxConnections: 2,
		connectTimeout: "5 seconds",
	});
	const managed = ManagedRuntime.make(
		Layer.mergeAll(
			sql,
			BrowserCrypto.layer,
			FetchHttpClient.layer.pipe(
				Layer.provide(Layer.succeed(FetchHttpClient.Fetch, options.fetch)),
			),
		),
	);
	const run = managed.runPromise;
	const io = <A>(work: () => Promise<A>) =>
		Effect.tryPromise({ try: work, catch: () => new RuntimeProtocolFailed() });
	type Entry = {
		readonly source: string;
		readonly build: BuildId;
		readonly handler: ReturnType<typeof createAppHandler>;
	};
	const entries = new Map<string, Promise<Entry>>();
	const builds = new Map<BuildId, Entry>();
	const definition = (pluginId: string) => {
		const plugin = options.plugin(pluginId);
		if (!plugin) throw new Error("Unknown plugin");
		return plugin;
	};
	const headerOf = (plugin: EnginePlugin) => plugin.header ?? "Authorization";

	const sourceOf = (plugin: EnginePlugin, auth: EngineAuth) =>
		`export default ${JSON.stringify({ protocol: 1, id: plugin.id, name: plugin.name, endpoint: plugin.endpoint, auth, scopes: plugin.scopes ?? [], ...(plugin.oauthDiscovery ? { oauthDiscovery: plugin.oauthDiscovery } : {}), ...(auth === "header" ? { header: headerOf(plugin) } : {}) })};\n`;

	const register = (pluginId: string, auth: EngineAuth): Promise<Entry> => {
		const key = JSON.stringify([pluginId, auth]);
		const existing = entries.get(key);
		if (existing) return existing;
		const created = (async () => {
			const plugin = definition(pluginId);
			const source = sourceOf(plugin, auth);
			const digest = await crypto.subtle.digest(
				"SHA-256",
				new TextEncoder().encode(source),
			);
			const build = BuildId.make(
				`bld_${Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("")}`,
			);
			const header = headerOf(plugin);
			const app =
				auth === "none"
					? defineApp({ accounts: {} }, async ({ signal }) => ({
							tools: await mcpRouter({ url: plugin.endpoint, signal }),
						}))
					: auth === "header"
						? defineApp(
								{
									accounts: {
										service: defineProvider({
											name: plugin.name,
											auth: {
												key: secrets({
													label: header,
													fields: object({ value: string({ minLength: 1 }) }),
												}),
											},
										}),
									},
								},
								async ({ accounts, signal }) => ({
									tools: await mcpRouter({
										url: plugin.endpoint,
										signal,
										headers: { [header]: accounts.service.fields.value },
										accountId: accounts.service.id,
									}),
								}),
							)
						: defineApp(
								{
									accounts: {
										service: defineProvider({
											name: plugin.name,
											auth: {
												oauth: oauth2({
													discover: plugin.oauthDiscovery ?? plugin.endpoint,
													scopes: [...(plugin.scopes ?? [])],
												}),
											},
										}),
									},
								},
								async ({ accounts, signal }) => ({
									tools: await mcpRouter({
										url: plugin.endpoint,
										signal,
										accountId: accounts.service.id,
										headers: {
											Authorization: `Bearer ${accounts.service.fields.access_token}`,
										},
									}),
								}),
							);
			const entry = { source, build, handler: createAppHandler(app) };
			builds.set(build, entry);
			return entry;
		})();
		entries.set(key, created);
		created.catch(() => entries.delete(key));
		return created;
	};

	const entryForSource = async (source: string | undefined) => {
		if (source === undefined) return undefined;
		for (const entry of builds.values())
			if (entry.source === source) return entry;
		const declared = /^export default (\{.*\});\n$/s.exec(source)?.[1];
		if (!declared) return undefined;
		const parsed: unknown = JSON.parse(declared);
		if (typeof parsed !== "object" || parsed === null) return undefined;
		const { id, auth } = parsed as { id?: unknown; auth?: unknown };
		if (
			typeof id !== "string" ||
			(auth !== "none" && auth !== "oauth" && auth !== "header") ||
			!options.plugin(id)
		)
			return undefined;
		const entry = await register(id, auth);
		return entry.source === source ? entry : undefined;
	};
	const dispatch = <E>(
		build: string,
		command: unknown,
		error: Schema.Decoder<E>,
		context: HostContext = hostContext({}),
	) =>
		io(async () => {
			const item = builds.get(BuildId.make(build));
			if (!item) throw new Error("Unknown plugin build");
			const response = await item.handler(
				new Request("https://zuse.invalid/", {
					method: "POST",
					body: JSON.stringify(command),
					headers: { "content-type": "application/json" },
					signal: AbortSignal.timeout(30_000),
				}),
				context,
			);
			const result = Schema.decodeUnknownSync(HostResponse)(
				await response.json(),
			);
			return result;
		}).pipe(
			Effect.flatMap((result) => {
				if (!result.ok)
					return Schema.decodeUnknownEffect(error)(result.error).pipe(
						Effect.mapError(() => new RuntimeProtocolFailed()),
						Effect.flatMap(Effect.fail),
					);
				return Effect.gen(function* () {
					if (result.toolError === true)
						(yield* ToolResultObservation).failed();
					return result.value;
				});
			}),
		);
	const decode = <A>(schema: Schema.Decoder<A>) =>
		Effect.flatMap((value: unknown) =>
			Schema.decodeUnknownEffect(schema)(value).pipe(
				Effect.mapError(() => new RuntimeProtocolFailed()),
			),
		);
	const unsupported = () => Effect.fail(new RuntimeProtocolFailed());
	const runtime: Runtime = {
		build: ({ files }) =>
			Effect.gen(function* () {
				const source = files.find((file) => file.path === "index.ts")?.content;
				const entry = yield* Effect.tryPromise({
					try: () => entryForSource(source),
					catch: () => new RuntimeBuildFailed({ stage: "compile" }),
				});
				if (!entry) return yield* new RuntimeBuildFailed({ stage: "compile" });
				const requirements = yield* dispatch(
					entry.build,
					{
						operation: "requirements",
					},
					HostInspectError,
				).pipe(
					decode(DeclaredRequirements),
					Effect.mapError(() => new RuntimeBuildFailed({ stage: "compile" })),
				);
				return { build: entry.build, requirements };
			}),
		skills: () => Effect.succeed({ skills: [] }),
		inspect: ({ build, tools, ...context }) =>
			dispatch(
				build,
				{ operation: "inspect", ...(tools ? { tools } : {}) },
				HostInspectError,
				context,
			).pipe(decode(HostedCatalog)),
		index: ({ build, ...context }) =>
			dispatch(
				build,
				{ operation: "inspect", detail: "summary" },
				HostInspectError,
				context,
			).pipe(decode(HostedCatalogSummary)),
		call: ({ build, tool, input, kind, ...context }) =>
			dispatch(
				build,
				{
					operation: "call",
					tool,
					input,
					...(kind === undefined ? {} : { kind }),
				},
				HostCallError,
				context,
			).pipe(decode(JsonValue)),
		query: unsupported,
		mutate: unsupported,
		webhook: unsupported,
		workflow: unsupported,
		checkAccount: unsupported,
		changes: () => Stream.empty,
	};
	const blobs: BlobStorage = {
		get: (key) =>
			Effect.tryPromise({
				try: async () =>
					Option.fromNullishOr(
						await options.storage.get<Uint8Array>(`v2:blob:${key}`),
					),
				catch: () => new BlobStoreError({ operation: "get" }),
			}),
		put: (key, body) =>
			Effect.tryPromise({
				try: () => options.storage.put(`v2:blob:${key}`, body),
				catch: () => new BlobStoreError({ operation: "put" }),
			}),
		remove: (key) =>
			Effect.tryPromise({
				try: async () => {
					await options.storage.delete(`v2:blob:${key}`);
				},
				catch: () => new BlobStoreError({ operation: "remove" }),
			}),
	};
	const sourceIo = <A>(work: () => Promise<A>) =>
		Effect.tryPromise({
			try: work,
			catch: () => new SourceError({ reason: "storage" }),
		});
	const retain: AppSourceStorage["retain"] = (code, files) =>
		sourceIo(async () => {
			const digest = await crypto.subtle.digest(
				"SHA-1",
				new TextEncoder().encode(JSON.stringify(files)),
			);
			const commit = Array.from(new Uint8Array(digest), (b) =>
				b.toString(16).padStart(2, "0"),
			).join("");
			await options.storage.put(`v2:source:${code}:${commit}`, files);
			return { code, commit };
		});
	const sources: AppSourceStorage = {
		retain,
		read: ({ code, commit }) =>
			sourceIo(async () =>
				Schema.decodeUnknownSync(SourceFiles)(
					await options.storage.get(`v2:source:${code}:${commit}`),
				),
			),
		workspace: (code) =>
			sourceIo(async () => {
				const value = await options.storage.get(`v2:head:${code}`);
				return value === undefined
					? null
					: Schema.decodeUnknownSync(SourceSnapshot)(value);
			}),
		commit: (input) =>
			Effect.gen(function* () {
				const old = yield* sources.workspace(input.code);
				if ((old?.revision.commit ?? null) !== input.expected)
					return yield* new SourceError({ reason: "conflict" });
				const revision = yield* retain(input.code, input.files);
				const next = { revision, files: input.files };
				yield* sourceIo(() =>
					options.storage.put(`v2:head:${input.code}`, next),
				);
				return next;
			}),
	};
	try {
		const db = await run(makeExecutorStorage({ provider: "postgresql" }));
		if (options.migrate) await run(db.migrate);
		else await run(db.checkMigration);
		const bytes = Uint8Array.from(
			atob(options.encryptionKey.replaceAll("-", "+").replaceAll("_", "/")),
			(c) => c.charCodeAt(0),
		);
		if (bytes.length !== 32) throw new Error("Invalid plugin encryption key");
		const material = await crypto.subtle.importKey(
			"raw",
			bytes,
			"HKDF",
			false,
			["deriveBits"],
		);
		const scoped = await crypto.subtle.deriveBits(
			{
				name: "HKDF",
				hash: "SHA-256",
				salt: new TextEncoder().encode(options.credentialScope),
				info: new TextEncoder().encode("zuse-plugins-v2"),
			},
			material,
			256,
		);
		const hex = Array.from(new Uint8Array(scoped), (b) =>
			b.toString(16).padStart(2, "0"),
		).join("");
		const credentials = await run(
			aesGcmCredentials(Redacted.make(hex), crypto),
		);
		const client = await run(HttpClient.HttpClient);
		const e = await run(
			createExecutor({
				storage: db,
				sources,
				blobs,
				credentials,
				runtime: runtimeAdapter(runtime),
				oauth: {
					httpClient: client,
					clientName: "Zuse",
					urlPolicy: {
						allowLoopbackHttp: options.allowLoopbackHttp ?? false,
						allowedHttpOrigins: [],
					},
				},
			}),
		);
		await run(e.apps.list({ owner: OwnerId.make("__readiness__") }));
		const ownerOf = (owner: string) => OwnerId.make(owner);

		const target = async (owner: string, ref: EngineConnection) => {
			await register(ref.plugin, ref.auth);
			const app = AppId.make(ref.app),
				profile = ProfileId.make(ref.profile);
			await run(e.apps.get({ app, owner: ownerOf(owner) }));
			await run(e.apps.profiles.get({ app, profile, owner: ownerOf(owner) }));
			return { app, profile };
		};
		return {
			async prepare(owner, subject, id, pluginId, auth) {
				const entry = await register(pluginId, auth);
				const name = AppName.make(id),
					own = ownerOf(owner);
				const existing = await run(e.apps.list({ owner: own, name }));
				const app =
					existing[0] ??
					(
						await run(
							e.apps.deploy({
								owner: own,
								name,
								files: [{ path: "index.ts", content: entry.source }],
							}),
						)
					).app;
				const profile = await run(
					e.apps.profiles.create({
						app: app.id,
						owner: own,
						subject,
						accounts: {},
						idempotencyKey: id,
					}),
				);
				const ref = {
					app: app.id,
					profile: profile.id,
					plugin: pluginId,
					auth,
				};
				if (auth === "none") return ref;
				const connection = await run(
					e.accountConnections.create({
						owner: own,
						target: {
							app: app.id,
							profile: profile.id,
							requirement: "service",
						},
					}),
				);
				return { ...ref, connection: connection.id };
			},
			async start(owner, ref, label, redirectUri) {
				await target(owner, ref);
				if (!ref.connection) throw new Error("Missing connection");
				const result = await run(
					e.accountConnections.startOAuth({
						owner: ownerOf(owner),
						connection: AccountConnectionId.make(ref.connection),
						method: "oauth",
						label,
						redirectUri,
					}),
				).catch((error: unknown) => {
					throw oauthSetupError(error);
				});
				if (result.status !== "redirect")
					throw new Error("Expected authorization redirect");
				return result.authorizationUrl;
			},
			async submit(owner, ref, label, value) {
				await target(owner, ref);
				if (!ref.connection) throw new Error("Missing connection");
				const account = await run(
					e.accountConnections.submit({
						owner: ownerOf(owner),
						connection: AccountConnectionId.make(ref.connection),
						method: "key",
						label,
						fields: Redacted.make({ value }),
					}),
				);
				return { ...ref, account: account.id };
			},
			async complete(owner, ref, callbackUrl) {
				await target(owner, ref);
				if (!ref.connection) throw new Error("Missing connection");
				const result = await run(
					e.accountConnections.completeOAuth({
						owner: ownerOf(owner),
						connection: AccountConnectionId.make(ref.connection),
						callbackUrl: Redacted.make(callbackUrl),
					}),
				);
				return { ...ref, account: result.id };
			},
			async remove(owner, id, ref) {
				const own = ownerOf(owner);
				const app =
					ref?.app ??
					(await run(e.apps.list({ owner: own, name: AppName.make(id) })))[0]
						?.id;
				let account = ref?.account;
				if (ref?.connection) {
					const cancelled = await run(
						e.accountConnections.cancel({
							owner: own,
							connection: AccountConnectionId.make(ref.connection),
						}),
					);
					if (cancelled.state.status === "completed")
						account = cancelled.state.account.id;
				}
				if (app) await run(e.apps.remove({ app: AppId.make(app), owner: own }));
				if (account)
					await run(
						e.accounts.remove({ account: AccountId.make(account), owner: own }),
					);
			},
			async list(owner, ref) {
				const t = await target(owner, ref);
				const result = await run(e.tools.index(t));
				return result.items.map((tool) => ({
					name: tool.name,
					description: tool.description,
				}));
			},
			async schema(owner, ref, tool) {
				return run(
					e.tools.get({
						...(await target(owner, ref)),
						tool: ToolName.make(tool),
					}),
				);
			},
			async call(owner, ref, tool, input) {
				const result = await run(
					e.tools.call({
						...(await target(owner, ref)),
						tool: ToolName.make(tool),
						input: Schema.decodeUnknownSync(JsonValue)(input),
					}),
				);
				if (result.status !== "completed")
					throw new Error("Plugin requires additional approval");
				return result.value;
			},
			close: () => managed.dispose(),
		};
	} catch (error) {
		await managed.dispose();
		throw error;
	}
}
