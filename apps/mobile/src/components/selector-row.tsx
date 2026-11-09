import { type SFSymbol, SymbolView } from "expo-symbols";
import { Pressable, Text } from "react-native";

import { type SelectorRowProps, selectorMenuItems } from "~/lib/action-menus";
import { cn } from "~/lib/cn";
import { platformSymbolName } from "~/lib/symbol-names";
import { colors } from "~/theme";
import { ActionMenu } from "./action-menu";

export type { SelectorOption } from "~/lib/action-menus";

/** A labelled row that opens a Material menu of the selector's options. */
export function SelectorRow({
	symbol,
	label,
	options,
	disabled = false,
	emptyLabel = "None",
	compact = false,
}: SelectorRowProps) {
	return (
		<ActionMenu
			items={selectorMenuItems(options, { disabled, emptyLabel })}
			stretch={!compact}
			style={{ alignSelf: compact ? "flex-start" : "stretch" }}
		>
			{(open) => (
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={label}
					className={cn(
						"flex-row items-center gap-2.5 active:opacity-60",
						compact ? "h-7" : "h-12",
					)}
					onPress={open}
				>
					<SymbolView
						name={platformSymbolName(symbol as SFSymbol)}
						size={18}
						tintColor={colors.fg}
					/>
					<Text
						className="min-w-0 flex-shrink font-sans-medium text-[15px] text-foreground"
						numberOfLines={1}
					>
						{label}
					</Text>
					<SymbolView
						name={platformSymbolName("arrowtriangle.down.fill")}
						size={20}
						tintColor={colors.tertiaryFg}
					/>
				</Pressable>
			)}
		</ActionMenu>
	);
}
