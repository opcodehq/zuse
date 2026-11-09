import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
	Modal,
	Pressable,
	Text,
	type TextInputInstance,
	View,
} from "react-native";

import { Button } from "~/components/ui/button";
import { Input } from "~/components/ui/input";
import { KeyboardAvoidingView } from "~/components/ui/keyboard-avoiding-view";
import {
	DEFAULT_CANCEL_LABEL,
	DEFAULT_CONFIRM_LABEL,
	type TextPromptRequest,
	textPromptQueue,
} from "~/lib/text-prompt";

/**
 * Renders `promptText` requests on platforms without a native `Alert.prompt`.
 * Mount once near the app root.
 */
export function TextPromptHost() {
	const request = useSyncExternalStore(
		textPromptQueue.subscribe,
		textPromptQueue.current,
	);
	// Keep the closing dialog painted while the modal fades out.
	const [shown, setShown] = useState(request);
	if (request !== null && request !== shown) setShown(request);
	const inputRef = useRef<TextInputInstance>(null);

	return (
		<Modal
			transparent
			visible={request !== null}
			animationType="fade"
			statusBarTranslucent
			navigationBarTranslucent
			onShow={() => inputRef.current?.focus()}
			onRequestClose={() => {
				if (request !== null)
					textPromptQueue.settle(request.id, { kind: "cancel" });
			}}
		>
			{shown === null ? null : (
				<TextPromptDialog key={shown.id} request={shown} inputRef={inputRef} />
			)}
		</Modal>
	);
}

function TextPromptDialog({
	request,
	inputRef,
}: {
	request: TextPromptRequest;
	inputRef: React.RefObject<TextInputInstance | null>;
}) {
	const [value, setValue] = useState(request.defaultValue ?? "");
	const cancel = () => textPromptQueue.settle(request.id, { kind: "cancel" });
	const submit = () =>
		textPromptQueue.settle(request.id, { kind: "submit", value });

	// A follow-up prompt reuses the already-open modal, so `onShow` won't refire.
	useEffect(() => {
		const timer = setTimeout(() => inputRef.current?.focus(), 50);
		return () => clearTimeout(timer);
	}, [inputRef]);

	return (
		<KeyboardAvoidingView behavior="padding" className="flex-1">
			<View className="flex-1 items-center justify-center px-6">
				<Pressable
					accessibilityLabel="Dismiss"
					accessibilityRole="button"
					onPress={cancel}
					className="absolute inset-0 bg-black/50"
				/>
				<View
					accessibilityViewIsModal
					style={{ borderCurve: "continuous" }}
					className="w-full max-w-sm gap-4 rounded-3xl bg-card p-5 shadow-lg"
				>
					<View className="gap-1.5">
						<Text
							accessibilityRole="header"
							className="font-sans-bold text-[17px] text-foreground"
						>
							{request.title}
						</Text>
						{request.message ? (
							<Text className="font-sans text-[14px] leading-5 text-muted-foreground">
								{request.message}
							</Text>
						) : null}
					</View>
					<Input
						ref={inputRef}
						value={value}
						onChangeText={setValue}
						placeholder={request.placeholder}
						secureTextEntry={request.secure}
						keyboardType={request.keyboardType}
						autoCapitalize="none"
						autoCorrect={false}
						selectTextOnFocus
						returnKeyType="done"
						submitBehavior="blurAndSubmit"
						onSubmitEditing={submit}
						accessibilityLabel={request.title}
						className="bg-background"
					/>
					<View className="flex-row justify-end gap-2">
						<Button variant="ghost" size="sm" onPress={cancel}>
							{request.cancelLabel ?? DEFAULT_CANCEL_LABEL}
						</Button>
						<Button
							variant={request.destructive ? "danger" : "primary"}
							size="sm"
							onPress={submit}
						>
							{request.confirmLabel ?? DEFAULT_CONFIRM_LABEL}
						</Button>
					</View>
				</View>
			</View>
		</KeyboardAvoidingView>
	);
}
