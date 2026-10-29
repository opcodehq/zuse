import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
	platform: { OS: "android" as string },
	prompt: vi.fn(),
}));
vi.mock("react-native", () => ({
	Alert: { prompt: mocks.prompt },
	Platform: mocks.platform,
}));

import {
	createTextPromptQueue,
	promptText,
	textPromptQueue,
} from "../../../src/lib/text-prompt";

beforeEach(() => {
	vi.clearAllMocks();
	mocks.platform.OS = "android";
});

describe("text prompt queue", () => {
	it("shows requests one at a time in order", () => {
		const queue = createTextPromptQueue();
		const first = queue.enqueue({ title: "First", onSubmit: vi.fn() });
		queue.enqueue({ title: "Second", onSubmit: vi.fn() });
		expect(queue.current()?.title).toBe("First");
		queue.settle(first, { kind: "cancel" });
		expect(queue.current()?.title).toBe("Second");
	});

	it("passes the raw value to submit and ignores cancellation", () => {
		const queue = createTextPromptQueue();
		const onSubmit = vi.fn();
		const submitted = queue.enqueue({ title: "Rename", onSubmit });
		queue.settle(submitted, { kind: "submit", value: "  name " });
		expect(onSubmit).toHaveBeenCalledWith("  name ");
		const cancelled = queue.enqueue({ title: "Rename", onSubmit });
		queue.settle(cancelled, { kind: "cancel" });
		expect(onSubmit).toHaveBeenCalledOnce();
	});

	it("settles each request at most once", () => {
		const queue = createTextPromptQueue();
		const onSubmit = vi.fn();
		const id = queue.enqueue({ title: "Delete", onSubmit });
		queue.settle(id, { kind: "submit", value: "DELETE" });
		queue.settle(id, { kind: "submit", value: "DELETE" });
		queue.settle(id, { kind: "cancel" });
		expect(onSubmit).toHaveBeenCalledOnce();
	});

	it("closes before running callbacks so follow-up prompts queue behind none", () => {
		const queue = createTextPromptQueue();
		const id = queue.enqueue({
			title: "First",
			onSubmit: () => {
				expect(queue.current()).toBeNull();
				queue.enqueue({ title: "Follow-up", onSubmit: vi.fn() });
			},
		});
		queue.settle(id, { kind: "submit", value: "" });
		expect(queue.current()?.title).toBe("Follow-up");
	});

	it("notifies subscribers until they unsubscribe", () => {
		const queue = createTextPromptQueue();
		const listener = vi.fn();
		const unsubscribe = queue.subscribe(listener);
		const id = queue.enqueue({ title: "Rename", onSubmit: vi.fn() });
		queue.settle(id, { kind: "cancel" });
		expect(listener).toHaveBeenCalledTimes(2);
		unsubscribe();
		queue.enqueue({ title: "Rename", onSubmit: vi.fn() });
		expect(listener).toHaveBeenCalledTimes(2);
	});
});

describe("promptText", () => {
	it("queues the in-app dialog outside iOS", () => {
		const onSubmit = vi.fn();
		promptText({ title: "Rename chat", defaultValue: "Old", onSubmit });
		const request = textPromptQueue.current();
		expect(mocks.prompt).not.toHaveBeenCalled();
		expect(request).toMatchObject({
			title: "Rename chat",
			defaultValue: "Old",
		});
		if (request !== null)
			textPromptQueue.settle(request.id, { kind: "submit", value: "New" });
		expect(onSubmit).toHaveBeenCalledWith("New");
		expect(textPromptQueue.current()).toBeNull();
	});

	it("keeps the native iOS prompt and its system buttons", () => {
		mocks.platform.OS = "ios";
		const onSubmit = vi.fn();
		promptText({
			title: "Delete permanently?",
			message: "Type DELETE to continue.",
			confirmLabel: "Delete",
			destructive: true,
			onSubmit,
		});
		expect(textPromptQueue.current()).toBeNull();
		const [title, message, callback, type, defaultValue] =
			mocks.prompt.mock.lastCall ?? [];
		expect([title, message, type, defaultValue]).toEqual([
			"Delete permanently?",
			"Type DELETE to continue.",
			"plain-text",
			undefined,
		]);
		expect(typeof callback).toBe("function");
		callback("DELETE");
		callback(undefined);
		expect(onSubmit.mock.calls).toEqual([["DELETE"], [""]]);
	});

	it("uses secure entry on iOS when requested", () => {
		mocks.platform.OS = "ios";
		promptText({ title: "Token", secure: true, onSubmit: vi.fn() });
		const [, , , type] = mocks.prompt.mock.lastCall ?? [];
		expect(type).toBe("secure-text");
	});
});
