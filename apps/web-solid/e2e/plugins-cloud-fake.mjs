// A test-only stand-in for the Zuse plugin and cloud services, installed in the page before the app loads.
// A disposable engine cannot sign in to a Zuse account, so the account, plugin and cloud methods are answered
// here with the same wrappers the engine adds (crates/engine/src/cloud.rs, rpc.rs). Every other request reaches
// the real engine. Tests steer it through `window.__fake`: `fail[method]`, `hold[method]`, `push()`, `drop()`.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { repo } from "./harness.mjs";

const bundled = JSON.parse(readFileSync(join(repo, "crates/protocol/src/plugin-catalog.json"), "utf8"));

export function fakeSource({ subscribed = true } = {}) {
  const seed = { bundled, subscribed, now: Date.now() };
  return `(${install.toString()})(${JSON.stringify(seed)});`;
}

/* eslint-disable */
function install(seed) {
  const Native = window.WebSocket;
  const now = seed.now;
  const day = 86400000;
  const ACCOUNT = "user_fixture";
  const fake = (window.__fake = { log: [], fail: {}, hold: {}, held: [], opened: [], sockets: [], offline: false });
  window.open = (url) => (fake.opened.push(String(url)), null);

  const extra = Array.from({ length: 30 }, (_, i) => ({ id: `tool-${i + 1}`, name: `Fixture tool ${String(i + 1).padStart(2, "0")}`, description: "A fixture integration for paging.", domain: `tool-${i + 1}.example.test`, category: "fixtures" }));
  const plugins = {
    tenant: `personal:${ACCOUNT}`,
    tenants: [{ id: `personal:${ACCOUNT}`, kind: "personal", name: "Personal" }, { id: "organization:acme", kind: "organization", name: "Acme" }],
    catalog: [...seed.bundled, { id: "keyed", name: "Keyed service", description: "Uses an API key instead of a hosted sign-in.", domain: "keyed.example.test", category: "developer-tools", apiKeyHeader: "Authorization" }, ...extra],
    connections: [{ id: "conn-notion", pluginId: "notion", label: "Notion", owner: ACCOUNT, state: "connected", createdAt: now - 3 * day, enabled: true }],
    attempts: {},
    seq: 1,
  };
  const repos = [
    { nameWithOwner: "octo/web", description: "Web app", sshUrl: "git@github.com:octo/web.git", httpsUrl: "https://github.com/octo/web.git", isPrivate: true, defaultBranch: "main", updatedAt: "2026-10-01T00:00:00Z", ownerAvatarUrl: null },
    { nameWithOwner: "octo/api", description: "API server", sshUrl: "git@github.com:octo/api.git", httpsUrl: "https://github.com/octo/api.git", isPrivate: false, defaultBranch: "trunk", updatedAt: "2026-10-02T00:00:00Z", ownerAvatarUrl: null },
    { nameWithOwner: "octo/docs", description: "Documentation", sshUrl: "git@github.com:octo/docs.git", httpsUrl: "https://github.com/octo/docs.git", isPrivate: false, defaultBranch: "main", updatedAt: "2026-10-03T00:00:00Z", ownerAvatarUrl: null },
  ];
  const project = (repo, id) => ({ projectId: id, repositoryIdentity: `github.com/${repo.nameWithOwner}`, repositoryUrl: repo.httpsUrl, displayName: repo.nameWithOwner, defaultBranch: repo.defaultBranch, visibility: repo.isPrivate ? "private" : "public", state: "ready", createdAt: now - 10 * day, updatedAt: now - 10 * day });
  const build = (id, state, extra = {}) => ({ buildId: id, state, mode: "update", active: state === "ready", progressPhase: null, errorCode: null, logText: `[${id}] cloning repositories\n[${id}] installing agents\n[${id}] snapshot saved`, runtimeVersion: "2026.10.1", configurationDigest: "sha256:fixture", repositories: [{ projectId: "proj-web", repositoryIdentity: "github.com/octo/web", displayName: "octo/web", defaultBranch: "main", sourceCommit: "abc123" }], createdAt: now - 2 * day, updatedAt: now - 2 * day + 95000, ...extra });
  const cloud = {
    subscribed: seed.subscribed,
    cap: 50_000_000,
    projects: [project(repos[0], "proj-web")],
    image: {
      storage: { snapshotId: "snap-1", state: "retained", monthlyCostMicros: 1_500_000, billingEnabled: true, lastError: null },
      state: "ready", generation: "g1", providerId: "boxd", runtimeVersion: "2026.10.1", progressPhase: null, errorCode: null,
      repositories: [{ projectId: "proj-web", repositoryIdentity: "github.com/octo/web", displayName: "octo/web", defaultBranch: "main", sourceCommit: "abc123" }],
      builds: [build("b-7", "ready"), ...Array.from({ length: 7 }, (_, i) => build(`b-${6 - i}`, i === 1 ? "failed" : "ready", { active: false, errorCode: i === 1 ? "build-timeout" : null, createdAt: now - (3 + i) * day, updatedAt: now - (3 + i) * day + 60000 }))],
      builtAt: now - 2 * day, updatedAt: now - 2 * day,
    },
    workspaces: [
      { workspaceId: "ws-run", chatId: "chat-run", initialSessionId: "s-run", branch: "zuse/fix-login", state: "running", desiredState: "running", statusCode: "ready", revision: 3, title: "Fix the login redirect", rpcProtocol: "zuse-rpc-v1" },
      { workspaceId: "ws-arch", chatId: "chat-arch", initialSessionId: "s-arch", branch: "zuse/old-work", state: "archived", desiredState: "archived", statusCode: "archived", revision: 9, title: "Archived refactor", rpcProtocol: "zuse-rpc-v1",
        archiveStatus: { workspaceId: "ws-arch", archived: true, machineRetained: true, backupVerified: true, computeRetired: false, retentionDeadline: now + 5 * day, phase: "retained", verifiedChunks: 12, totalChunks: 12, errorCode: null } },
      { workspaceId: "ws-clean", chatId: "chat-clean", initialSessionId: "s-clean", branch: "zuse/cleanup", state: "deleted", desiredState: "deleted", statusCode: "deleted", revision: 4, title: "Cleanup pending", rpcProtocol: "zuse-rpc-v1", localMirrorCleanup: { state: "error", removed: 2, preserved: 1, error: "Local files are in use." } },
    ],
    usage: Array.from({ length: 14 }, (_, i) => ({ entryId: `u-${i}`, resourceKind: "sandbox", usageKind: "sandbox-run", resourceId: `ws-${i}`, provider: "boxd", startedAt: now - (i + 1) * 3600000, endedAt: now - i * 3600000, vcpuCount: 4, memoryMib: 8192, providerCostMicros: 120_000 + i * 1000, status: i === 0 ? "provisional" : "final" })),
    seq: 1,
  };
  const transcript = (id, count) => Array.from({ length: count }, (_, i) => ({ id: `${id}-m${i}`, role: i % 2 ? "assistant" : "user", createdAt: now - (count - i) * 60000, deviceId: "cloud", parts: [{ kind: "text", id: `${id}-p${i}`, text: i % 2 ? `Reply ${i}: updated **the redirect** handling.\n\n- step one\n- step two` : `Request ${i}: please look at the login flow.` }] }));

  const ok = (data) => ({ accountId: ACCOUNT, organizationId: null, data });
  const wsReply = (row) => ({ accountId: ACCOUNT, workspace: row });
  const snapshot = () => ({ kind: "snapshot", canManage: plugins.tenant.startsWith("personal:"), tenants: plugins.tenants, tenantId: plugins.tenant, endpoint: "https://plugins.example.test", catalog: plugins.catalog, connections: plugins.tenant.startsWith("personal:") ? plugins.connections : [] });
  const imageState = () => {
    const included = cloud.image.repositories.map((r) => r.projectId).sort().join();
    const now = cloud.projects.map((p) => p.projectId).sort().join();
    if (cloud.image.state === "ready" && included !== now) cloud.image.state = "outdated";
    if (cloud.image.state === "outdated" && included === now) cloud.image.state = "ready";
    return cloud.image;
  };
  const summary = () => ({ currency: "usd", status: "active", periodStart: now - 10 * day, periodEnd: now + 20 * day, basePriceMicros: 40_000_000, includedProviderCostMicros: 35_000_000, providerCostMicros: 12_340_000, storageCostMicros: 750_000, includedUsedMicros: 12_340_000, includedRemainingMicros: 22_660_000, overageProviderCostMicros: 0, overageChargeMicros: 0, overageCapMicros: cloud.cap, markupBasisPoints: 500, currentInvoiceEstimateMicros: 40_000_000, usageProvisional: true });

  const handlers = {
    CloudAccountStatus: () => fake.account,
    CloudAccountSignOut: () => { fake.account = { user: null, login: null, error: null, orgId: null }; fake.push(); return { ok: true }; },
    CloudAccountSignIn: () => { const login = { url: "https://auth.example.test/device", userCode: "ABCD-EFGH", expiresAt: Math.floor(now / 1000) + 600 }; fake.account = { ...fake.account, login }; fake.push(); return login; },
    SelectPluginWorkspace: (p) => { if (!plugins.tenants.some((t) => t.id === p.tenantId)) throw new Error("Choose a plugin workspace."); plugins.tenant = p.tenantId; return snapshot(); },
    Plugins: (p) => {
      switch (p.action) {
        case "list": return snapshot();
        case "connect": {
          const def = plugins.catalog.find((d) => d.id === p.pluginId);
          const id = `conn-${plugins.seq++}`;
          if (def?.apiKeyHeader) {
            if (!p.apiKey || !p.apiKey.startsWith("Bearer ")) throw new Error("The API key was rejected.");
            plugins.connections.push({ id, pluginId: p.pluginId, label: p.label, owner: ACCOUNT, state: "connected", createdAt: Date.now(), enabled: true });
            return { kind: "attempt", id: `att-${id}`, connectionId: id, state: "connected", authorizationUrl: null, expiresAt: Date.now() / 1000 + 600, pluginId: p.pluginId };
          }
          const attempt = { kind: "attempt", id: `att-${id}`, connectionId: id, state: "pending", authorizationUrl: `https://auth.example.test/${p.pluginId}`, expiresAt: Date.now() / 1000 + 600, pluginId: p.pluginId, polls: 0, label: p.label };
          plugins.attempts[attempt.id] = attempt;
          const { polls, label, ...wire } = attempt;
          return wire;
        }
        case "poll": {
          const attempt = plugins.attempts[p.attemptId];
          if (!attempt) throw new Error("This sign-in is no longer active.");
          attempt.polls++;
          if (attempt.polls >= (fake.pollsToConnect ?? 2)) {
            attempt.state = "connected";
            plugins.connections.push({ id: attempt.connectionId, pluginId: attempt.pluginId, label: attempt.label, owner: ACCOUNT, state: "connected", createdAt: Date.now(), enabled: true });
            delete plugins.attempts[p.attemptId];
          }
          const { polls, label, ...wire } = attempt;
          return wire;
        }
        case "cancel": delete plugins.attempts[p.attemptId]; return { kind: "ok" };
        case "setEnabled": { const row = plugins.connections.find((c) => c.id === p.connectionId); if (!row) throw new Error("Connection not found."); row.enabled = p.enabled; return { kind: "ok" }; }
        case "disconnect": plugins.connections = plugins.connections.filter((c) => c.id !== p.connectionId); return { kind: "ok" };
        case "addServer": {
          const id = `custom-${plugins.seq++}`;
          plugins.catalog.unshift({ id, name: p.name, description: "", domain: new URL(p.url).host, custom: true, url: p.url, apiKeyHeader: p.apiKey?.header ?? null });
          const conn = `conn-${plugins.seq++}`;
          plugins.connections.push({ id: conn, pluginId: id, label: p.name, owner: ACCOUNT, state: "connected", createdAt: Date.now(), enabled: true });
          return { kind: "attempt", id: `att-${conn}`, connectionId: conn, state: "connected", authorizationUrl: null, expiresAt: Date.now() / 1000 + 600, pluginId: id };
        }
        case "addLocalServer": {
          const id = `local-${plugins.seq++}`;
          plugins.catalog.unshift({ id, name: p.name, description: "", domain: "", custom: true, command: p.command });
          const conn = `conn-${plugins.seq++}`;
          plugins.connections.push({ id: conn, pluginId: id, label: p.name, owner: ACCOUNT, state: "connected", createdAt: Date.now(), enabled: true });
          return { kind: "attempt", id: `att-${conn}`, connectionId: conn, state: "connected", authorizationUrl: null, expiresAt: Date.now() / 1000 + 600, pluginId: id };
        }
        case "removeServer": plugins.catalog = plugins.catalog.filter((d) => d.id !== p.pluginId); plugins.connections = plugins.connections.filter((c) => c.pluginId !== p.pluginId); return { kind: "ok" };
      }
      throw new Error("Unsupported plugin action.");
    },
    CloudBillingEntitlements: () => ok({ entitlements: cloud.subscribed ? [{ offerId: "cloud-workspace-standard-v1", status: "active", paidThrough: now + 20 * day }] : [] }),
    CloudBillingSummary: () => { if (!cloud.subscribed) throw new Error("An active cloud billing period is required."); return ok(summary()); },
    CloudBillingUsage: (p) => { if (!cloud.subscribed) throw new Error("An active cloud billing period is required."); const start = p.cursor ? Number(p.cursor) : 0; const items = cloud.usage.slice(start, start + p.limit); return ok({ items, nextCursor: start + p.limit < cloud.usage.length ? String(start + p.limit) : null }); },
    CloudBillingSetCap: (p) => { cloud.cap = p.overageCapMicros; return ok(summary()); },
    CloudBillingCheckout: (p) => ok({ checkoutUrl: `https://checkout.example.test/${p.offerId}` }),
    CloudBillingPortal: () => ok({ portalUrl: "https://billing.example.test/portal" }),
    CloudPrepaidBalance: () => ok({ available: true, creditCents: 1234, debitCents: 0, currency: "usd" }),
    CloudPrepaidCheckout: (p) => ok({ checkoutUrl: `https://checkout.example.test/credits/${p.amountCents}` }),
    CloudProjects: () => ok({ projects: cloud.projects }),
    CloudGithubCatalog: () => ok({ configured: true, canManageInstallations: true, user: { login: "octo", name: "Octo Cat", email: "octo@example.test" }, installations: [{ installationId: 7, accountLogin: "octo", accountType: "User", avatarUrl: null, repositorySelection: "selected", suspended: false }], repositories: repos }),
    CloudProjectConnect: (p) => { const repo = repos.find((r) => r.httpsUrl === p.repositoryUrl); if (!repo) throw new Error("Check the repository URL, branch and environment settings."); const row = project(repo, `proj-${cloud.seq++}`); row.updatedAt = Date.now(); cloud.projects.push(row); return ok(row); },
    CloudProjectRemove: (p) => { const row = cloud.projects.find((r) => r.projectId === p.projectId); cloud.projects = cloud.projects.filter((r) => r.projectId !== p.projectId); return ok(row); },
    CloudImage: () => ok(imageState()),
    CloudImageBuild: (p) => { cloud.image.state = "building"; cloud.image.progressPhase = "queued"; cloud.image.builds.unshift(build(`b-${8 + cloud.seq++}`, "building", { mode: p.mode, active: false, logText: null, progressPhase: "prepare", createdAt: Date.now(), updatedAt: Date.now() })); fake.buildFinishes = Date.now() + 2500; return ok(cloud.image); },
    CloudImageDelete: () => { cloud.image.storage = { ...cloud.image.storage, state: "deleting" }; setTimeout(() => (cloud.image.storage = null), 2500); return ok(cloud.image); },
    CloudContext: () => ({ accountId: ACCOUNT, projects: cloud.projects.map((p) => ({ projectId: p.projectId, providerId: "boxd", displayName: p.displayName, defaultBranch: p.defaultBranch, prepared: cloud.image.repositories.some((r) => r.projectId === p.projectId) })), agentCatalog: { providers: [{ id: "claude-code", name: "Claude Code", defaultModel: "default", models: [{ id: "default", label: "Default" }, { id: "fast", label: "Fast" }] }] } }),
    CloudCreate: (p) => { const row = { workspaceId: `ws-new-${cloud.seq++}`, chatId: "chat-new", initialSessionId: "s-new", branch: p.branch ?? "zuse/new-chat", state: "starting", desiredState: "running", statusCode: "starting", revision: 1, title: null, rpcProtocol: "zuse-rpc-v1" }; cloud.workspaces.unshift(row); setTimeout(() => (row.state = "running"), 4000); return { accountId: ACCOUNT, workspace: { workspace: row } }; },
    CloudWorkspaces: () => ({ accountId: ACCOUNT, workspaces: cloud.workspaces.filter((w) => w.rpcProtocol === "zuse-rpc-v1") }),
    CloudHistory: (p) => {
      const row = cloud.workspaces.find((w) => w.workspaceId === p.workspaceId);
      if (!row) throw new Error("Select a workspace.");
      const session = p.sessionId ?? row.initialSessionId;
      const conversations = [{ id: row.initialSessionId, title: "Main" }, { id: `${row.workspaceId}-side`, title: "Side quest" }];
      return { accountId: ACCOUNT, workspace: row, head: { schemaVersion: 1, payloadFormat: "json", workspaceId: row.workspaceId, sessionId: session, runtimeGeneration: 1, cursor: {}, chat: { id: row.chatId, title: row.title }, conversations, transcript: { reset: transcript(session, session.endsWith("-side") ? 4 : 40) }, queue: null, queuePaused: false, olderMessageSequence: null } };
    },
    CloudPause: (p) => { const row = cloud.workspaces.find((w) => w.workspaceId === p.workspaceId); row.state = "paused"; row.desiredState = "paused"; return wsReply(row); },
    CloudUnarchive: (p) => { const row = cloud.workspaces.find((w) => w.workspaceId === p.workspaceId); Object.assign(row, { state: "paused", desiredState: "paused", statusCode: "paused", archiveStatus: null }); return wsReply(row); },
    CloudDelete: (p) => { const row = cloud.workspaces.find((w) => w.workspaceId === p.workspaceId); Object.assign(row, { desiredState: "deleted", statusCode: "delete-provider-cleanup", archiveStatus: null }); setTimeout(() => Object.assign(row, { state: "deleted", localMirrorCleanup: { state: "done", removed: 3, preserved: 0 } }), 3500); return wsReply(row); },
    CloudCleanupMirror: (p) => { const row = cloud.workspaces.find((w) => w.workspaceId === p.workspaceId); row.localMirrorCleanup = { state: "done", removed: 3, preserved: 0 }; return wsReply(row); },
  };
  fake.account = { user: { id: ACCOUNT, email: "fixture@example.test", name: "Fixture User" }, login: null, error: null, orgId: null };
  fake.plugins = plugins;
  fake.cloud = cloud;
  setInterval(() => {
    if (fake.buildFinishes && Date.now() > fake.buildFinishes) {
      fake.buildFinishes = 0;
      const b = cloud.image.builds[0];
      Object.assign(b, { state: "ready", active: true, logText: "[new] cloning\n[new] snapshot saved", updatedAt: Date.now(), repositories: cloud.projects.map((p) => ({ projectId: p.projectId, repositoryIdentity: p.repositoryIdentity, displayName: p.displayName, defaultBranch: p.defaultBranch, sourceCommit: "def456" })) });
      cloud.image.builds.slice(1).forEach((x) => (x.active = false));
      Object.assign(cloud.image, { state: "ready", progressPhase: null, repositories: b.repositories, builtAt: Date.now(), updatedAt: Date.now() });
    }
  }, 250);

  const streams = new Map();
  fake.push = () => { for (const [id, socket] of streams) socket.__deliver({ id, item: fake.account }); };
  fake.drop = () => { const socket = fake.sockets.at(-1); socket?.close(); };
  window.WebSocket = class extends Native {
    constructor(url, protocols) {
      super(fake.offline ? "ws://127.0.0.1:9/" : url, protocols);
      fake.sockets.push(this);
      this.addEventListener("close", () => { for (const [id, s] of streams) if (s === this) streams.delete(id); });
    }
    __deliver(frame) {
      if (this.readyState === 1) this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(frame) }));
    }
    send(text) {
      let frame;
      try { frame = JSON.parse(text); } catch { return super.send(text); }
      if (frame.cancel) { if (streams.delete(frame.id)) return; return super.send(text); }
      const handler = handlers[frame.method];
      if (!handler) return super.send(text);
      const params = JSON.parse(JSON.stringify(frame.params ?? {}));
      fake.log.push({ method: frame.method, params });
      if (frame.method === "CloudAccountStatus") {
        streams.set(frame.id, this);
        setTimeout(() => this.__deliver({ id: frame.id, item: fake.account }), 10);
        return;
      }
      const answer = () => {
        const failure = fake.fail[frame.method];
        if (failure) {
          if (typeof failure === "string") return this.__deliver({ id: frame.id, err: failure });
          if (failure.once) delete fake.fail[frame.method];
          return this.__deliver({ id: frame.id, err: failure.message });
        }
        try { this.__deliver({ id: frame.id, ok: JSON.parse(JSON.stringify(handler(params))) }); }
        catch (error) { this.__deliver({ id: frame.id, err: error.message }); }
      };
      if (fake.hold[frame.method]) { fake.held.push({ method: frame.method, answer }); return; }
      setTimeout(answer, fake.latency ?? 60);
    }
  };
}
