import { useAtomValue } from "@effect/atom-react";
import { Slot } from "expo-router";
import { SplitView } from "expo-router/unstable-split-view";
import { createContext, use, useState } from "react";
import { Platform, useWindowDimensions } from "react-native";

import { HomeFeed } from "~/components/home/home-feed";
import {
	SIDEBAR_WIDTH,
	SidebarWidthContext,
	TABLET_MIN_SIDE,
} from "~/lib/form-factor";
import {
	onboardingCompleteAtom,
	onboardingHydratedAtom,
} from "~/store/onboarding";

/** Whether the chat list currently lives in the iOS split view's sidebar. */
const SplitExpandedContext = createContext(false);

/** True when the chat list lives in the sidebar rather than the main stack. */
export const useSplitExpanded = (): boolean => use(SplitExpandedContext);

/** Display modes in which the sidebar sits beside the content. */
const SIDEBAR_BESIDE_MODES = new Set([
	"oneBesideSecondary",
	"twoBesideSecondary",
	"twoDisplaceSecondary",
]);

/**
 * Wide iOS windows (iPad, unfolded iPhone Duo) use a native split view with
 * the chat list beside the open chat. Compact windows (iPhone, folded Duo,
 * narrow multitasking), first-run onboarding, and other platforms use the
 * plain stack: a collapsed native split would nest the stack under its own
 * navigation bar and cannot present onboarding.
 */
export function RootNavigator() {
	const { width } = useWindowDimensions();
	const onboardingHydrated = useAtomValue(onboardingHydratedAtom);
	const onboardingComplete = useAtomValue(onboardingCompleteAtom);
	// UIKit may still collapse a wide window; stay on the stack at that width.
	const [collapsedAtWidth, setCollapsedAtWidth] = useState<number | null>(null);
	const split =
		Platform.OS === "ios" &&
		width >= TABLET_MIN_SIDE &&
		onboardingHydrated &&
		onboardingComplete &&
		collapsedAtWidth !== width;
	if (!split) return <Slot />;
	return <SplitNavigator onCollapse={() => setCollapsedAtWidth(width)} />;
}

function SplitNavigator({ onCollapse }: { onCollapse: () => void }) {
	const [sidebarBeside, setSidebarBeside] = useState(true);
	return (
		<SplitExpandedContext value={true}>
			<SidebarWidthContext value={sidebarBeside ? SIDEBAR_WIDTH : 0}>
				<SplitView
					topColumnForCollapsing="secondary"
					preferredDisplayMode="oneBesideSecondary"
					preferredSplitBehavior="tile"
					primaryBackgroundStyle="sidebar"
					columnMetrics={{
						minimumPrimaryColumnWidth: SIDEBAR_WIDTH,
						maximumPrimaryColumnWidth: SIDEBAR_WIDTH,
						preferredPrimaryColumnWidthOrFraction: SIDEBAR_WIDTH,
					}}
					onCollapse={onCollapse}
					onDisplayModeWillChange={(event) =>
						setSidebarBeside(
							SIDEBAR_BESIDE_MODES.has(event.nativeEvent.nextDisplayMode),
						)
					}
				>
					<SplitView.Column>
						<HomeFeed placement="sidebar" />
					</SplitView.Column>
				</SplitView>
			</SidebarWidthContext>
		</SplitExpandedContext>
	);
}
