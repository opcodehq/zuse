import type { RuntimeMode } from "@zuse/contracts";
import { type SFSymbol, SymbolView } from "expo-symbols";
import { Pressable } from "react-native";

import { approvalMenuItems } from "~/lib/action-menus";
import { runtimeOptionFor } from "~/lib/model-options";
import { platformSymbolName } from "~/lib/symbol-names";
import { ActionMenu } from "./action-menu";

/** The composer approval button: a Material menu of runtime modes. */
export function ComposerApprovalMenu({
	runtimeMode,
	onChange,
}: {
	runtimeMode: RuntimeMode;
	onChange: (mode: RuntimeMode) => void;
}) {
	const selected = runtimeOptionFor(runtimeMode);
	return (
		<ActionMenu items={approvalMenuItems(runtimeMode, onChange)}>
			{(open) => (
				<Pressable
					accessibilityRole="button"
					accessibilityLabel={`${selected.label} permissions`}
					className="h-10 w-10 items-center justify-center active:opacity-60"
					onPress={open}
				>
					<SymbolView
						name={platformSymbolName(selected.systemImage as SFSymbol)}
						size={18}
						tintColor={selected.tint}
					/>
				</Pressable>
			)}
		</ActionMenu>
	);
}
