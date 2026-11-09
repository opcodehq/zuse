import { parseGenerativeUiSpec } from "../../../packages/utils/src/generative-ui.ts";

Object.assign(globalThis, {
	parseNativeSpec: (spec: string): string =>
		JSON.stringify(parseGenerativeUiSpec(spec)),
});
