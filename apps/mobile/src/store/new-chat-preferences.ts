import { type Folder, FolderId } from "@zuse/contracts";
import * as SecureStore from "expo-secure-store";

import type { ModelModeValue } from "~/lib/model-options";
import type { NewChatSource, NewChatSourceKind } from "~/lib/new-chat";

const STORE_KEY = "zuse.new-chat-preferences.v1";

export type NewChatPreferences = Readonly<{
	connectionKey: string | null;
	projectId: Folder["id"] | null;
	sourceKind: NewChatSourceKind;
}>;

export const DEFAULT_NEW_CHAT_PREFERENCES: NewChatPreferences = {
	connectionKey: null,
	projectId: null,
	sourceKind: "main",
};

const isSourceKind = (value: unknown): value is NewChatSourceKind =>
	value === "main" ||
	value === "worktree" ||
	value === "branch" ||
	value === "pr";

const nullableId = (value: unknown): string | null | undefined =>
	value === null || typeof value === "string" ? value : undefined;

const nullableProjectId = (value: unknown): Folder["id"] | null | undefined =>
	value === null
		? null
		: typeof value === "string"
			? FolderId.make(value)
			: undefined;

const decodePreferences = (value: unknown): NewChatPreferences => {
	if (typeof value !== "object" || value === null)
		return DEFAULT_NEW_CHAT_PREFERENCES;
	const connectionKey = nullableId(Reflect.get(value, "connectionKey"));
	const projectId = nullableProjectId(Reflect.get(value, "projectId"));
	const sourceKind = Reflect.get(value, "sourceKind");
	if (
		connectionKey === undefined ||
		projectId === undefined ||
		!isSourceKind(sourceKind)
	) {
		return DEFAULT_NEW_CHAT_PREFERENCES;
	}
	return { connectionKey, projectId, sourceKind };
};

export const readNewChatPreferences = async (): Promise<NewChatPreferences> => {
	try {
		const raw = await SecureStore.getItemAsync(STORE_KEY);
		return raw === null
			? DEFAULT_NEW_CHAT_PREFERENCES
			: decodePreferences(JSON.parse(raw));
	} catch {
		return DEFAULT_NEW_CHAT_PREFERENCES;
	}
};

let writeTail = Promise.resolve();

export const saveNewChatPreferences = (
	preferences: NewChatPreferences,
): Promise<void> => {
	writeTail = writeTail
		.catch(() => undefined)
		.then(() =>
			SecureStore.setItemAsync(STORE_KEY, JSON.stringify(preferences)),
		);
	return writeTail;
};

/** A New Chat screen's in-progress choices. */
export type NewChatScreenState = Readonly<{
	connectionKey: string | null;
	projectId: Folder["id"] | null;
	source: NewChatSource;
	sourceKind: NewChatSourceKind;
	modelMode: ModelModeValue;
	cloudSandbox: string | null;
	cloudProjectId: string | null;
}>;

/**
 * In-memory, keyed by navigation route key. A layout change (iPad resize or
 * iPhone Duo fold) remounts the screen under the same route and restores these
 * choices; a fresh New Chat gets a new route key and never sees stale ones.
 */
const screenStates = new Map<string, NewChatScreenState>();

export const newChatScreenState = (
	routeKey: string,
): NewChatScreenState | undefined => screenStates.get(routeKey);

export const saveNewChatScreenState = (
	routeKey: string,
	state: NewChatScreenState,
): void => {
	screenStates.set(routeKey, state);
};

export const clearNewChatScreenState = (routeKey: string): void => {
	screenStates.delete(routeKey);
};
