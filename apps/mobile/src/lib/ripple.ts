import { Platform, type PressableAndroidRippleConfig } from "react-native";

/** Material press feedback; iOS ignores `android_ripple` and keeps opacity states. */
export const RIPPLE: PressableAndroidRippleConfig = {
	color: "rgba(128, 128, 128, 0.22)",
	foreground: true,
};

export const BORDERLESS_RIPPLE: PressableAndroidRippleConfig = {
	color: "rgba(128, 128, 128, 0.22)",
	borderless: true,
	radius: 22,
};

/**
 * Appended after a pressable's own classes (merged by `cn`): on Android the
 * ripple replaces dimming and is clipped to rounded corners. Unset on iOS.
 */
export const ANDROID_RIPPLE_SURFACE =
	Platform.OS === "android" ? "overflow-hidden active:opacity-100" : undefined;

export const ANDROID_RIPPLE_ROW =
	Platform.OS === "android" ? "active:bg-transparent" : undefined;
