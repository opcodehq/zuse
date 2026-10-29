import { bundledResolvedModelCatalog } from "@zuse/contracts";
import { describe, expect, test, vi } from "vitest";

import {
	defaultModelOptions,
	type ModelModeValue,
	modelOptionsForProvider,
	providerOptions,
} from "../../../src/lib/model-options";
import {
	modelSheetChoices,
	modelSheetMenuItems,
	withModel,
	withProvider,
	withReasoning,
	withRuntimeMode,
} from "../../../src/lib/model-sheet";

const catalog = bundledResolvedModelCatalog();
const value: ModelModeValue = {
	providerId: "codex",
	model: "gpt-5.6-sol",
	runtimeMode: "approval-required",
	permissionMode: "default",
	modelOptions: { reasoning: "high" },
};
const scope = {
	value,
	canChangeProvider: true,
	canChangeReasoning: true,
};

describe("modelSheetChoices", () => {
	test("offers every catalog provider and the current provider's models", () => {
		const choices = modelSheetChoices(catalog, scope);
		expect(choices.providers).toEqual(providerOptions(catalog));
		expect(choices.showProviderPicker).toBe(choices.providers.length > 1);
		expect(choices.models).toEqual(modelOptionsForProvider(catalog, "codex"));
		expect(choices.reasoning).toMatchObject({ value: "high" });
		expect(choices.runtime.label).toBe("Ask for approval");
	});

	test("locks the provider and reasoning when they cannot change", () => {
		const choices = modelSheetChoices(catalog, {
			...scope,
			canChangeProvider: false,
			canChangeReasoning: false,
		});
		expect(choices.providers.map((provider) => provider.value)).toEqual([
			"codex",
		]);
		expect(choices.showProviderPicker).toBe(false);
		expect(choices.reasoning).toBeNull();
	});

	test("keeps the current provider but hides unavailable ones", () => {
		const choices = modelSheetChoices(catalog, {
			...scope,
			availableProviders: ["claude"],
		});
		expect(choices.providers.map((provider) => provider.value)).toEqual(
			expect.arrayContaining(["codex", "claude"]),
		);
		expect(choices.providers).toHaveLength(2);
	});

	test("strict availability drops the current provider and its models", () => {
		const choices = modelSheetChoices(catalog, {
			...scope,
			availableProviders: ["claude"],
			strictProviders: true,
		});
		expect(choices.providers.map((provider) => provider.value)).toEqual([
			"claude",
		]);
		expect(choices.models).toEqual([]);
	});
});

describe("model sheet transitions", () => {
	test("switching provider selects its first model with default options", () => {
		const model = modelOptionsForProvider(catalog, "claude")[0]?.value;
		expect(model).toBeDefined();
		expect(withProvider(catalog, value, "claude")).toEqual({
			...value,
			providerId: "claude",
			model,
			modelOptions: defaultModelOptions(catalog, "claude", model as string),
		});
	});

	test("switching model resets its options; reasoning and runtime merge", () => {
		expect(withModel(catalog, value, "gpt-5.6-sol").modelOptions).toEqual(
			defaultModelOptions(catalog, "codex", "gpt-5.6-sol"),
		);
		expect(
			withReasoning(
				{ ...value, modelOptions: { other: "x" } },
				"reasoning",
				"low",
			).modelOptions,
		).toEqual({ other: "x", reasoning: "low" });
		expect(withRuntimeMode(value, "full-access").runtimeMode).toBe(
			"full-access",
		);
	});
});

describe("modelSheetMenuItems", () => {
	test("checks the current choices and applies selections", () => {
		const onChange = vi.fn();
		const choices = modelSheetChoices(catalog, scope);
		const items = modelSheetMenuItems(catalog, choices, value, onChange);
		expect(items.providers.find((item) => item.checked)?.id).toBe("codex");
		expect(items.models.find((item) => item.checked)?.id).toBe("gpt-5.6-sol");
		expect(items.reasoning.find((item) => item.checked)?.id).toBe("high");
		expect(items.approval.find((item) => item.checked)?.id).toBe(
			"approval-required",
		);

		items.reasoning.find((item) => item.id === "low")?.onSelect?.();
		expect(onChange).toHaveBeenLastCalledWith(
			withReasoning(value, "reasoning", "low"),
		);
		items.approval.find((item) => item.id === "auto")?.onSelect?.();
		expect(onChange).toHaveBeenLastCalledWith(withRuntimeMode(value, "auto"));
	});

	test("has no reasoning items when reasoning is locked", () => {
		const choices = modelSheetChoices(catalog, {
			...scope,
			canChangeReasoning: false,
		});
		expect(
			modelSheetMenuItems(catalog, choices, value, vi.fn()).reasoning,
		).toEqual([]);
	});
});
