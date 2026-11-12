import type {
	EngineOptions,
	PluginEngine,
	PluginEngineErrorCode,
} from "../src/types.ts";
export declare function createPluginEngine(
	options: EngineOptions,
): Promise<PluginEngine>;
export declare class PluginEngineError extends Error {
	readonly code: PluginEngineErrorCode;
	constructor(code: PluginEngineErrorCode);
}
