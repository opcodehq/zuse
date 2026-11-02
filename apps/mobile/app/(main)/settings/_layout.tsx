import { Stack } from "expo-router";
import { useReadableContentStyle } from "~/lib/form-factor";
import { TRANSPARENT_HEADER } from "~/lib/native-header";
import { colors } from "~/theme";
export default function SettingsLayout() {
	const readableContent = useReadableContentStyle();
	return (
		<Stack
			screenOptions={{
				headerLargeTitle: false,
				headerTransparent: TRANSPARENT_HEADER,
				headerBackButtonDisplayMode: "minimal",
				headerTintColor: colors.fg,
				contentStyle: readableContent,
			}}
		/>
	);
}
