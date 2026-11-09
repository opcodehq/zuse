import "@zuse/i18n/english/chat";
import { useMessages } from "@zuse/i18n/react";

import {
	type SessionActionsMenuProps,
	sessionActionMenuItems,
} from "~/lib/action-menus";
import type { MenuItem } from "~/lib/menu-items";

/** Localised header overflow items shared by the iOS and Android menus. */
export const useSessionActionMenuItems = (
	props: SessionActionsMenuProps,
): MenuItem[] => {
	const { message } = useMessages(["chat"]);
	return sessionActionMenuItems(props, {
		checkpoints: message("chat:checkpoints_title"),
		attempts: message("chat:attempts_title"),
	});
};
