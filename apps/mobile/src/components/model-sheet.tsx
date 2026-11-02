import { BottomSheet } from "@expo/ui/community/bottom-sheet";
import { type SFSymbol, SymbolView } from "expo-symbols";
import type { ReactNode } from "react";
import { type ColorValue, View } from "react-native";

import type { MenuItem } from "~/lib/menu-items";
import {
	type ModelSheetProps,
	modelSheetChoices,
	modelSheetMenuItems,
} from "~/lib/model-sheet";
import { platformSymbolName } from "~/lib/symbol-names";
import { activeModelCatalog } from "~/store/model-catalog";
import { colors } from "~/theme";
import { ActionMenu } from "./action-menu";
import { ProviderLogo } from "./provider-logo";
import { ListRow, ListSection } from "./ui/list";

/**
 * The model / approval picker as a Material bottom sheet. Each row opens an
 * anchored menu of its options, mirroring the iOS sheet's menu-style pickers.
 */
export function ModelSheet({
	open,
	onOpenChange,
	value,
	availableProviders,
	strictProviders = false,
	canChangeProvider,
	canChangeReasoning,
	onChange,
}: ModelSheetProps) {
	const catalog = activeModelCatalog();
	const choices = modelSheetChoices(catalog, {
		value,
		availableProviders,
		strictProviders,
		canChangeProvider,
		canChangeReasoning,
	});
	const items = modelSheetMenuItems(catalog, choices, value, onChange);
	const providerLabel =
		choices.providers.find((provider) => provider.value === value.providerId)
			?.label ?? value.providerId;
	const modelLabel =
		choices.models.find((model) => model.value === value.model)?.label ??
		value.model;

	return (
		<BottomSheet
			index={open ? 0 : -1}
			enablePanDownToClose
			onClose={() => onOpenChange(false)}
			backgroundStyle={{ backgroundColor: colors.bg }}
		>
			<View className="gap-5 px-4 pt-1 pb-8">
				<ListSection header="Model">
					{choices.showProviderPicker ? (
						<PickerRow
							key="provider"
							title="Provider"
							value={providerLabel}
							items={items.providers}
							leading={<RowSymbol name="cpu" />}
						/>
					) : null}
					<PickerRow
						key="model"
						title="Model"
						value={modelLabel}
						items={items.models}
						leading={
							<View className="w-5 items-center">
								<ProviderLogo providerId={value.providerId} size={17} />
							</View>
						}
					/>
					{choices.reasoning === null ? null : (
						<PickerRow
							key="reasoning"
							title="Intelligence"
							value={choices.reasoning.label}
							items={items.reasoning}
							leading={<RowSymbol name="gauge.with.dots.needle.67percent" />}
						/>
					)}
				</ListSection>
				<ListSection header="Permissions">
					<PickerRow
						key="approval"
						title="Approval"
						value={choices.runtime.label}
						items={items.approval}
						leading={
							<RowSymbol
								name={choices.runtime.systemImage}
								color={choices.runtime.tint}
							/>
						}
					/>
				</ListSection>
			</View>
		</BottomSheet>
	);
}

function PickerRow({
	title,
	value,
	items,
	leading,
}: {
	title: string;
	value: string;
	items: readonly MenuItem[];
	leading: ReactNode;
}) {
	return (
		<ActionMenu items={items} stretch>
			{(open) => (
				<ListRow
					title={title}
					value={value}
					leading={leading}
					chevron={false}
					trailing={
						<RowSymbol
							name="arrowtriangle.down.fill"
							size={20}
							color={colors.tertiaryFg}
						/>
					}
					accessibilityLabel={`${title}: ${value}`}
					disabled={items.length === 0}
					className="min-h-12 py-2"
					onPress={open}
				/>
			)}
		</ActionMenu>
	);
}

function RowSymbol({
	name,
	size = 17,
	color = colors.fg,
}: {
	name: string;
	size?: number;
	color?: ColorValue;
}) {
	return (
		<View className="w-5 items-center">
			<SymbolView
				name={platformSymbolName(name as SFSymbol)}
				size={size}
				tintColor={color}
			/>
		</View>
	);
}
