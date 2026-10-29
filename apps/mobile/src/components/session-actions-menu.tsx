import { SymbolView } from "expo-symbols";
import { SquarePen } from "lucide-react-native";
import { Pressable, View } from "react-native";

import { useSessionActionMenuItems } from "~/hooks/use-session-action-menu-items";
import type { SessionActionsMenuProps } from "~/lib/action-menus";
import { platformSymbolName } from "~/lib/symbol-names";
import { colors } from "~/theme";
import { ActionMenu } from "./action-menu";

/** Header actions: New chat plus a Material overflow menu. */
export function SessionActionsMenu(props: SessionActionsMenuProps) {
	const items = useSessionActionMenuItems(props);
	return (
		<View className="flex-row items-center gap-1">
			<Pressable
				accessibilityRole="button"
				accessibilityLabel="New chat"
				hitSlop={8}
				className="h-10 w-10 items-center justify-center active:opacity-60"
				onPress={props.onNewChat}
			>
				<SquarePen size={20} color={colors.fg} />
			</Pressable>
			<ActionMenu items={items}>
				{(open) => (
					<Pressable
						accessibilityRole="button"
						accessibilityLabel="Chat actions"
						hitSlop={8}
						className="h-10 w-10 items-center justify-center active:opacity-60"
						onPress={open}
					>
						<SymbolView
							name={platformSymbolName("ellipsis")}
							size={22}
							tintColor={colors.fg}
						/>
					</Pressable>
				)}
			</ActionMenu>
		</View>
	);
}
