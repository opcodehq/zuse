import type { SFSymbol } from "expo-symbols";
import { SymbolView } from "expo-symbols";
import { Pressable, Text, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { RIPPLE } from "~/lib/ripple";
import { platformSymbolName } from "~/lib/symbol-names";
import { PRIMARY_FOREGROUND } from "~/theme";

/** Material 3 extended floating action button, pinned above the navigation bar. */
export function Fab({
	symbol,
	label,
	onPress,
}: {
	symbol: SFSymbol;
	label: string;
	onPress: () => void;
}) {
	const insets = useSafeAreaInsets();
	return (
		<View
			pointerEvents="box-none"
			className="absolute right-4"
			style={{ bottom: insets.bottom + 16 }}
		>
			<Pressable
				accessibilityRole="button"
				accessibilityLabel={label}
				android_ripple={RIPPLE}
				onPress={onPress}
				className="h-14 flex-row items-center gap-3 overflow-hidden rounded-2xl bg-primary pl-4 pr-5"
				style={{ elevation: 3 }}
			>
				<SymbolView
					name={platformSymbolName(symbol)}
					size={24}
					tintColor={PRIMARY_FOREGROUND}
				/>
				<Text className="font-sans-medium text-[15px] text-primary-foreground">
					{label}
				</Text>
			</Pressable>
		</View>
	);
}
