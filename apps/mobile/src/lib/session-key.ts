import type { SessionId } from "@zuse/contracts";

export const connectionSessionKey = (
	connKey: string,
	sessionId: SessionId | string,
): string => JSON.stringify([connKey, sessionId]);

/** Whether a key made by `connectionSessionKey` belongs to `connKey`. */
export const isConnectionSessionKey = (
	key: string,
	connKey: string,
): boolean => {
	try {
		const parsed: unknown = JSON.parse(key);
		return Array.isArray(parsed) && parsed[0] === connKey;
	} catch {
		return false;
	}
};
