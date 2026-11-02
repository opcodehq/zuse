import { writeFileSync } from "node:fs";
import { resolve } from "node:path";

const uploads = new Map<string, Buffer[]>();
const files = new Map<
	string,
	{ bytes: string; mimeType: string; originalName: string }
>();
type FixtureSession = {
	id: string;
	projectId: string;
	title: string;
	providerId: string;
	model: string;
	worktreeId: string | null;
};
const fixtureSession: FixtureSession = {
	id: "native-fixture",
	projectId: "fixture-project",
	title: "Fixture conversation",
	providerId: "fixture",
	model: "fixture",
	worktreeId: null,
};
const server = Bun.serve<{
	timeline?: string;
	version: number;
	session?: FixtureSession;
	initialPrompt?: string;
	settings?: Record<string, unknown>;
}>({
	hostname: "127.0.0.1",
	port: 0,
	fetch(request, server) {
		if (server.upgrade(request, { data: { version: 0 } })) return;
		return new Response("WebSocket required", { status: 426 });
	},
	websocket: {
		message(socket, raw) {
			const frame = JSON.parse(String(raw));
			const activeSession = socket.data.session ?? fixtureSession;
			if (frame._tag !== "Request") return;
			if (process.env.ZUSE_FIXTURE_DIAGNOSTICS) console.log(frame.tag);
			const reply = (value: unknown) =>
				socket.send(
					JSON.stringify({
						_tag: "Exit",
						requestId: frame.id,
						exit: { _tag: "Success", value },
					}),
				);
			if (frame.tag === "connect.handshake") reply({ protocolVersion: 7 });
			else if (frame.tag === "connect.describe")
				reply({
					environmentId: "fixture-environment",
					label: "Fixture computer",
				});
			else if (frame.tag === "workspace.list")
				reply([{ id: "fixture-project", name: "Fixture project" }]);
			else if (frame.tag === "session.list") reply([activeSession]);
			else if (frame.tag === "session.get") reply(activeSession);
			else if (frame.tag === "model.catalog")
				reply({
					providers: {
						fixture: {
							displayName: "Fixture agent",
							defaultModelId: "fixture",
							models: [
								{ id: "fixture", label: "Fixture model", defaultModel: true },
							],
						},
					},
				});
			else if (frame.tag === "git.prState")
				reply({
					state: "open",
					branch: "main",
					baseBranch: "main",
					additions: 1,
					deletions: 1,
					number: 42,
					url: "https://github.com/example/project/pull/42",
					isDraft: false,
					checks: "success",
					mergeable: "mergeable",
					checksTotal: 1,
					checksRunning: 0,
					checksPassing: 1,
					checksFailing: 0,
					autoMergeEnabled: false,
				});
			else if (frame.tag === "git.status")
				reply({ branch: "main", ahead: 0, behind: 0, dirtyFiles: 1 });
			else if (frame.tag === "git.branches")
				reply([
					{ name: "main", kind: "local", remote: null, current: true },
					{ name: "feature", kind: "remote", remote: "origin", current: false },
				]);
			else if (frame.tag === "worktree.list")
				reply([
					{ id: "fixture-tree", name: "Fixture worktree", branch: "feature" },
				]);
			else if (frame.tag === "worktree.create")
				reply({
					id: "fixture-tree",
					name: "Fixture worktree",
					branch: "feature",
				});
			else if (frame.tag === "chat.create") {
				socket.data.session = {
					...fixtureSession,
					id: frame.payload.initialSessionId,
					title: "Created native chat",
					providerId: frame.payload.providerId,
					model: frame.payload.model,
					worktreeId:
						frame.payload.workspacePolicy?._tag === "main"
							? null
							: "fixture-tree",
				};
				socket.data.initialPrompt = frame.payload.initialPrompt;
				reply({
					initialSession: socket.data.session,
					chat: { id: frame.payload.chatId },
					initialMessage: null,
				});
			} else if (frame.tag === "session.setModel") {
				socket.data.session = { ...activeSession, model: frame.payload.model };
				reply(null);
			} else if (
				frame.tag === "settings.get" ||
				frame.tag === "settings.update"
			) {
				socket.data.settings ??= {
					schemaVersion: 1,
					appearanceMode: "system",
					defaultProviderId: "fixture",
					defaultRuntimeMode: "approval-required",
					defaultAutoCreateWorktree: false,
					providerEnabled: { fixture: true },
					providerBinaryPaths: {},
					disabledSkills: [],
				};
				if (frame.tag === "settings.update")
					Object.assign(socket.data.settings, frame.payload.patch);
				reply(socket.data.settings);
			} else if (frame.tag === "workspace.browseDirectory")
				reply({ path: "/workspaces/zuse", parent: "/workspaces", entries: [] });
			else if (frame.tag === "workspace.add")
				reply({
					id: "fixture-project",
					name: "Fixture project",
					path: frame.payload.path,
				});
			else if (frame.tag === "session.events") {
				socket.data.timeline = frame.id;
				socket.send(
					JSON.stringify({
						_tag: "Chunk",
						requestId: frame.id,
						values: [
							{
								kind: "snapshot",
								sessionId: activeSession.id,
								throughVersion: 0,
								cursor: { epoch: "fixture", version: 0 },
								projection: {
									messages: socket.data.initialPrompt
										? [
												{
													id: "initial-prompt",
													sessionId: activeSession.id,
													role: "user",
													content: {
														_tag: "user",
														text: socket.data.initialPrompt,
													},
													createdAt: new Date().toISOString(),
												},
											]
										: [],
									status: "idle",
									interactions: [],
								},
							},
							{
								kind: "synchronized",
								sessionId: activeSession.id,
								throughVersion: 0,
							},
						],
					}),
				);
			} else if (frame.tag === "session.events.head")
				reply({ throughVersion: socket.data.version, streamEpoch: "fixture" });
			else if (frame.tag === "messages.send") {
				const p = frame.payload;
				const text = p.text ?? p.input?.text ?? "";
				const values = [
					{
						id: p.clientMessageId,
						role: "user",
						content: { _tag: "user", text },
					},
					{
						id: crypto.randomUUID(),
						role: "assistant",
						content: { _tag: "assistant", text: `Received: ${text}` },
					},
				].map((message) => ({
					kind: "event",
					sessionId: activeSession.id,
					streamVersion: ++socket.data.version,
					event: {
						_tag: "MessagePersisted",
						message: {
							...message,
							sessionId: activeSession.id,
							createdAt: new Date().toISOString(),
						},
					},
				}));
				socket.send(
					JSON.stringify({
						_tag: "Chunk",
						requestId: socket.data.timeline,
						values,
					}),
				);
				reply(null);
			} else if (frame.tag === "git.reviewPatches") {
				socket.send(
					JSON.stringify({
						_tag: "Chunk",
						requestId: frame.id,
						values: [
							{
								path: "example.swift",
								result: {
									mode: "text",
									patch:
										"--- a/example.swift\n+++ b/example.swift\n@@ -1 +1 @@\n-let value = 1\n+let value = 2",
									truncated: false,
									bytes: 100,
								},
								error: null,
							},
						],
					}),
				);
				reply(null);
			} else if (
				frame.tag === "attachments.upload" ||
				frame.tag === "attachments.uploadChunk"
			) {
				const p = frame.payload;
				const id = p.uploadId ?? crypto.randomUUID();
				const chunks = uploads.get(id) ?? [];
				const bytes = Buffer.from(p.bytes, "base64");
				if (
					p.offset !== undefined &&
					p.offset !== chunks.reduce((n, c) => n + c.length, 0)
				)
					throw new Error("Unexpected chunk offset");
				chunks.push(bytes);
				uploads.set(id, chunks);
				const size = chunks.reduce((n, c) => n + c.length, 0);
				if (p.totalBytes !== undefined && size < p.totalBytes) reply(null);
				else {
					files.set(id, {
						bytes: Buffer.concat(chunks).toString("base64"),
						mimeType: p.mimeType,
						originalName: p.originalName,
					});
					uploads.delete(id);
					reply({
						id,
						mimeType: p.mimeType,
						originalName: p.originalName,
						sizeBytes: size,
					});
				}
			} else if (frame.tag === "attachments.read")
				reply(files.get(frame.payload.id));
			else if (frame.tag === "fixture.echo") reply(frame.payload);
			else if (frame.tag === "fixture.stream") {
				socket.send(
					JSON.stringify({
						_tag: "Chunk",
						requestId: frame.id,
						values: [1, 2, 3],
					}),
				);
				reply(null);
			} else if (frame.tag === "fixture.failure") {
				socket.send(
					JSON.stringify({
						_tag: "Exit",
						requestId: frame.id,
						exit: {
							_tag: "Failure",
							cause: [
								{ _tag: "Fail", error: { message: "Expected rejection" } },
							],
						},
					}),
				);
			} else if (frame.tag === "fixture.close") socket.close();
		},
	},
});
writeFileSync(
	process.env.ZUSE_FIXTURE_PORT_FILE ??
		resolve(import.meta.dir, "../.build/rpc-port"),
	String(server.port),
);
