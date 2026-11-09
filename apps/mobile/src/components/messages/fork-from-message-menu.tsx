import { SymbolView } from "expo-symbols";
import { Pressable } from "react-native";

import {
	type ForkFromMessageMenuProps,
	forkFromMessageMenuItems,
} from "~/lib/action-menus";
import { platformSymbolName } from "~/lib/symbol-names";
import { colors } from "~/theme";
import { ActionMenu } from "../action-menu";

/** Anchored fork menu; the new-chat destinations live in a submenu. */
export function ForkFromMessageMenu(props: ForkFromMessageMenuProps) {
	return (
		<ActionMenu items={forkFromMessageMenuItems(props)}>
			{(open) => (
				<Pressable
					accessibilityRole="button"
					accessibilityLabel="Fork from here"
					hitSlop={8}
					className="h-9 w-9 items-center justify-center active:opacity-60"
					onPress={open}
				>
					<SymbolView
						name={platformSymbolName("arrow.triangle.branch")}
						size={16}
						tintColor={colors.secondaryFg}
					/>
				</Pressable>
			)}
		</ActionMenu>
	);
}
