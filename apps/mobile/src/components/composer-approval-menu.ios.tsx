import { Host } from "@expo/ui";
import { Image, Menu } from "@expo/ui/swift-ui";
import { accessibilityLabel, frame } from "@expo/ui/swift-ui/modifiers";
import type { RuntimeMode } from "@zuse/contracts";

import { approvalMenuItems } from "~/lib/action-menus";
import { runtimeOptionFor } from "~/lib/model-options";
import { SwiftMenuItems } from "./swift-menu-items";

const sf = (name: string) => name as never;

/** The composer "hand" button: a native menu of approval (runtime) modes. */
export function ComposerApprovalMenu({
	runtimeMode,
	onChange,
}: {
	runtimeMode: RuntimeMode;
	onChange: (mode: RuntimeMode) => void;
}) {
	const selected = runtimeOptionFor(runtimeMode);

	return (
		<Host
			key={runtimeMode}
			ignoreSafeArea="keyboard"
			seedColor={selected.tint}
			style={{ width: 40, height: 40 }}
		>
			<Menu
				label={
					<Image
						systemName={sf(selected.systemImage)}
						size={16}
						color={selected.tint}
						modifiers={[frame({ width: 40, height: 40 })]}
					/>
				}
				modifiers={[accessibilityLabel(`${selected.label} permissions`)]}
			>
				<SwiftMenuItems items={approvalMenuItems(runtimeMode, onChange)} />
			</Menu>
		</Host>
	);
}
