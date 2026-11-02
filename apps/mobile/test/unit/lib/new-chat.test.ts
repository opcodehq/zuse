import { describe, expect, test } from "vitest";

import {
	buildNewChatCreatePayload,
	MAIN_SOURCE,
	reconcileNewChatSource,
	sourceOptionsForKind,
} from "../../../src/lib/new-chat";

describe("new chat helper", () => {
	test("does not create without prompt or project", () => {
		const base = {
			connectionKey: "env-1",
			projectId: "project-1" as never,
			providerId: "codex" as const,
			model: "gpt-5-codex",
			runtimeMode: "approval-required" as const,
			permissionMode: "default" as const,
			source: MAIN_SOURCE,
		};

		expect(buildNewChatCreatePayload({ ...base, text: "   " })).toBeNull();
		expect(
			buildNewChatCreatePayload({ ...base, projectId: null, text: "hello" }),
		).toBeNull();
	});

	test("builds payload from selected options", () => {
		const payload = buildNewChatCreatePayload({
			connectionKey: "env-1",
			projectId: "project-1" as never,
			providerId: "claude",
			model: "claude-sonnet-5",
			runtimeMode: "full-access",
			permissionMode: "plan",
			modelOptions: { effort: "high" },
			source: {
				kind: "branch",
				label: "feature",
				worktreeId: null,
				createSource: { _tag: "branch", branch: "feature", remote: "origin" },
			},
			text: "  build it  ",
		});

		expect(payload).toMatchObject({
			projectId: "project-1",
			providerId: "claude",
			model: "claude-sonnet-5",
			runtimeMode: "full-access",
			permissionMode: "plan",
			modelOptions: { effort: "high" },
			initialPrompt: "build it",
			createWorktree: true,
			createSource: { _tag: "branch", branch: "feature", remote: "origin" },
		});
	});

	test("creates a fresh worktree from the default branch", () => {
		const payload = buildNewChatCreatePayload({
			connectionKey: "env-1",
			projectId: "project-1" as never,
			providerId: "codex",
			model: "gpt-5-codex",
			runtimeMode: "approval-required",
			permissionMode: "default",
			source: {
				kind: "worktree",
				label: "main",
				worktreeId: null,
			},
			text: "build it",
		});

		expect(payload).toMatchObject({
			worktreeId: null,
			createWorktree: true,
			createSource: null,
		});
	});

	test("sourceOptionsForKind builds per-kind source objects", () => {
		const branches = [
			{ kind: "local", name: "main", current: true, remote: null },
			{ kind: "local", name: "feature-b", current: false, remote: "origin" },
		] as never;
		const prs = [
			{ number: 7, title: "Fix bug", headRefName: "fix-bug" },
		] as never;

		expect(sourceOptionsForKind("main", branches, prs)).toEqual([
			{ key: "main", label: MAIN_SOURCE.label, source: MAIN_SOURCE },
		]);

		expect(sourceOptionsForKind("worktree", branches, prs)).toEqual([
			{
				key: "new-worktree:Default branch",
				label: "Default branch",
				source: {
					kind: "worktree",
					label: "Default branch",
					worktreeId: null,
				},
			},
		]);

		const branchOpts = sourceOptionsForKind("branch", branches, prs);
		expect(branchOpts).toHaveLength(1);
		expect(branchOpts[0]?.source).toMatchObject({
			kind: "branch",
			label: "feature-b",
			createSource: { _tag: "branch", branch: "feature-b", remote: "origin" },
		});

		const prOpts = sourceOptionsForKind("pr", branches, prs);
		expect(prOpts[0]).toMatchObject({
			key: "pr:7",
			label: "#7 Fix bug",
			source: {
				kind: "pr",
				label: "#7",
				createSource: { _tag: "pr", number: 7, headRefName: "fix-bug" },
			},
		});
	});
});

describe("reconcileNewChatSource", () => {
	const branches = [
		{ kind: "local", name: "main", current: true, remote: null },
		{ kind: "local", name: "feature-b", current: false, remote: "origin" },
		{ kind: "local", name: "feature-c", current: false, remote: "origin" },
	] as never;
	const choices = sourceOptionsForKind("branch", branches, []);
	const restored = choices[1]?.source ?? MAIN_SOURCE;

	test("keeps a restored branch while choices are still loading", () => {
		expect(
			reconcileNewChatSource({
				source: restored,
				sourceKind: "branch",
				choices: [],
				choicesLoaded: false,
			}),
		).toBe(restored);
	});

	test("keeps the restored branch once it appears in the loaded choices", () => {
		expect(
			reconcileNewChatSource({
				source: restored,
				sourceKind: "branch",
				choices,
				choicesLoaded: true,
			}),
		).toBe(restored);
	});

	test("falls back to the first choice only when the branch is gone", () => {
		expect(
			reconcileNewChatSource({
				source: restored,
				sourceKind: "branch",
				choices: choices.slice(0, 1),
				choicesLoaded: true,
			}),
		).toBe(choices[0]?.source);
	});

	test("resets to the main checkout when local work is chosen", () => {
		expect(
			reconcileNewChatSource({
				source: restored,
				sourceKind: "main",
				choices: [],
				choicesLoaded: false,
			}),
		).toBe(MAIN_SOURCE);
	});
});
