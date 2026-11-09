import { SymbolView } from "expo-symbols";
import { Pressable } from "react-native";

import {
	type ComposerPlusMenuProps,
	composerPlusMenuItems,
} from "~/lib/action-menus";
import { platformSymbolName } from "~/lib/symbol-names";
import { colors } from "~/theme";
import { ActionMenu } from "./action-menu";

/** Composer actions: attachments plus the Goal and Plan send modes. */
export function ComposerPlusMenu(props: ComposerPlusMenuProps) {
	return (
		<ActionMenu items={composerPlusMenuItems(props)}>
			{(open) => (
				<Pressable
					accessibilityRole="button"
					accessibilityLabel="Add attachment or mode"
					className="h-10 w-10 items-center justify-center active:opacity-60"
					onPress={open}
				>
					<SymbolView
						name={platformSymbolName("plus")}
						size={21}
						tintColor={colors.fg}
					/>
				</Pressable>
			)}
		</ActionMenu>
	);
}
