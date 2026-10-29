import { Host } from "@expo/ui";
import { Image, Menu } from "@expo/ui/swift-ui";
import { frame } from "@expo/ui/swift-ui/modifiers";

import {
	type ComposerPlusMenuProps,
	composerPlusMenuItems,
} from "~/lib/action-menus";
import { colors } from "~/theme";
import { SwiftMenuItems } from "./swift-menu-items";

const sf = (name: string) => name as never;

/**
 * Native composer action menu. Attachments launch system pickers and the two
 * send modes remain visible here instead of being buried in model settings.
 */
export function ComposerPlusMenu(props: ComposerPlusMenuProps) {
	return (
		<Host
			ignoreSafeArea="keyboard"
			seedColor={colors.fg}
			style={{ width: 40, height: 40 }}
		>
			<Menu
				label={
					<Image
						systemName={sf("plus")}
						size={17}
						color={colors.fg}
						modifiers={[frame({ width: 40, height: 40 })]}
					/>
				}
			>
				<SwiftMenuItems items={composerPlusMenuItems(props)} />
			</Menu>
		</Host>
	);
}
