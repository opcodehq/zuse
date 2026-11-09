import { Menu, Button as NativeButton, Section } from "@expo/ui/swift-ui";

import { type MenuItem, menuItemSystemImage } from "~/lib/menu-items";

const sf = (name: string | undefined) => name as never;

const noop = () => {};

/**
 * Renders shared menu items inside an iOS SwiftUI `Menu`. Only imported from
 * `.ios.tsx` components; Android draws the same items with `ActionMenu`.
 */
export function SwiftMenuItems({ items }: { items: readonly MenuItem[] }) {
	return items.map((item) => <SwiftMenuItem key={item.id} item={item} />);
}

function SwiftMenuItem({ item }: { item: MenuItem }) {
	if (item.children !== undefined) {
		return item.inline === true ? (
			<Section title={item.title}>
				<SwiftMenuItems items={item.children} />
			</Section>
		) : (
			<Menu label={item.title} systemImage={sf(item.systemImage)}>
				<SwiftMenuItems items={item.children} />
			</Menu>
		);
	}
	return (
		<NativeButton
			label={item.title}
			systemImage={sf(menuItemSystemImage(item))}
			role={item.destructive === true ? "destructive" : undefined}
			onPress={item.onSelect ?? noop}
		/>
	);
}
