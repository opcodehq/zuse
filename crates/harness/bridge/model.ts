import { createOpenAI } from "@ai-sdk/openai";
import { createXai } from "@ai-sdk/xai";
import {
	APICallError,
	jsonSchema,
	StreamProviderError,
	streamText,
	type ToolSet,
	tool,
} from "ai";
import { createHash } from "node:crypto";
const promptCacheKey=(connection:string,model:string,root:string)=>createHash("sha256").update(JSON.stringify(["zuse-harness-v1",connection,model,root])).digest("hex");
type HarnessTool={description:string;parameters:Record<string,unknown>};
type HarnessModelInput={rootId:string;model:string;reasoning?:string;messages:any[];tools:ToolSet;signal:AbortSignal;onText:(text:string)=>void};
type HarnessModelOutput=Record<string,unknown>;

export class HarnessModelError extends Error {
    readonly code:string;
    readonly status?:number;
    constructor(code:string,status?:number){super(`Model request failed (${code}).`);this.code=code;this.status=status;}
}
const safeErrorCode = (code: unknown, status?: number): string =>
	code === "subscription_sharing_usage_limit_exceeded" ||
	code === "subscription_sharing_unavailable"
		? code
		: `http_${status ?? "unknown"}`;
export function modelError(error: unknown): HarnessModelError {
	if (error instanceof HarnessModelError) return error;
	if (StreamProviderError.isInstance(error))
		return new HarnessModelError(
			safeErrorCode(error.code, error.statusCode),
			error.statusCode,
		);
	if (APICallError.isInstance(error)) {
		let code = `http_${error.statusCode ?? "unknown"}`;
		try {
			const body = JSON.parse(error.responseBody ?? "{}");
			code = safeErrorCode(body.error?.code, error.statusCode);
		} catch {}
		return new HarnessModelError(code, error.statusCode);
	}
	return new HarnessModelError("stream_interrupted");
}
export function modelTools(
	definitions: Readonly<Record<string, HarnessTool>>,
): ToolSet {
	return Object.fromEntries(
		Object.entries(definitions)
			.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
			.map(([name, definition]) => [
				name,
				tool({
					description: definition.description,
					inputSchema: jsonSchema(definition.parameters),
					providerOptions: {
						openai: {
							namespace: {
								name: "zuse",
								description: "Local coding and collaboration tools",
							},
						},
					},
				}),
			]),
	);
}

/** AI SDK translates messages and streams; the harness alone executes tools/retries. */
export async function requestModel(
	input: HarnessModelInput,
	credential: {
		connectionId: string;
		provider?: "chatgpt" | "supergrok";
		accessToken: string;
	},
	fetcher?: typeof fetch,
): Promise<HarnessModelOutput> {
	const grok = credential.provider === "supergrok";
    const safeFetch:typeof fetch=(url,init)=>(fetcher ?? fetch)(url,{...init,redirect:"error"});
	const provider = createOpenAI({
		apiKey: credential.accessToken,
		baseURL: "https://api.openai.com/v1",
		fetch: subscriptionFetch(safeFetch),
	});
	const result = streamText({
		model: grok
			? createXai({ apiKey: credential.accessToken, fetch: safeFetch }).responses(
					input.model,
				)
			: provider.responses(input.model),
		instructions: input.messages
			.filter((message) => message.role === "system")
			.map((message) => message.content)
			.join("\n\n"),
		messages: input.messages.filter((message) => message.role !== "system"),
		tools: input.tools,
		abortSignal: input.signal,
		maxRetries: 0,
		onError: () => {},
		providerOptions: grok
			? {
					xai: {
						store: false,
						...(input.reasoning ? { reasoningEffort: input.reasoning } : {}),
					},
				}
			: {
					openai: {
						store: false,
						systemMessageMode: "developer",
						strictJsonSchema: false,
						promptCacheKey: promptCacheKey(
							credential.connectionId,
							input.model,
							input.rootId,
						),
						...(input.reasoning ? { reasoningEffort: input.reasoning } : {}),
					},
				},
	});
	let finished = false;
	const started = performance.now();
	let firstTokenMs: number | undefined;
	try {
		for await (const part of result.fullStream) {
			if (part.type === "text-delta") {
				firstTokenMs ??= performance.now() - started;
				input.onText(part.text);
			}
			if (part.type === "error") throw modelError(part.error);
			if (part.type === "abort")
				throw new DOMException("Interrupted", "AbortError");
			if (part.type === "finish") {
				if (part.finishReason !== "stop" && part.finishReason !== "tool-calls")
					throw new HarnessModelError(`response_${part.finishReason}`);
				finished = true;
			}
		}
		if (!finished) throw new HarnessModelError("stream_incomplete");
		const [response, calls, usage, text, steps] = await Promise.all([
			result.response,
			result.toolCalls,
			result.usage,
			result.text,
			result.steps,
		]);
		return {
			messages: response.messages,
			calls: calls.map((call) => ({
				id: call.toolCallId,
				name: call.toolName,
				input: call.input,
			})),
			text,
			inputTokens: usage.inputTokens ?? 0,
			outputTokens: usage.outputTokens ?? 0,
			cachedTokens: reportedCacheTokens(steps[0]?.usage.raw, "cached_tokens"),
			cacheWriteTokens: reportedCacheTokens(
				steps[0]?.usage.raw,
				"cache_write_tokens",
			),
			connectionId: credential.connectionId,
			firstTokenMs,
		};
	} catch (error) {
		if (input.signal.aborted)
			throw new DOMException("Interrupted", "AbortError");
		throw modelError(error);
	}
}

/** Deny unexpected SDK fields before a subscription request leaves the process. */
export const SUBSCRIPTION_FIELDS = new Set([
	"model",
	"input",
	"instructions",
	"tools",
	"tool_choice",
	"parallel_tool_calls",
	"reasoning",
	"text",
	"include",
	"store",
	"stream",
	"prompt_cache_key",
	"service_tier",
]);
export function subscriptionFetch(fetcher: typeof fetch): typeof fetch {
	return async (url, init) => {
		if (typeof init?.body === "string") {
			const body: Record<string, unknown> = JSON.parse(init.body);
			for (const key of Object.keys(body))
				if (!SUBSCRIPTION_FIELDS.has(key))
					throw new HarnessModelError("unsupported_request_field");
			if (
				body.store !== false ||
				body.stream !== true ||
				!Array.isArray(body.input)
			)
				throw new HarnessModelError("invalid_subscription_request");
		} else throw new HarnessModelError("invalid_subscription_request");
		return fetcher(url, init);
	};
}

/** The SDK normalizes absent cache reads to zero; diagnostics must preserve unknown. */
function reportedCacheTokens(raw: unknown, key: string): number | undefined {
	if (
		raw === null ||
		typeof raw !== "object" ||
		!("input_tokens_details" in raw)
	)
		return undefined;
	const details = raw.input_tokens_details;
	if (details === null || typeof details !== "object") return undefined;
	const value = Reflect.get(details, key);
	return typeof value === "number" && Number.isFinite(value) && value >= 0
		? value
		: undefined;
}
