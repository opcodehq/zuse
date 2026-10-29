import { Host } from "@expo/ui";
import { Image, Menu } from "@expo/ui/swift-ui";
import { accessibilityLabel, frame } from "@expo/ui/swift-ui/modifiers";
import { SquarePen } from "lucide-react-native";
import { Pressable, View } from "react-native";

import { useSessionActionMenuItems } from "~/hooks/use-session-action-menu-items";
import type { SessionActionsMenuProps } from "~/lib/action-menus";
import { colors } from "~/theme";
import { SwiftMenuItems } from "./swift-menu-items";

const sf = (name: string) => name as never;

/** Header actions backed by SwiftUI's native anchored UIMenu. */
export function SessionActionsMenu(props: SessionActionsMenuProps) {
	const items = useSessionActionMenuItems(props);
	return (
		<View style={{ flexDirection: "row", alignItems: "center", gap: 4 }}>
			<Pressable
				accessibilityRole="button"
				accessibilityLabel="New chat"
				hitSlop={8}
				onPress={props.onNewChat}
				style={{
					width: 40,
					height: 40,
					alignItems: "center",
					justifyContent: "center",
				}}
			>
				<SquarePen size={20} color={colors.fg} />
			</Pressable>
			<Host
				matchContents
				seedColor={colors.fg}
				style={{ width: 40, height: 40 }}
			>
				<Menu
					label={
						<Image
							systemName={sf("ellipsis")}
							size={20}
							color={colors.fg}
							modifiers={[frame({ width: 40, height: 40 })]}
						/>
					}
					modifiers={[accessibilityLabel("Chat actions")]}
				>
					<SwiftMenuItems items={items} />
				</Menu>
			</Host>
		</View>
	);
}
