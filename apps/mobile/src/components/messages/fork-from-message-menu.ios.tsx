import { Host } from "@expo/ui";
import { Image, Menu } from "@expo/ui/swift-ui";
import { accessibilityLabel, frame } from "@expo/ui/swift-ui/modifiers";

import {
	type ForkFromMessageMenuProps,
	forkFromMessageMenuItems,
} from "~/lib/action-menus";
import { colors } from "~/theme";
import { SwiftMenuItems } from "../swift-menu-items";

const sf = (name: string) => name as never;

/** A compact native counterpart to the desktop message fork menu. */
export function ForkFromMessageMenu(props: ForkFromMessageMenuProps) {
	return (
		<Host
			ignoreSafeArea="all"
			seedColor={colors.fg}
			style={{ width: 36, height: 36 }}
		>
			<Menu
				label={
					<Image
						systemName={sf("arrow.triangle.branch")}
						size={16}
						color={colors.secondaryFg}
						modifiers={[frame({ width: 36, height: 36 })]}
					/>
				}
				modifiers={[
					frame({ width: 36, height: 36, alignment: "center" }),
					accessibilityLabel("Fork from here"),
				]}
			>
				<SwiftMenuItems items={forkFromMessageMenuItems(props)} />
			</Menu>
		</Host>
	);
}
