import "@zuse/i18n/english/chat";
import { useAtomValue } from "@effect/atom-react";
import { type SessionCheckpointSummary, SessionId } from "@zuse/contracts";
import { useMessages } from "@zuse/i18n/react";
import { Effect } from "effect";
import { Stack, useLocalSearchParams } from "expo-router";
import { useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, ScrollView, Text, View } from "react-native";
import { Button } from "~/components/ui/button";
import { connectionErrorMessage } from "~/lib/connection-error-message";
import {
	normalizeConnParam,
	optionsForConnection,
} from "~/lib/connection-params";
import { sessionCheckpointActions } from "~/rpc/session-checkpoints";
import { allConnectionsAtom } from "~/store/connections";
import { colors } from "~/theme";

export default function SessionCheckpointsScreen() {
	const { message } = useMessages(["chat"]);
	const params = useLocalSearchParams<{ conn: string; sessionId: string }>();
	const connKey = normalizeConnParam(params.conn);
	const sessionId = SessionId.make(normalizeConnParam(params.sessionId));
	const connections = useAtomValue(allConnectionsAtom);
	const connection = useMemo(
		() => optionsForConnection(connKey, connections),
		[connKey, connections],
	);
	const actions = useMemo(
		() =>
			connection === null
				? null
				: sessionCheckpointActions(connection, sessionId),
		[connection, sessionId],
	);
	const [rows, setRows] = useState<readonly SessionCheckpointSummary[]>([]);
	const [preview, setPreview] = useState<{
		previewId: string;
		files: readonly string[];
	} | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const generation = useRef(0);
	const running = useRef(false);
	useEffect(() => {
		const current = ++generation.current;
		setRows([]);
		setPreview(null);
		setError(null);
		running.current = false;
		if (actions === null) {
			setBusy(false);
			return;
		}
		setBusy(true);
		running.current = true;
		void Effect.runPromise(actions.list())
			.then(
				(value) => {
					if (current === generation.current) setRows(value);
				},
				(cause) => {
					if (current === generation.current)
						setError(connectionErrorMessage(cause));
				},
			)
			.finally(() => {
				if (current === generation.current) {
					setBusy(false);
					running.current = false;
				}
			});
		return () => {
			generation.current++;
		};
	}, [actions]);
	const run = async (
		operation: (api: NonNullable<typeof actions>) => Promise<void>,
	) => {
		if (actions === null || running.current) return;
		const current = generation.current;
		running.current = true;
		setBusy(true);
		setError(null);
		try {
			await operation(actions);
		} catch (cause) {
			if (current === generation.current)
				setError(connectionErrorMessage(cause));
		} finally {
			if (current === generation.current) {
				running.current = false;
				setBusy(false);
			}
		}
	};
	const refresh = () =>
		run(async (api) => {
			const current = generation.current;
			const value = await Effect.runPromise(api.list());
			if (current === generation.current) setRows(value);
		});
	const restore = (previewId: string) =>
		run(async (api) => {
			const current = generation.current;
			await Effect.runPromise(api.restore(previewId));
			if (current !== generation.current) return;
			setPreview(null);
			const value = await Effect.runPromise(api.list());
			if (current === generation.current) setRows(value);
		});
	return (
		<View className="flex-1 bg-background">
			<Stack.Screen
				options={{
					title: message("chat:checkpoints_title"),
					headerLargeTitle: false,
				}}
			/>
			<ScrollView
				contentInsetAdjustmentBehavior="automatic"
				contentContainerStyle={{ padding: 16, gap: 12, paddingBottom: 32 }}
			>
				<Text className="text-sm text-muted-foreground">
					{message("chat:checkpoints_description")}
				</Text>
				{error !== null ? (
					<Text accessibilityRole="alert" className="text-sm text-destructive">
						{error}
					</Text>
				) : null}
				{connection === null ? (
					<Text className="text-sm text-muted-foreground">
						{message("chat:attempts_unavailable")}
					</Text>
				) : null}
				{busy ? <ActivityIndicator color={colors.fg} /> : null}
				{preview !== null ? (
					<View className="gap-3">
						<Text className="text-sm">
							{message("chat:checkpoints_confirm")}
						</Text>
						{preview.files.length === 0 ? (
							<Text className="text-sm text-muted-foreground">
								{message("chat:checkpoints_no_files")}
							</Text>
						) : (
							preview.files.map((file) => (
								<Text selectable key={file} className="text-xs text-foreground">
									{file}
								</Text>
							))
						)}
						<View className="flex-row gap-2">
							<Button
								className="h-7"
								variant="ghost"
								disabled={busy}
								onPress={() => setPreview(null)}
							>
								{message("chat:checkpoints_back")}
							</Button>
							<Button
								className="h-7"
								disabled={busy}
								onPress={() => void restore(preview.previewId)}
							>
								{message("chat:checkpoints_restore")}
							</Button>
						</View>
					</View>
				) : (
					<>
						<Button
							className="h-7 self-start"
							variant="ghost"
							disabled={busy || actions === null}
							onPress={() => void refresh()}
						>
							{message("chat:checkpoints_refresh")}
						</Button>
						{!busy && rows.length === 0 ? (
							<Text className="text-sm text-muted-foreground">
								{message("chat:checkpoints_empty")}
							</Text>
						) : null}
						{rows.map((row) => (
							<View key={row.id} className="gap-2 py-1">
								<Text className="text-sm text-foreground">
									{message(
										row.phase === "baseline"
											? "chat:checkpoints_before"
											: "chat:checkpoints_after",
									)}{" "}
									{row.turnOrdinal}
								</Text>
								<Text className="text-xs text-muted-foreground">
									{message(`chat:checkpoints_status_${row.status}`)}
								</Text>
								{row.error !== null ? (
									<Text className="text-xs text-destructive">{row.error}</Text>
								) : null}
								{row.restoreId !== null ? (
									<Button
										className="h-7 self-start"
										disabled={busy}
										onPress={() => {
											if (row.restoreId !== null) void restore(row.restoreId);
										}}
									>
										{message("chat:checkpoints_resume")}
									</Button>
								) : row.status === "ready" ? (
									<Button
										className="h-7 self-start"
										variant="ghost"
										disabled={busy}
										onPress={() =>
											void run(async (api) => {
												const current = generation.current;
												const value = await Effect.runPromise(
													api.preview(row.id),
												);
												if (current === generation.current) setPreview(value);
											})
										}
									>
										{message("chat:checkpoints_review")}
									</Button>
								) : row.status === "error" ? (
									<Button
										className="h-7 self-start"
										disabled={busy}
										onPress={() =>
											void run(async (api) => {
												const current = generation.current;
												await Effect.runPromise(api.retry());
												if (current !== generation.current) return;
												const value = await Effect.runPromise(api.list());
												if (current === generation.current) setRows(value);
											})
										}
									>
										{message("chat:checkpoints_retry")}
									</Button>
								) : null}
							</View>
						))}
					</>
				)}
			</ScrollView>
		</View>
	);
}
