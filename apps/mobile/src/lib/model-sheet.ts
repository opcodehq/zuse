import type {
	ModelCatalogView,
	ProviderId,
	RuntimeMode,
} from "@zuse/contracts";

import { approvalMenuItems } from "./action-menus";
import type { MenuItem } from "./menu-items";
import {
	defaultModelOptions,
	type ModelModeValue,
	modelOptionsForProvider,
	providerOptions,
	type RuntimeOption,
	reasoningValueForModel,
	runtimeOptionFor,
} from "./model-options";

export type ModelSheetProps = {
	open: boolean;
	onOpenChange: (open: boolean) => void;
	value: ModelModeValue;
	availableProviders?: readonly ProviderId[] | null;
	strictProviders?: boolean;
	canChangeProvider: boolean;
	canChangeReasoning: boolean;
	onChange: (value: ModelModeValue) => void;
};

export type ModelSheetChoices = {
	providers: readonly { value: ProviderId; label: string }[];
	/** The provider picker only appears when there is a real choice. */
	showProviderPicker: boolean;
	models: readonly { value: string; label: string }[];
	/** `null` when the model has no reasoning levels or they are locked. */
	reasoning: ReturnType<typeof reasoningValueForModel>;
	runtime: RuntimeOption;
};

/**
 * What the model sheet offers for the current value. Shared by the iOS
 * (SwiftUI Form) and Android (Material) sheets so both show the same choices.
 */
export const modelSheetChoices = (
	catalog: ModelCatalogView,
	{
		value,
		availableProviders,
		strictProviders = false,
		canChangeProvider,
		canChangeReasoning,
	}: Pick<
		ModelSheetProps,
		| "value"
		| "availableProviders"
		| "strictProviders"
		| "canChangeProvider"
		| "canChangeReasoning"
	>,
): ModelSheetChoices => {
	const providers = providerOptions(catalog).filter((provider) => {
		if (strictProviders && !availableProviders?.includes(provider.value))
			return false;
		if (!canChangeProvider) return provider.value === value.providerId;
		if (availableProviders == null) return true;
		return (
			provider.value === value.providerId ||
			availableProviders.includes(provider.value)
		);
	});
	const models =
		strictProviders && !availableProviders?.includes(value.providerId)
			? []
			: modelOptionsForProvider(catalog, value.providerId);
	return {
		providers,
		showProviderPicker: canChangeProvider && providers.length > 1,
		models,
		reasoning: canChangeReasoning
			? reasoningValueForModel(
					catalog,
					value.providerId,
					value.model,
					value.modelOptions,
				)
			: null,
		runtime: runtimeOptionFor(value.runtimeMode),
	};
};

/** Switching provider picks its first model and that model's defaults. */
export const withProvider = (
	catalog: ModelCatalogView,
	value: ModelModeValue,
	providerId: ProviderId,
): ModelModeValue => {
	const model =
		modelOptionsForProvider(catalog, providerId)[0]?.value ?? value.model;
	return {
		...value,
		providerId,
		model,
		modelOptions: defaultModelOptions(catalog, providerId, model),
	};
};

export const withModel = (
	catalog: ModelCatalogView,
	value: ModelModeValue,
	model: string,
): ModelModeValue => ({
	...value,
	model,
	modelOptions: defaultModelOptions(catalog, value.providerId, model),
});

export const withReasoning = (
	value: ModelModeValue,
	descriptorId: string,
	optionId: string,
): ModelModeValue => ({
	...value,
	modelOptions: { ...(value.modelOptions ?? {}), [descriptorId]: optionId },
});

export const withRuntimeMode = (
	value: ModelModeValue,
	runtimeMode: RuntimeMode,
): ModelModeValue => ({ ...value, runtimeMode });

/**
 * The sheet's choices as menu items, for platforms that present each picker
 * as an anchored menu rather than a SwiftUI `Picker`.
 */
export const modelSheetMenuItems = (
	catalog: ModelCatalogView,
	choices: ModelSheetChoices,
	value: ModelModeValue,
	onChange: (value: ModelModeValue) => void,
): {
	providers: MenuItem[];
	models: MenuItem[];
	reasoning: MenuItem[];
	approval: MenuItem[];
} => {
	const { reasoning } = choices;
	return {
		providers: choices.providers.map((provider) => ({
			id: provider.value,
			title: provider.label,
			checked: provider.value === value.providerId,
			onSelect: () => onChange(withProvider(catalog, value, provider.value)),
		})),
		models: choices.models.map((model) => ({
			id: model.value,
			title: model.label,
			checked: model.value === value.model,
			onSelect: () => onChange(withModel(catalog, value, model.value)),
		})),
		reasoning:
			reasoning === null
				? []
				: reasoning.descriptor.options.map((option) => ({
						id: option.id,
						title: option.label,
						checked: option.id === reasoning.value,
						onSelect: () =>
							onChange(
								withReasoning(value, reasoning.descriptor.id, option.id),
							),
					})),
		approval: approvalMenuItems(value.runtimeMode, (runtimeMode) =>
			onChange(withRuntimeMode(value, runtimeMode)),
		),
	};
};
