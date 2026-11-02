import { Color } from "expo-router";
import {
	Appearance,
	type ColorValue,
	DynamicColorIOS,
	Platform,
} from "react-native";

/** Brand fill; use the adaptive accent for text and standalone icons. */
export const APP_TINT = "hsl(83 74% 43%)";
export const PRIMARY_FOREGROUND = "#ffffff";

export const glass = {
	borderDark: "rgba(255,255,255,0.16)",
	borderLight: "rgba(0,0,0,0.12)",
	fillDark: "rgba(24,24,24,0.72)",
	fillLight: "rgba(255,255,255,0.78)",
	tintDark: "rgba(24,24,24,0.35)",
	tintLight: "rgba(255,255,255,0.4)",
	surfaceDark: "hsl(0 0% 12%)",
	surfaceLight: "#ffffff",
	hairlineDark: "rgba(255,255,255,0.1)",
	hairlineLight: "rgba(0,0,0,0.08)",
} as const;

type Palette = { light: string; dark: string };
type NativeColor = Palette & { ios?: () => ColorValue };

/**
 * iOS resolves light/dark natively; other platforms read the brand palette for
 * the current appearance each time a color is accessed.
 */
const nativeColors = <T extends Record<string, NativeColor | string>>(
	entries: T,
): { readonly [K in keyof T]: ColorValue } => {
	const resolved = {} as Record<string, ColorValue>;
	for (const [key, entry] of Object.entries(entries)) {
		if (typeof entry === "string") {
			resolved[key] = entry;
		} else if (Platform.OS === "ios") {
			resolved[key] =
				entry.ios?.() ??
				DynamicColorIOS({ light: entry.light, dark: entry.dark });
		} else {
			Object.defineProperty(resolved, key, {
				enumerable: true,
				get: () =>
					Appearance.getColorScheme() === "dark" ? entry.dark : entry.light,
			});
		}
	}
	return resolved as { readonly [K in keyof T]: ColorValue };
};

/** Semantic colors that follow the system appearance, matching `global.css`. */
export const colors = nativeColors({
	bg: { light: "#f7f7f5", dark: "hsl(0 0% 6%)" },
	fg: { light: "#171716", dark: "hsl(0 0% 93%)" },
	secondaryFg: {
		ios: () => Color.ios.secondaryLabel,
		light: "#6f6f6b",
		dark: "hsl(0 0% 68%)",
	},
	tertiaryFg: {
		ios: () => Color.ios.tertiaryLabel,
		light: "#999994",
		dark: "hsl(0 0% 43%)",
	},
	mutedFg: {
		ios: () => Color.ios.secondaryLabel,
		light: "#6f6f6b",
		dark: "hsl(0 0% 68%)",
	},
	card: { light: "#ffffff", dark: "hsl(0 0% 12%)" },
	cardElevated: { light: "#f0f0ed", dark: "hsl(0 0% 13%)" },
	border: {
		ios: () => Color.ios.separator,
		light: "rgba(23, 23, 22, 0.1)",
		dark: "rgba(255, 255, 255, 0.1)",
	},
	accent: { light: "#486900", dark: "hsl(83 72% 46%)" },
	primaryForeground: PRIMARY_FOREGROUND,
	danger: {
		ios: () => Color.ios.systemRed,
		light: "#dc2626",
		dark: "hsl(0 84% 60%)",
	},
	warning: {
		ios: () => Color.ios.systemOrange,
		light: "#d97706",
		dark: "hsl(18 96% 62%)",
	},
	success: {
		ios: () => Color.ios.systemGreen,
		light: "#248a3d",
		dark: "#30d158",
	},
	diffAdded: "#269a3b",
	diffRemoved: "#d93f4c",
	diffHunk: "#a63aa5",

	diffAddedBg: "rgba(38,154,59,0.14)",
	diffRemovedBg: "rgba(217,63,76,0.14)",
});

export const spacing = {
	xs: 4,
	sm: 8,
	md: 12,
	lg: 16,
	xl: 24,
} as const;

export const radii = {
	sm: 12,
	md: 16,
	lg: 24,
	pill: 999,
} as const;
