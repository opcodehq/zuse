import { Ellipsis } from "lucide-react-native";
import { Alert, Pressable } from "react-native";

import type { MenuItem } from "~/lib/menu-items";
import { colors } from "~/theme";

/** iOS keeps its row action sheet: the items as alert buttons plus Cancel. */
export function OverflowMenu({
	items,
	label,
	title,
}: {
	items: readonly MenuItem[];
	label: string;
	title: string;
}) {
	return (
		<Pressable
			accessibilityRole="button"
			accessibilityLabel={label}
			className="h-11 w-11 items-center justify-center"
			onPress={() =>
				Alert.alert(title, undefined, [
					...items.map((item) => ({
						text: item.title,
						style:
							item.destructive === true ? ("destructive" as const) : undefined,
						onPress: item.onSelect,
					})),
					{ text: "Cancel", style: "cancel" as const },
				])
			}
		>
			<Ellipsis size={19} color={colors.secondaryFg} />
		</Pressable>
	);
}
