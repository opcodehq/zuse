/**
 * Platform-neutral description of an anchored action menu. Builders produce one
 * tree; iOS renders it with SwiftUI (`swift-menu-items.tsx`) and Android with
 * a Compose Material dropdown (`action-menu.tsx`).
 */
export type MenuItem = {
	/** Unique within the whole tree; Android reports presses by id. */
	readonly id: string;
	readonly title: string;
	/** SF Symbol name. Android maps it through `materialSymbolNames`. */
	readonly systemImage?: string;
	readonly checked?: boolean;
	readonly destructive?: boolean;
	readonly onSelect?: () => void;
	/** Renders as a submenu, or as a titled section when `inline`. */
	readonly children?: readonly MenuItem[];
	readonly inline?: boolean;
};

/** iOS menus mark the selected item by swapping its icon for a checkmark. */
export const menuItemSystemImage = (item: MenuItem): string | undefined =>
	item.checked === true ? "checkmark" : item.systemImage;
