import { FolderId } from "@zuse/contracts";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { MAIN_SOURCE } from "../../../src/lib/new-chat";

const secureStore = vi.hoisted(() => ({
	value: null as string | null,
	writes: [] as Array<[string, string]>,
}));

vi.mock("expo-secure-store", () => ({
	getItemAsync: () => Promise.resolve(secureStore.value),
	setItemAsync: (key: string, value: string) => {
		secureStore.writes.push([key, value]);
		return Promise.resolve();
	},
}));

import {
	clearNewChatScreenState,
	DEFAULT_NEW_CHAT_PREFERENCES,
	type NewChatScreenState,
	newChatScreenState,
	readNewChatPreferences,
	saveNewChatPreferences,
	saveNewChatScreenState,
} from "../../../src/store/new-chat-preferences";

describe("new chat preferences", () => {
	beforeEach(() => {
		secureStore.value = null;
		secureStore.writes = [];
	});

	test("restores machine, repository, and work mode", async () => {
		secureStore.value = JSON.stringify({
			connectionKey: "machine-1",
			projectId: "project-1",
			sourceKind: "worktree",
		});

		await expect(readNewChatPreferences()).resolves.toEqual({
			connectionKey: "machine-1",
			projectId: "project-1",
			sourceKind: "worktree",
		});
	});

	test("falls back safely for malformed preferences", async () => {
		secureStore.value = JSON.stringify({
			connectionKey: 42,
			projectId: [],
			sourceKind: "branch-name-that-must-not-be-saved",
		});

		await expect(readNewChatPreferences()).resolves.toEqual(
			DEFAULT_NEW_CHAT_PREFERENCES,
		);
	});

	test("persists only defaults, never a concrete branch selection", async () => {
		await saveNewChatPreferences({
			connectionKey: "machine-2",
			projectId: FolderId.make("project-2"),
			sourceKind: "pr",
		});

		const encoded = secureStore.writes[0]?.[1];
		expect(encoded).toBeDefined();
		expect(JSON.parse(encoded ?? "{}")).toEqual({
			connectionKey: "machine-2",
			projectId: "project-2",
			sourceKind: "pr",
		});
	});
});

describe("new chat screen state", () => {
	const state: NewChatScreenState = {
		connectionKey: "machine-1",
		projectId: FolderId.make("project-1"),
		source: MAIN_SOURCE,
		sourceKind: "branch",
		modelMode: {
			providerId: "codex",
			model: "gpt-5",
			runtimeMode: "approval-required",
			permissionMode: "plan",
			modelOptions: {},
		},
		cloudSandbox: null,
		cloudProjectId: null,
	};

	beforeEach(() => {
		secureStore.writes = [];
	});

	test("restores choices for the same route after a layout remount", () => {
		saveNewChatScreenState("route-a", state);
		expect(newChatScreenState("route-a")).toEqual(state);
		expect(newChatScreenState("route-b")).toBeUndefined();
	});

	test("forgets choices once the route is removed", () => {
		saveNewChatScreenState("route-c", state);
		clearNewChatScreenState("route-c");
		expect(newChatScreenState("route-c")).toBeUndefined();
	});

	test("keeps in-progress choices in memory only", () => {
		saveNewChatScreenState("route-d", state);
		expect(secureStore.writes).toEqual([]);
	});
});
