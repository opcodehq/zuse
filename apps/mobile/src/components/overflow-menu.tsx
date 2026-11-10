import { SymbolView } from "expo-symbols";
import { Pressable } from "react-native";

import type { MenuItem } from "~/lib/menu-items";
import { BORDERLESS_RIPPLE } from "~/lib/ripple";
import { platformSymbolName } from "~/lib/symbol-names";
import { colors } from "~/theme";
import { ActionMenu } from "./action-menu";

/** A row-level ⋮ button that opens an anchored Material dropdown. */
export function OverflowMenu({
	items,
	label,
}: {
	items: readonly MenuItem[];
	label: string;
	/** Heading of the iOS action sheet; Material menus have no title. */
	title: string;
}) {
	return (
		<ActionMenu items={items}>
			{(open) => (
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={label}
					android_ripple={BORDERLESS_RIPPLE}
					className="h-11 w-11 items-center justify-center"
					onPress={open}
				>
					<SymbolView
						name={platformSymbolName("ellipsis")}
						size={20}
						tintColor={colors.secondaryFg}
					/>
				</Pressable>
			)}
		</ActionMenu>
	);
}
