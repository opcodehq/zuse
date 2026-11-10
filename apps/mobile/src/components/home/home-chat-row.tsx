import { ArchiveIcon, PinIcon, PinOffIcon } from "@zuse/icons/solid-rounded";
import { type Href, Link } from "expo-router";
import { Pressable, Text, View } from "react-native";
import Swipeable from "react-native-gesture-handler/ReanimatedSwipeable";

import { DitherCloudIcon } from "~/components/dither-cloud-icon";
import { ProviderLogo } from "~/components/provider-logo";
import { HugeIcon } from "~/components/ui/huge-icon";
import { cn } from "~/lib/cn";
import type { HomeFeedItem } from "~/lib/home-feed";
import { colors } from "~/theme";

type ChatItem = HomeFeedItem & { type: "chat" };

export function HomeChatRow({
	item,
	onArchive,
	onTogglePin,
	onOpen,
	selectedPathname,
}: {
	item: ChatItem;
	onArchive: (item: ChatItem) => Promise<void>;
	onTogglePin: (item: ChatItem) => void;
	/** Replaces link navigation, e.g. the sidebar replacing the open chat. */
	onOpen?: (href: Href) => void;
	/** Highlights the row whose chat is currently open. */
	selectedPathname?: string;
}) {
	const row = item.row;
	const isActive = row.status === "running" || row.status === "booting";
	const canPin = row.chat !== null;
	const href =
		row.session === null
			? {
					pathname: "/new-chat" as const,
					params: { conn: row.connectionKey, chatId: row.chat?.id },
				}
			: (`/c/${encodeURIComponent(row.connectionKey)}/session/${encodeURIComponent(
					row.session.id,
				)}` as const);
	const selected =
		selectedPathname !== undefined &&
		row.session !== null &&
		safeDecode(selectedPathname).startsWith(
			`/c/${row.connectionKey}/session/${row.session.id}`,
		);

	const label = `${row.title}, ${row.projectName}`;
	const rowClassName = cn(
		"mx-1 min-h-[46px] justify-center rounded-xl px-3 py-2 active:bg-muted",
		selected && "bg-muted",
	);
	const rowBody = (
		<View className="flex-row items-center gap-2.5">
			<View
				className="h-5 w-5 items-center justify-center"
				style={{ marginTop: 2 }}
			>
				{row.connectionKey.startsWith("cloud:") ? (
					<DitherCloudIcon />
				) : (
					<ProviderLogo
						providerId={row.session?.providerId ?? "codex"}
						size={17}
						color={colors.secondaryFg}
					/>
				)}
				{isActive || row.unread ? (
					<View className="absolute -bottom-0.5 -right-0.5 h-1.5 w-1.5 rounded-full bg-primary" />
				) : null}
			</View>
			<Text
				className={cn(
					"min-w-0 flex-1 font-sans text-[16px] leading-5",
					row.unread ? "text-foreground" : "text-foreground/90",
				)}
				numberOfLines={1}
			>
				{row.title}
			</Text>
			{item.showProject ? (
				<Text
					numberOfLines={1}
					style={{ maxWidth: "28%" }}
					className="rounded-full bg-muted px-2 py-0.5 font-sans text-[11px] text-muted-foreground"
				>
					{row.projectName}
				</Text>
			) : null}
			<Text
				className="font-sans text-[12px] text-muted-foreground"
				style={{ fontVariant: ["tabular-nums"] }}
			>
				{row.subtitle}
			</Text>
		</View>
	);

	return (
		<Swipeable
			friction={2}
			leftThreshold={54}
			rightThreshold={54}
			overshootLeft={false}
			overshootRight={false}
			enableTrackpadTwoFingerGesture
			// Actions are always mounted underneath; the row must cover them.
			childrenContainerStyle={{ backgroundColor: colors.bg }}
			renderLeftActions={
				canPin
					? (_, __, methods) => (
							<Pressable
								accessibilityRole="button"
								accessibilityLabel={row.pinned ? "Unpin chat" : "Pin chat"}
								className="w-24 items-center justify-center bg-muted"
								onPress={() => {
									methods.close();
									onTogglePin(item);
								}}
							>
								<HugeIcon
									icon={row.pinned ? PinOffIcon : PinIcon}
									size={20}
									color={colors.secondaryFg}
								/>
								<Text className="mt-1 font-sans-medium text-[12px] text-muted-foreground">
									{row.pinned ? "Unpin" : "Pin"}
								</Text>
							</Pressable>
						)
					: undefined
			}
			renderRightActions={(_, __, methods) => (
				<Pressable
					accessibilityRole="button"
					accessibilityLabel="Archive chat"
					className="w-24 items-center justify-center bg-danger/15"
					onPress={() => {
						methods.close();
						void onArchive(item);
					}}
				>
					<HugeIcon icon={ArchiveIcon} size={20} color={colors.danger} />
					<Text className="mt-1 font-sans-medium text-[12px] text-danger">
						Archive
					</Text>
				</Pressable>
			)}
		>
			{onOpen ? (
				<Pressable
					onPress={() => onOpen(href)}
					accessibilityLabel={label}
					accessibilityState={{ selected }}
					className={rowClassName}
				>
					{rowBody}
				</Pressable>
			) : (
				<Link href={href} asChild>
					<Link.Trigger>
						<Pressable accessibilityLabel={label} className={rowClassName}>
							{rowBody}
						</Pressable>
					</Link.Trigger>
				</Link>
			)}
		</Swipeable>
	);
}

const safeDecode = (value: string): string => {
	try {
		return decodeURIComponent(value);
	} catch {
		return value;
	}
};
