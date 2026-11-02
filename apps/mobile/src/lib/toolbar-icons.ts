import {
	type SFSymbol,
	unstable_getMaterialSymbolSourceAsync,
} from "expo-symbols";
import { useSyncExternalStore } from "react";
import { type ImageSourcePropType, Platform } from "react-native";

import { platformSymbolName } from "./symbol-names";

export type ToolbarIcon = SFSymbol | ImageSourcePropType;

const ICON_SIZE_DP = 24;
const ICON_COLOR = "#000000";

const sources = new Map<SFSymbol, ImageSourcePropType>();
const requested = new Set<SFSymbol>();
const listeners = new Set<() => void>();
let version = 0;

const notify = () => {
	version += 1;
	for (const listener of listeners) listener();
};

const subscribe = (listener: () => void) => {
	listeners.add(listener);
	return () => {
		listeners.delete(listener);
	};
};

const getVersion = () => version;

function requestSource(name: SFSymbol) {
	if (requested.has(name)) return;
	requested.add(name);
	unstable_getMaterialSymbolSourceAsync(
		platformSymbolName(name).android,
		ICON_SIZE_DP,
		ICON_COLOR,
	)
		.then((source) => {
			if (source === null) return;
			sources.set(name, source);
			notify();
		})
		.catch(() => {
			requested.delete(name);
		});
}

const identity = (name: SFSymbol): ToolbarIcon => name;

/**
 * Resolves SF Symbol names for `Stack.Toolbar` items. Android toolbars only draw
 * image sources, so the matching Material Symbol is rendered to an image once and
 * cached; the item stays hidden until its image is ready.
 */
export function useToolbarIcons(): (name: SFSymbol) => ToolbarIcon | undefined {
	useSyncExternalStore(subscribe, getVersion, getVersion);
	if (Platform.OS === "ios") return identity;
	return (name) => {
		const source = sources.get(name);
		if (source === undefined) requestSource(name);
		return source;
	};
}
