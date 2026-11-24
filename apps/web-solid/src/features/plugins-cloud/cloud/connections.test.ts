import assert from "node:assert/strict";
import { test } from "node:test";
import { connectionName, launchConnections, orderedConnections, type AgentConnection } from "./connections";

const row = (connectionId: string, data: Partial<AgentConnection> = {}): AgentConnection => ({
  connectionId, agent: "claude", method: "api-key", state: "connected", ...data,
});

test("connection settings follows server priority and keeps disabled and signed-out rows visible", () => {
  const rows = [row("old"), row("two", { position: 2, enabled: false }), row("one", { position: 1, state: "reconnect-required" }), row("hidden", { position: 0, state: "revoked" })];
  assert.deepEqual(orderedConnections(rows).map((item) => item.connectionId), ["one", "two", "old"]);
  assert.equal(rows.length, 4);
});

test("cloud launches only offer connected, enabled, personal accounts for the selected harness", () => {
  const rows = [
    row("second", { position: 5 }), row("first", { position: 3 }), row("off", { position: 0, enabled: false }),
    row("pending", { position: 1, state: "pending" }), row("org", { position: 2, connectionScope: "organization" }),
    row("codex", { position: 4, agent: "codex" }),
  ];
  assert.deepEqual(launchConnections(rows, "claude-code").map((item) => item.connectionId), ["first", "second"]);
  assert.deepEqual(launchConnections(rows, "codex").map((item) => item.connectionId), ["codex"]);
  assert.deepEqual(launchConnections(rows, undefined), []);
});

test("an alias takes precedence over the provider's account name", () => {
  assert.equal(connectionName(row("a", { alias: "Personal", accountLabel: "user@example.test" })), "Personal");
  assert.equal(connectionName(row("a", { alias: "", accountLabel: "user@example.test" })), "user@example.test");
  assert.equal(connectionName(row("a")), "Claude");
});
