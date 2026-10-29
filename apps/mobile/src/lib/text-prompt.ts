import { Alert, type KeyboardTypeOptions, Platform } from "react-native";

export type TextPromptOptions = {
	title: string;
	message?: string;
	defaultValue?: string;
	placeholder?: string;
	/** Defaults to "OK", matching the native iOS prompt. */
	confirmLabel?: string;
	cancelLabel?: string;
	destructive?: boolean;
	secure?: boolean;
	keyboardType?: KeyboardTypeOptions;
	/** Receives the raw entered text; callers own trimming and validation. */
	onSubmit: (value: string) => void;
};

export type TextPromptRequest = TextPromptOptions & { readonly id: number };

export type TextPromptResult =
	| { readonly kind: "submit"; readonly value: string }
	| { readonly kind: "cancel" };

export const DEFAULT_CONFIRM_LABEL = "OK";
export const DEFAULT_CANCEL_LABEL = "Cancel";

/**
 * FIFO queue behind the in-app prompt dialog. Only the head request is shown;
 * a prompt opened from another prompt's callback waits for the first to close.
 */
export const createTextPromptQueue = () => {
	let pending: readonly TextPromptRequest[] = [];
	let nextId = 1;
	const listeners = new Set<() => void>();
	const emit = () => {
		for (const listener of listeners) listener();
	};

	return {
		current: (): TextPromptRequest | null => pending[0] ?? null,
		subscribe: (listener: () => void): (() => void) => {
			listeners.add(listener);
			return () => {
				listeners.delete(listener);
			};
		},
		enqueue: (options: TextPromptOptions): number => {
			const id = nextId++;
			pending = [...pending, { ...options, id }];
			emit();
			return id;
		},
		/** Closes a request once; repeated or stale settles are ignored. */
		settle: (id: number, result: TextPromptResult): void => {
			const request = pending.find((candidate) => candidate.id === id);
			if (request === undefined) return;
			pending = pending.filter((candidate) => candidate.id !== id);
			emit();
			if (result.kind === "submit") request.onSubmit(result.value);
		},
	};
};

export type TextPromptQueue = ReturnType<typeof createTextPromptQueue>;

/** Requests rendered by `TextPromptHost` on platforms without `Alert.prompt`. */
export const textPromptQueue = createTextPromptQueue();

/**
 * Cross-platform single-line text prompt. iOS keeps the native `Alert.prompt`;
 * other platforms render the in-app dialog mounted by `TextPromptHost`.
 */
export const promptText = (options: TextPromptOptions): void => {
	if (Platform.OS !== "ios") {
		textPromptQueue.enqueue(options);
		return;
	}
	// The callback form keeps iOS's system Cancel/OK buttons; the labels and
	// destructive styling only apply to the in-app dialog.
	Alert.prompt(
		options.title,
		options.message,
		(value?: string) => options.onSubmit(value ?? ""),
		options.secure ? "secure-text" : "plain-text",
		options.defaultValue,
		options.keyboardType,
	);
};
