import { useHeaderHeight } from "expo-router/react-navigation";
import { Platform } from "react-native";

/** iOS headers float over content (glass/scroll edge); Android headers are opaque bars. */
export const TRANSPARENT_HEADER = Platform.OS === "ios";

/** Top inset content needs to clear the native header. */
export function useHeaderContentInset(): number {
	const headerHeight = useHeaderHeight();
	return TRANSPARENT_HEADER ? headerHeight : 0;
}

/**
 * iOS presents secondary flows as detented form sheets; Android sheets have no
 * app bar for their toolbar actions, so they open as full pages instead.
 */
export const SHEET_PRESENTATION = Platform.OS === "ios" ? "formSheet" : "card";

/** Height an opaque header takes above a screen's content; 0 when it floats. */
export function useOpaqueHeaderHeight(): number {
	const headerHeight = useHeaderHeight();
	return TRANSPARENT_HEADER ? 0 : headerHeight;
}
