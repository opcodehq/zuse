import { Host, Switch as MaterialSwitch } from "@expo/ui/jetpack-compose";
import { View } from "react-native";
import { useUniwind } from "uniwind";

import { APP_TINT, colors, PRIMARY_FOREGROUND } from "~/theme";

/**
 * Material 3 switch in the app palette. The wrapper carries the accessible
 * name and state so TalkBack announces it like a native labelled switch.
 */
export function Switch({
	value,
	onValueChange,
	disabled = false,
	accessibilityLabel,
}: {
	value: boolean;
	onValueChange?: (value: boolean) => void;
	disabled?: boolean;
	accessibilityLabel?: string;
}) {
	const { theme } = useUniwind();
	return (
		<View
			accessible
			accessibilityRole="switch"
			accessibilityLabel={accessibilityLabel}
			accessibilityState={{ checked: value, disabled }}
			accessibilityActions={[{ name: "activate" }]}
			onAccessibilityAction={() => {
				if (!disabled) onValueChange?.(!value);
			}}
		>
			<View importantForAccessibility="no-hide-descendants">
				<Host matchContents colorScheme={theme === "dark" ? "dark" : "light"}>
					<MaterialSwitch
						value={value}
						enabled={!disabled}
						onCheckedChange={onValueChange}
						colors={{
							checkedTrackColor: APP_TINT,
							checkedThumbColor: PRIMARY_FOREGROUND,
							checkedBorderColor: APP_TINT,
							uncheckedTrackColor: colors.cardElevated,
							uncheckedThumbColor: colors.tertiaryFg,
							uncheckedBorderColor: colors.tertiaryFg,
						}}
					/>
				</Host>
			</View>
		</View>
	);
}
