import { Host } from "@expo/ui";
import { Label, Menu } from "@expo/ui/swift-ui";
import { frame } from "@expo/ui/swift-ui/modifiers";
import { useState } from "react";

import { type SelectorRowProps, selectorMenuItems } from "~/lib/action-menus";
import { colors } from "~/theme";
import { SwiftMenuItems } from "./swift-menu-items";

export type { SelectorOption } from "~/lib/action-menus";

/**
 * Uses the supported native Label primitive rather than HStackView. Giving the
 * host the React row width and the menu a leading-aligned SwiftUI frame avoids
 * intrinsic Menu padding escaping the host and clipping the leading content.
 */
export function SelectorRow({
	symbol,
	label,
	options,
	disabled = false,
	emptyLabel = "None",
	compact = false,
}: SelectorRowProps) {
	const [rowWidth, setRowWidth] = useState(1);

	return (
		<Host
			className={compact ? "h-7" : undefined}
			matchContents={compact ? true : { vertical: true }}
			seedColor={colors.fg}
			style={{
				alignSelf: compact ? "flex-start" : "stretch",
				height: compact ? 28 : 48,
			}}
			onLayout={
				compact
					? undefined
					: (event) => setRowWidth(event.nativeEvent.layout.width)
			}
		>
			<Menu
				label={<Label title={label} systemImage={sf(symbol)} />}
				modifiers={
					compact
						? [frame({ height: 28, alignment: "leading" })]
						: [
								frame({
									width: rowWidth,
									height: 48,
									alignment: "leading",
								}),
							]
				}
			>
				<SwiftMenuItems
					items={selectorMenuItems(options, { disabled, emptyLabel })}
				/>
			</Menu>
		</Host>
	);
}

const sf = (name: string) => name as never;
