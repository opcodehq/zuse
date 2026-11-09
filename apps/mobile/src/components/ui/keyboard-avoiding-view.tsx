import {
	KeyboardAvoidingView as NativeKeyboardAvoidingView,
	Platform,
} from "react-native";
import { KeyboardAvoidingView as ControllerKeyboardAvoidingView } from "react-native-keyboard-controller";
import { withUniwind } from "uniwind";

/**
 * Android runs edge-to-edge, so the window no longer resizes for the keyboard
 * and React Native's view never sees it; the keyboard-controller drop-in reads
 * the real IME insets. iOS keeps React Native's own implementation.
 */
export const KeyboardAvoidingView =
	Platform.OS === "android"
		? withUniwind(ControllerKeyboardAvoidingView)
		: NativeKeyboardAvoidingView;
