import { type SFSymbol, SymbolView } from "expo-symbols";
import type { LucideIcon } from "lucide-react-native";
import type { ReactNode } from "react";
import { Text, View } from "react-native";
import { platformSymbolName } from "~/lib/symbol-names";
import { colors } from "~/theme";

export const EmptyState = ({
	icon: Icon,
	symbol,
	title,
	detail,
	children,
}: {
	icon?: LucideIcon;
	symbol?: SFSymbol;
	title: string;
	detail?: string;
	/** Actions shown below the text, centered with it. */
	children?: ReactNode;
}) => (
	<View className="flex-1 items-center justify-center gap-3 px-7">
		<View
			style={{ borderCurve: "continuous" }}
			className="mb-1 h-16 w-16 items-center justify-center rounded-[20px] bg-muted"
		>
			{symbol ? (
				<SymbolView
					name={platformSymbolName(symbol)}
					size={27}
					weight="medium"
					tintColor={colors.fg}
				/>
			) : Icon ? (
				<Icon size={26} strokeWidth={1.8} color={colors.fg} />
			) : null}
		</View>
		<Text className="text-center font-sans-bold text-xl tracking-[-0.3px] text-foreground">
			{title}
		</Text>
		{detail !== undefined ? (
			<Text className="max-w-[320px] text-center font-sans text-[15px] leading-[21px] text-muted-foreground">
				{detail}
			</Text>
		) : null}
		{children === undefined ? null : <View className="mt-3">{children}</View>}
	</View>
);
