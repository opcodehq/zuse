import { useAtomValue } from "@effect/atom-react";
import { router } from "expo-router";
import { useEffect } from "react";

import { authAccountAtom, authBusyAtom, authHydratedAtom } from "~/store/auth";
import {
	allConnectionsAtom,
	connectionsHydratedAtom,
} from "~/store/connections";
import {
	completeOnboarding,
	onboardingCompleteAtom,
	onboardingHydratedAtom,
} from "~/store/onboarding";

/**
 * Sends first-time users to onboarding. Runs from the main stack's root screen,
 * which is mounted whether the chat feed is in the stack or the split sidebar,
 * so the redirect never fires before the stack can navigate.
 */
export function useOnboardingGate(): void {
	const account = useAtomValue(authAccountAtom);
	const authHydrated = useAtomValue(authHydratedAtom);
	const busy = useAtomValue(authBusyAtom);
	const connections = useAtomValue(allConnectionsAtom);
	const connectionsHydrated = useAtomValue(connectionsHydratedAtom);
	const onboardingHydrated = useAtomValue(onboardingHydratedAtom);
	const onboardingComplete = useAtomValue(onboardingCompleteAtom);
	useEffect(() => {
		if (busy || !onboardingHydrated || !authHydrated || !connectionsHydrated)
			return;
		if (onboardingComplete) return;
		if (account !== null || connections.length > 0) {
			void completeOnboarding();
			return;
		}
		router.replace("/onboarding");
	}, [
		account,
		authHydrated,
		busy,
		connections.length,
		connectionsHydrated,
		onboardingComplete,
		onboardingHydrated,
	]);
}
