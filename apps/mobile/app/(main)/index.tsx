import { router, Stack } from "expo-router";
import { View } from "react-native";

import { HomeFeed } from "~/components/home/home-feed";
import { useOnboardingGate } from "~/components/home/use-onboarding-gate";
import { useSplitExpanded } from "~/components/root-navigator";
import { Button } from "~/components/ui/button";
import { EmptyState } from "~/components/ui/empty-state";

/**
 * Compact windows show the chat feed here. When the split view is expanded the
 * feed lives in the sidebar, so this column shows an empty state instead.
 */
export default function HomeScreen() {
	const expanded = useSplitExpanded();
	useOnboardingGate();
	if (!expanded) return <HomeFeed placement="stack" />;
	return (
		<>
			<Stack.Screen options={{ title: "", headerLargeTitle: false }} />
			<View className="flex-1 bg-background">
				<EmptyState
					symbol="bubble.left.and.bubble.right.fill"
					title="Select a chat"
					detail="Choose a chat from the sidebar or start a new one."
				>
					<Button onPress={() => router.push("/new-chat")}>New Chat</Button>
				</EmptyState>
			</View>
		</>
	);
}
