import { isAuthRedirectUrl } from "~/auth/config";
import { notificationRoute } from "~/notifications/route";

export function redirectSystemPath({
	path,
	initial,
}: {
	path: string;
	initial: boolean;
}): string {
	if (isAuthRedirectUrl(path)) return initial ? "/" : "";
	return notificationRoute(path) ?? path;
}
