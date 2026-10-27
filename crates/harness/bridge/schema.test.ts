import { strict as assert } from "node:assert";
import { modelMessageSchema } from "ai";

// Exercise the public schema, including parts absent from the text/tool replay.
const text = { type: "text", text: "Unicode λ and src/main.rs" };
const file = {
	type: "file",
	data: { type: "data", data: new Uint8Array([1, 2, 3]) },
	mediaType: "application/octet-stream",
};
const call = {
	type: "tool-call", toolCallId: "call-λ", toolName: "read", input: {},
};
const result = {
	type: "tool-result", toolCallId: "call-λ", toolName: "read",
	output: { type: "text", value: "saved result" },
};
const cases = [
	{ role: "system", content: "Keep requirements" },
	{ role: "user", content: "hello" },
	{ role: "user", content: [text, file, {
		type: "image", image: new URL("https://example.test/image.png"),
	}] },
	{ role: "assistant", content: "response" },
	{ role: "assistant", content: [text, file, call, result,
		{ type: "reasoning", text: "reasoning" },
		{ type: "reasoning-file", data: "aGVsbG8=", mediaType: "text/plain" },
		{ type: "custom", kind: "provider.part" },
		{ type: "tool-approval-request", approvalId: "approval", toolCallId: "call-λ" },
	] },
	{ role: "tool", content: [result, {
		type: "tool-approval-response", approvalId: "approval", approved: true,
	}] },
];
for (const message of cases) {
	const withMetadata = { ...message, providerOptions: { openai: { itemId: "saved" } } };
	assert.deepEqual(modelMessageSchema.parse(withMetadata), withMetadata);
	assert.equal(modelMessageSchema.safeParse({ ...message, role: "unknown" }).success, false);
	assert.equal(modelMessageSchema.safeParse({ ...message, content: null }).success, false);
}
for (const content of [
	[{ ...call, toolCallId: 42 }],
	[{ ...result, output: { type: "text", value: 42 } }],
	[{ type: "unknown", text: "must not pass as text" }],
]) {
	assert.equal(modelMessageSchema.safeParse({ role: "assistant", content }).success, false);
}
console.log("Message schemas preserve text, files, images, reasoning, tools, approvals and metadata; malformed inputs remain rejected.");
