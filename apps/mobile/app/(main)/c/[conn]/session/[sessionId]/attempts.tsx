import "@zuse/i18n/english/chat";
import { useAtomValue } from "@effect/atom-react";
import { createSessionAttemptHistory } from "@zuse/client-runtime/session-attempt-history";
import { SessionId } from "@zuse/contracts";
import { useMessages } from "@zuse/i18n/react";
import { Effect } from "effect";
import { Stack, useLocalSearchParams } from "expo-router";
import { useEffect, useMemo, useSyncExternalStore } from "react";
import { ActivityIndicator, FlatList, Text, View } from "react-native";
import { Button } from "~/components/ui/button";
import { connectionErrorMessage } from "~/lib/connection-error-message";
import {
	normalizeConnParam,
	optionsForConnection,
} from "~/lib/connection-params";
import { listSessionAttempts } from "~/rpc/session-attempts";
import { allConnectionsAtom } from "~/store/connections";
import { colors } from "~/theme";

export default function SessionAttemptsScreen() {
	const { message } = useMessages(["chat"]);
	const { conn, sessionId } = useLocalSearchParams<{
		conn: string;
		sessionId: string;
	}>();
	const connKey = normalizeConnParam(conn);
	const id = SessionId.make(normalizeConnParam(sessionId));
	const connections = useAtomValue(allConnectionsAtom);
	const connection = useMemo(
		() => optionsForConnection(connKey, connections),
		[connKey, connections],
	);
	const history = useMemo(
		() =>
			createSessionAttemptHistory((before) =>
				connection === null
					? Promise.reject(new Error("Connection unavailable"))
					: Effect.runPromise(
							listSessionAttempts({
								connection,
								sessionId: id,
								...(before === undefined ? {} : { before }),
							}),
						),
			),
		[connection, id],
	);
	const state = useSyncExternalStore(
		history.subscribe,
		history.snapshot,
		history.snapshot,
	);
	useEffect(() => {
		if (connection !== null) void history.refresh();
		return history.cancel;
	}, [connection, history]);
	return (
		<View className="flex-1 bg-background">
			<Stack.Screen
				options={{
					title: message("chat:attempts_title"),
					headerLargeTitle: false,
				}}
			/>
			<FlatList
				data={state.items}
				keyExtractor={(item) => item.id}
				contentInsetAdjustmentBehavior="automatic"
				contentContainerStyle={{ padding: 16, paddingBottom: 32, gap: 12 }}
				ListHeaderComponent={
					<View className="gap-3">
						<Text className="font-sans text-[13px] leading-5 text-muted-foreground">
							{message("chat:attempts_description")}
						</Text>
						{connection === null || state.error !== null ? (
							<Text
								accessibilityRole="alert"
								selectable
								className="font-sans text-[13px] text-danger"
							>
								{connection === null
									? message("chat:attempts_unavailable")
									: connectionErrorMessage(state.error)}
							</Text>
						) : null}
						<View className="flex-row items-center justify-end gap-2">
							{state.loading ? (
								<ActivityIndicator
									size="small"
									color={colors.secondaryFg}
									accessibilityLabel={message("chat:attempts_loading")}
								/>
							) : null}
							<Button
								variant="ghost"
								className="h-7"
								disabled={state.loading || connection === null}
								onPress={() => void history.refresh()}
							>
								{message("chat:attempts_refresh")}
							</Button>
						</View>
					</View>
				}
				ListEmptyComponent={
					state.loaded && !state.loading && state.error === null ? (
						<Text className="py-4 font-sans text-[13px] text-muted-foreground">
							{message("chat:attempts_empty")}
						</Text>
					) : undefined
				}
				renderItem={({ item }) => (
					<View className="gap-1 rounded-xl bg-muted/30 px-3 py-2.5">
						<View className="flex-row items-start justify-between gap-3">
							<Text className="flex-1 font-sans-medium text-[14px] text-foreground">
								{message("chat:attempts_turn")} {item.turnOrdinal} ·{" "}
								{message("chat:attempts_attempt")} {item.ordinal}
							</Text>
							<Text className="font-sans text-[12px] text-muted-foreground">
								{message(`chat:attempts_${item.status}`)}
							</Text>
						</View>
						<Text className="font-sans text-[12px] text-muted-foreground">
							{message(
								`chat:attempts_${item.reason === "provider_recovery" ? "recovery" : item.reason}`,
							)}
							{item.rolledBack
								? ` · ${message("chat:attempts_rolled_back")}`
								: ""}
						</Text>
						<Text className="font-sans text-[11px] text-muted-foreground">
							{new Date(item.startedAt).toLocaleString()}
							{item.completedAt === null
								? ""
								: ` — ${new Date(item.completedAt).toLocaleString()}`}
						</Text>
					</View>
				)}
				ListFooterComponent={
					state.nextCursor === null ? undefined : (
						<Button
							variant="ghost"
							className="h-7"
							disabled={state.loading}
							onPress={() => void history.more()}
						>
							{message("chat:attempts_more")}
						</Button>
					)
				}
			/>
		</View>
	);
}
