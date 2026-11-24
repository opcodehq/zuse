// Run with `bun test src`. Cloud error copy and startup progress, so raw codes never reach the UI.
import { test } from "node:test";
import assert from "node:assert/strict";
import { cloudErrorText, cloudIssue, GENERIC_ISSUE } from "./errors";
import { elapsed, phaseText, rowIssue, startupLabel, stateBadge } from "./progress";
import type { Workspace } from "./types";

const row = (extra: Partial<Workspace>): Workspace => ({ workspaceId: "w", chatId: "c", initialSessionId: "s", branch: "b", state: "ready", desiredState: "ready", statusCode: "", revision: 1, ...extra });

test("known codes read as sentences and unknown codes fall back to generic copy", () => {
  assert.match(cloudErrorText("cloud_runtime_launch_incompatible"), /couldn't restart|runtime/i);
  assert.match(cloudErrorText("Cloud startup failed (cloud_provider_rejected). Retry after recovering the workspace."), /retry the original start/);
  assert.equal(cloudErrorText("cloud_something_brand_new"), GENERIC_ISSUE.detail);
  assert.equal(cloudErrorText("Cloud service is unavailable. Try again."), "Cloud service is unavailable. Try again.");
  assert.doesNotMatch(cloudErrorText("request failed: cloud_usage_hold"), /cloud_/);
});

test("agent authorization codes ask to reconnect the agent", () => {
  for (const code of ["agent_connection_reconnect_required", "codex-auth-reconnect-required", "credential_preparation_failed"]) assert.equal(cloudIssue(code)?.recovery, "agent", code);
  assert.notEqual(cloudIssue("cloud_provider_rejected")?.recovery, "agent");
  assert.match(cloudErrorText("start failed: claude-auth-reconnect-required"), /Reconnect the coding agent/);
});

test("startup label follows desktop's steps", () => {
  assert.equal(startupLabel(row({ state: "provisioning", startupPhase: "allocating" })), "Starting computer");
  assert.equal(startupLabel(row({ state: "provisioning", startupPhase: "resuming" })), "Resuming computer");
  assert.equal(startupLabel(row({ state: "setup", startupPhase: "syncing-repository" })), "Syncing repository");
  assert.equal(startupLabel(row({ state: "setup", preparation: { credentials: { state: "running" } } })), "Preparing agent access");
  assert.equal(startupLabel(row({ state: "setup", startupPhase: "starting-agent" })), "Starting agent");
  assert.equal(startupLabel(row({ state: "ready" })), null);
  assert.equal(phaseText("Resuming cloud workspace…"), "Resuming computer");
  assert.equal(phaseText("Syncing repository…"), "Syncing repository");
});

test("rows that stopped carry a mapped reason", () => {
  assert.equal(rowIssue(row({ state: "provisioning", statusCode: "cloud_provider_rejected", allocationRecovery: { operationId: "op", errorCode: "cloud_provider_rejected", retryAvailable: true } }))?.title, "Computer start was rejected");
  assert.equal(rowIssue(row({ state: "paused", statusCode: "cloud_lifecycle_timed_out" }))?.title, "The computer couldn't wake up");
  assert.equal(rowIssue(row({ state: "paused", statusCode: "paused" })), null);
  assert.equal(rowIssue(row({ state: "failed", preparation: { repository: { state: "failed", errorCode: "repository_preparation_failed" } } }))?.recovery, "github");
  assert.equal(rowIssue(row({ state: "blocked", statusCode: "" })), GENERIC_ISSUE);
  assert.equal(cloudIssue("agent_connection_required")?.recovery, "agent");
});

test("badges cover every lifecycle state with a neutral fallback", () => {
  assert.deepEqual(stateBadge(row({ state: "resuming" })), { tone: "warn", label: "Waking" });
  assert.deepEqual(stateBadge(row({ state: "provisioning", startupPhase: "resuming" })), { tone: "warn", label: "Waking" });
  assert.deepEqual(stateBadge(row({ state: "archiving" })), { tone: "warn", label: "Archiving" });
  assert.deepEqual(stateBadge(row({ state: "deleting" })), { tone: "warn", label: "Deleting" });
  assert.deepEqual(stateBadge(row({ state: "something-new" })), { tone: "outline", label: "Updating" });
  assert.deepEqual(stateBadge(row({ state: "paused", statusCode: "cloud_retained_provider_failed" })), { tone: "error", label: "Couldn't wake" });
});

test("elapsed time", () => {
  assert.equal(elapsed(0), "0:00");
  assert.equal(elapsed(42_500), "0:42");
  assert.equal(elapsed(725_000), "12:05");
  assert.equal(elapsed(3_723_000), "1:02:03");
});
