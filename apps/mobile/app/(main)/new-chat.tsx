import { useAtomValue } from "@effect/atom-react";
import {
	orderedChatSessions,
	resolveActiveChatSession,
} from "@zuse/client-runtime/chat-threads";
import {
	cloudProviderLabel,
	orderedCloudProviders,
	selectedCloudProvider,
} from "@zuse/client-runtime/cloud-sandbox-providers";
import type {
	ChatId,
	Folder,
	GitBranchInfo,
	GitPrSummary,
} from "@zuse/contracts";
import { Effect } from "effect";
import {
	router,
	Stack,
	useLocalSearchParams,
	useNavigation,
	useRoute,
} from "expo-router";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, ScrollView, Text, TextInput, View } from "react-native";
import { KeyboardController } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { ComposerActionSlot } from "~/components/composer-action-slot";
import { ComposerApprovalMenu } from "~/components/composer-approval-menu";
import { ComposerAttachmentStrip } from "~/components/composer-attachment-strip";
import { ComposerInputFrame } from "~/components/composer-input-frame";
import { ComposerModeChip } from "~/components/composer-mode-chip";
import { ComposerPlusMenu } from "~/components/composer-plus-menu";
import { ComposerSendButton } from "~/components/composer-send-button";
import type { ModelModeValue } from "~/components/model-mode-menu";
import { ModelSheet } from "~/components/model-sheet";
import { ModelSheetTrigger } from "~/components/model-sheet-trigger";
import { SelectorRow } from "~/components/selector-row";
import { Button } from "~/components/ui/button";
import { GlassSurface } from "~/components/ui/glass-surface";
import { KeyboardAvoidingView } from "~/components/ui/keyboard-avoiding-view";
import { cloudSandboxStatus } from "~/lib/cloud-sandbox-setup";
import {
	captureComposerImage,
	type LocalComposerAttachment,
	pickComposerFiles,
	pickComposerImages,
	uploadComposerAttachment,
} from "~/lib/composer-attachments";
import { connectionErrorMessage } from "~/lib/connection-error-message";
import { optionsForConnection } from "~/lib/connection-params";
import { availableConnections } from "~/lib/connection-records";
import { useReadableInset } from "~/lib/form-factor";
import {
	availableProviderIds,
	defaultModelForProvider,
	defaultModelOptions,
} from "~/lib/model-options";
import { useOpaqueHeaderHeight } from "~/lib/native-header";
import {
	buildNewChatCreatePayload,
	MAIN_SOURCE,
	type NewChatSource,
	type NewChatSourceKind,
	reconcileNewChatSource,
	sourceOptionsForKind,
	WORK_MODE_OPTIONS,
	workModeLabel,
} from "~/lib/new-chat";
import { connectionSessionKey } from "~/lib/session-key";
import { hasRunningChatThread } from "~/lib/thread-presentation";
import { useToolbarIcons } from "~/lib/toolbar-icons";
import {
	createWorktree,
	listBranches,
	listPullRequests,
	makeTextInput,
	sendMessage,
} from "~/rpc/actions";
import { cloudControlClient } from "~/rpc/api-client";
import { authAccountAtom } from "~/store/auth";
import {
	connectionAvailabilityAtom,
	hydrateAvailability,
} from "~/store/availability";
import {
	cloudAuthenticatedProvidersAtom,
	cloudCatalogAtom,
	refreshCloudCatalog,
} from "~/store/cloud-catalog";
import { launchMobileCloudChat } from "~/store/cloud-launch";
import {
	clearComposerDraft,
	composerDraft,
	hydrateComposerDraft,
	setComposerDraft,
} from "~/store/composer-drafts";
import {
	allConnectionsAtom as connectionsAtom,
	connectionsHydratedAtom,
	hydrateConnections,
	refreshConnectionLabel,
} from "~/store/connections";
import {
	connectToEnvironment,
	environmentsAtom,
	refreshEnvironments,
} from "~/store/environments";
import {
	activeModelCatalog,
	activeModelCatalogAtom,
	hydrateModelCatalog,
} from "~/store/model-catalog";
import {
	clearNewChatScreenState,
	newChatScreenState,
	readNewChatPreferences,
	saveNewChatPreferences,
	saveNewChatScreenState,
} from "~/store/new-chat-preferences";
import {
	bundlesByConnectionAtom,
	createChat,
	createSession,
	hydrateSessions,
	loadingByConnectionAtom,
	statusBySessionAtom,
} from "~/store/sessions";
import { colors } from "~/theme";

export default function NewChatScreen() {
	const keyboardOffset = useOpaqueHeaderHeight();
	const toolbarIcon = useToolbarIcons();
	const insets = useSafeAreaInsets();
	// Full-bleed screen: the readable column is applied to the content and composer.
	const readableInset = useReadableInset();
	const {
		conn,
		chatId,
		sandbox,
		cloudProjectId: requestedCloudProjectId,
		draft: requestedDraft,
	} = useLocalSearchParams<{
		conn?: string;
		chatId?: string;

		sandbox?: string;
		cloudProjectId?: string;
		draft?: string;
	}>();
	const requestedConnectionKey = conn?.trim() ?? "";
	const requestedChatId = (chatId?.trim() ?? "") as ChatId;
	const draftKey = `new-chat:${requestedConnectionKey || "auto"}:${requestedChatId || "root"}`;
	const [initialDraft] = useState(() => composerDraft(draftKey));
	// Layout changes (iPad resize, iPhone Duo fold) remount this screen; restore its choices.
	const routeKey = useRoute().key;
	const navigation = useNavigation();
	const [restored] = useState(() => newChatScreenState(routeKey));
	const inheritedModel = useRef(restored !== undefined);
	const submitGateRef = useRef(false);
	const [text, setText] = useState(requestedDraft ?? initialDraft.text);

	const [cloudSandbox, setCloudSandbox] = useState<string | null>(
		restored ? restored.cloudSandbox : (sandbox ?? null),
	);
	const cloudMode = cloudSandbox !== null;
	const [cloudProjectId, setCloudProjectId] = useState<string | null>(
		restored ? restored.cloudProjectId : (requestedCloudProjectId ?? null),
	);
	const [draftHydrated, setDraftHydrated] = useState(false);
	const [preferencesHydrated, setPreferencesHydrated] = useState(false);
	const [submitting, setSubmitting] = useState(false);
	const [modelSheetOpen, setModelSheetOpen] = useState(false);
	const [attachments, setAttachments] = useState<LocalComposerAttachment[]>([
		...initialDraft.attachments,
	]);
	const [goalMode, setGoalMode] = useState(initialDraft.goalMode);
	const [error, setError] = useState<string | null>(null);
	const [selectedConnectionKey, setSelectedConnectionKey] = useState<
		string | null
	>(restored ? restored.connectionKey : null);
	const [selectedProjectId, setSelectedProjectId] = useState<
		Folder["id"] | null
	>(restored ? restored.projectId : null);
	const [source, setSource] = useState<NewChatSource>(
		restored ? restored.source : MAIN_SOURCE,
	);

	const [sourceKind, setSourceKind] = useState<NewChatSourceKind>(
		restored ? restored.sourceKind : "main",
	);
	const initialModel = defaultModelForProvider(activeModelCatalog(), "codex");
	const [modelMode, setModelMode] = useState<ModelModeValue>(() =>
		restored
			? restored.modelMode
			: {
					providerId: "codex",
					model: initialModel,
					runtimeMode: "approval-required",
					permissionMode: "default",
					modelOptions: defaultModelOptions(
						activeModelCatalog(),
						"codex",
						initialModel,
					),
				},
	);

	const [branches, setBranches] = useState<readonly GitBranchInfo[]>([]);
	const [prs, setPrs] = useState<readonly GitPrSummary[]>([]);
	// Which machine and project the branch and PR choices were loaded for.
	const [sourcesLoadedFor, setSourcesLoadedFor] = useState<string | null>(null);

	const allConnections = useAtomValue(connectionsAtom);
	useAtomValue(activeModelCatalogAtom);
	const hydrated = useAtomValue(connectionsHydratedAtom);
	const account = useAtomValue(authAccountAtom);
	const connections = useMemo(
		() =>
			availableConnections(allConnections, account !== null).filter(
				(connection) =>
					connection.source !== "cloud" ||
					connection.key === requestedConnectionKey,
			),
		[account, allConnections, requestedConnectionKey],
	);
	const bundlesByConnection = useAtomValue(bundlesByConnectionAtom);
	const loadingByConnection = useAtomValue(loadingByConnectionAtom);
	const statusBySession = useAtomValue(statusBySessionAtom);

	useEffect(() => {
		let active = true;
		void hydrateComposerDraft(draftKey).then((draft) => {
			if (!active) return;
			if (draft !== null) {
				setText((current) =>
					current === initialDraft.text ? draft.text : current,
				);
				setAttachments((current) =>
					current.length === initialDraft.attachments.length
						? [...draft.attachments]
						: current,
				);
				setGoalMode((current) =>
					current === initialDraft.goalMode ? draft.goalMode : current,
				);
			}
			setDraftHydrated(true);
		});
		return () => {
			active = false;
		};
	}, [draftKey, initialDraft]);

	useEffect(() => {
		if (!draftHydrated) return;
		setComposerDraft(draftKey, {
			text,
			attachments,
			goalMode,
		});
	}, [attachments, draftHydrated, draftKey, goalMode, text]);

	useEffect(() => {
		if (!hydrated) void hydrateConnections();
	}, [hydrated]);

	useEffect(() => {
		for (const connection of connections) {
			const options = optionsForConnection(connection.key, connections);
			if (options === null) continue;
			void hydrateSessions(connection.key, options);

			void refreshConnectionLabel(connection.key, options);
		}
	}, [connections]);

	const selectedConnectionAvailable =
		selectedConnectionKey !== null &&
		connections.some((connection) => connection.key === selectedConnectionKey);
	const effectiveConnectionKey =
		(selectedConnectionAvailable ? selectedConnectionKey : null) ??
		(requestedConnectionKey.length > 0 ? requestedConnectionKey : null) ??
		connections[0]?.key ??
		null;

	const threadContext = useMemo(() => {
		if (effectiveConnectionKey === null || requestedChatId.length === 0)
			return null;
		for (const bundle of bundlesByConnection[effectiveConnectionKey] ?? []) {
			const chat = bundle.chats.find((item) => item.id === requestedChatId);
			if (chat === undefined) continue;
			const threads = orderedChatSessions(bundle.sessions, chat.id);
			return {
				chat,
				project: bundle.project,
				threads,
				activeThread: resolveActiveChatSession(chat, threads),
			};
		}
		return null;
	}, [bundlesByConnection, effectiveConnectionKey, requestedChatId]);
	const threadMode = requestedChatId.length > 0;

	useEffect(() => {
		// Restored choices already reflect the user's selection; preferences would overwrite them.
		if (threadMode || restored !== undefined) {
			setPreferencesHydrated(true);
			return;
		}
		let active = true;
		void readNewChatPreferences().then((preferences) => {
			if (!active) return;
			if (requestedConnectionKey.length === 0) {
				setSelectedConnectionKey(preferences.connectionKey);
				setSelectedProjectId(preferences.projectId);
			} else if (preferences.connectionKey === requestedConnectionKey) {
				setSelectedProjectId(preferences.projectId);
			}
			setSourceKind(preferences.sourceKind);
			setPreferencesHydrated(true);
		});
		return () => {
			active = false;
		};
	}, [requestedConnectionKey, restored, threadMode]);

	useEffect(() => {
		// Saving before preferences load would restore defaults over them.
		if (!preferencesHydrated) return;
		saveNewChatScreenState(routeKey, {
			connectionKey: selectedConnectionKey,
			projectId: selectedProjectId,
			source,
			sourceKind,
			modelMode,
			cloudSandbox,
			cloudProjectId,
		});
	}, [
		cloudProjectId,
		cloudSandbox,
		modelMode,
		preferencesHydrated,
		routeKey,
		selectedConnectionKey,
		selectedProjectId,
		source,
		sourceKind,
	]);

	useEffect(() => {
		// Fires only when the route is removed, not when a layout change remounts it.
		const unsubscribe = navigation.addListener("removed", () =>
			clearNewChatScreenState(routeKey),
		);
		// The event arrives after unmount, so unsubscribe on the next microtask.
		return () => queueMicrotask(unsubscribe);
	}, [navigation, routeKey]);

	const projectChoices = useMemo(() => {
		if (effectiveConnectionKey === null) return [];
		return (bundlesByConnection[effectiveConnectionKey] ?? []).map(
			(bundle) => ({
				project: bundle.project,
				connectionKey: effectiveConnectionKey,
			}),
		);
	}, [bundlesByConnection, effectiveConnectionKey]);

	const effectiveProjectId =
		threadContext?.chat.projectId ??
		(selectedProjectId !== null &&
		projectChoices.some((item) => item.project.id === selectedProjectId)
			? selectedProjectId
			: (projectChoices[0]?.project.id ?? null));

	useEffect(() => {
		if (
			!preferencesHydrated ||
			threadMode ||
			effectiveConnectionKey === null ||
			effectiveProjectId === null
		) {
			return;
		}
		void saveNewChatPreferences({
			connectionKey: effectiveConnectionKey,
			projectId: effectiveProjectId,
			sourceKind,
		}).catch(() => undefined);
	}, [
		effectiveConnectionKey,
		effectiveProjectId,
		preferencesHydrated,
		sourceKind,
		threadMode,
	]);

	const selectedOptions = useMemo(
		() =>
			effectiveConnectionKey === null
				? null
				: optionsForConnection(effectiveConnectionKey, connections),
		[connections, effectiveConnectionKey],
	);

	const availability = useAtomValue(
		connectionAvailabilityAtom(effectiveConnectionKey ?? ""),
	);
	useEffect(() => {
		if (effectiveConnectionKey === null || selectedOptions === null) return;
		void hydrateAvailability(effectiveConnectionKey, selectedOptions);
		void hydrateModelCatalog(effectiveConnectionKey, selectedOptions);
	}, [effectiveConnectionKey, selectedOptions]);
	const cloudProviders = useAtomValue(cloudAuthenticatedProvidersAtom);
	const availableProviders = useMemo(
		() =>
			cloudMode || requestedConnectionKey.startsWith("cloud:")
				? cloudProviders
				: availableProviderIds(availability),
		[availability, cloudMode, cloudProviders, requestedConnectionKey],
	);

	useEffect(() => {
		const active = threadContext?.activeThread;
		if (active === null || active === undefined || inheritedModel.current)
			return;
		inheritedModel.current = true;
		setModelMode({
			providerId: active.providerId,
			model: active.model,
			runtimeMode: active.runtimeMode,
			permissionMode: active.permissionMode,
			modelOptions: defaultModelOptions(
				activeModelCatalog(),
				active.providerId,
				active.model,
			),
		});
	}, [threadContext?.activeThread]);

	const effectiveModelMode = useMemo<ModelModeValue>(() => {
		if (
			availableProviders === null ||
			availableProviders.length === 0 ||
			availableProviders.includes(modelMode.providerId)
		) {
			return modelMode;
		}
		const providerId = availableProviders[0];
		if (providerId === undefined) return modelMode;
		const model = defaultModelForProvider(activeModelCatalog(), providerId);
		return {
			...modelMode,
			providerId,
			model,
			modelOptions: defaultModelOptions(
				activeModelCatalog(),
				providerId,
				model,
			),
		};
	}, [availableProviders, modelMode]);
	const goalSupported =
		availability
			?.find((entry) => entry.providerId === effectiveModelMode.providerId)
			?.capabilities?.includes("goalMode") === true;

	useEffect(() => {
		if (threadMode || selectedOptions === null || effectiveProjectId === null)
			return;
		let cancelled = false;
		void Promise.all([
			Effect.runPromise(
				listBranches({
					connection: selectedOptions,
					projectId: effectiveProjectId,
				}),
			),
			Effect.runPromise(
				listPullRequests({
					connection: selectedOptions,
					projectId: effectiveProjectId,
				}),
			),
		]).then(([nextBranches, nextPrs]) => {
			if (cancelled) return;
			setBranches(nextBranches);
			setPrs(nextPrs);
			setSourcesLoadedFor(`${effectiveConnectionKey}:${effectiveProjectId}`);
		});
		return () => {
			cancelled = true;
		};
	}, [selectedOptions, effectiveConnectionKey, effectiveProjectId, threadMode]);
	const sourcesLoaded =
		sourcesLoadedFor === `${effectiveConnectionKey}:${effectiveProjectId}`;

	const loading = Object.values(loadingByConnection).some(Boolean);
	const selectedProject =
		threadContext?.project ??
		projectChoices.find((item) => item.project.id === effectiveProjectId)
			?.project;

	const machineOptions = connections.map((connection) => ({
		key: connection.key,
		label: connection.label,
		selected: !cloudMode && connection.key === effectiveConnectionKey,
		onSelect: () => {
			setCloudSandbox(null);
			setSelectedConnectionKey(connection.key);
			setSelectedProjectId(null);
			setSource(MAIN_SOURCE);
			setSourceKind("main");
		},
	}));

	const accountEnvironments = useAtomValue(environmentsAtom);
	const cloudCatalog = useAtomValue(cloudCatalogAtom);
	const [connectingEnvironment, setConnectingEnvironment] = useState(false);
	useEffect(() => {
		if (account === null) return;
		void refreshEnvironments();
		void refreshCloudCatalog();
	}, [account]);
	const environmentOptions = accountEnvironments
		.filter(
			(environment) =>
				!connections.some(
					(connection) =>
						connection.environmentId === environment.environmentId,
				),
		)
		.map((environment) => ({
			key: `environment:${environment.environmentId}`,
			label: environment.label,
			selected: false,
			onSelect: () => {
				if (connectingEnvironment) return;
				setConnectingEnvironment(true);
				setError(null);
				void connectToEnvironment(environment.environmentId)
					.then((key) => {
						setCloudSandbox(null);
						setSelectedConnectionKey(key);
						setSelectedProjectId(null);
						setSource(MAIN_SOURCE);
						setSourceKind("main");
					})
					.catch((cause) => setError(connectionErrorMessage(cause)))
					.finally(() => setConnectingEnvironment(false));
			},
		}));

	const cloudProject =
		cloudCatalog.projects.find(
			(project) => project.projectId === cloudProjectId,
		) ?? cloudCatalog.projects[0];
	const cloudSandboxId =
		selectedCloudProvider(
			cloudCatalog.providers,
			cloudSandbox || cloudCatalog.image?.providerId || null,
			cloudCatalog.providers
				.filter(
					(provider) =>
						cloudSandboxStatus(
							cloudCatalog,
							provider.providerId,
							cloudProject?.projectId ?? null,
						).setup === "ready",
				)
				.map((provider) => provider.providerId),
		) ??
		cloudCatalog.image?.providerId ??
		"e2b";
	const cloudStatus = cloudSandboxStatus(
		cloudCatalog,
		cloudSandboxId,
		cloudProject?.projectId ?? null,
	);

	const cloudBlocked =
		cloudCatalog.providers.length > 0 && cloudStatus.setup !== "ready";
	const [rebuildingImage, setRebuildingImage] = useState(false);
	const rebuildCloudImage = async () => {
		setRebuildingImage(true);
		setError(null);
		try {
			await Effect.runPromise(
				cloudControlClient["cloud.image.build"]({
					mode: "update",
					providerId: cloudSandboxId,
					idempotencyKey: crypto.randomUUID(),
				}),
			);
			await refreshCloudCatalog();
		} catch (cause) {
			setError(connectionErrorMessage(cause));
		} finally {
			setRebuildingImage(false);
		}
	};

	const cloudOptions =
		account === null
			? []
			: cloudCatalog.providers.length === 0
				? [
						{
							key: "cloud",
							label: "Cloud",
							selected: cloudMode,
							onSelect: () => setCloudSandbox(""),
						},
					]
				: orderedCloudProviders(cloudCatalog.providers).map((provider) => {
						const status = cloudSandboxStatus(
							cloudCatalog,
							provider.providerId,
							cloudProject?.projectId ?? null,
						);
						return {
							key: `cloud:${provider.providerId}`,
							label: `Cloud · ${cloudProviderLabel(provider.providerId)}${status.label === null ? "" : ` — ${status.label}`}`,
							selected: cloudMode && cloudSandboxId === provider.providerId,
							onSelect: () => setCloudSandbox(provider.providerId),
						};
					});
	const destinationOptions = [
		...machineOptions,
		...environmentOptions,
		...cloudOptions,
	];
	const machineLabel = connectingEnvironment
		? "Connecting…"
		: cloudMode
			? `Cloud · ${cloudProviderLabel(cloudSandboxId)}`
			: (connections.find(
					(connection) => connection.key === effectiveConnectionKey,
				)?.label ?? (connections.length === 0 ? "No machines" : "Machine"));

	const projectOptions = projectChoices.map((item) => ({
		key: item.project.id,
		label: item.project.name,
		selected: item.project.id === effectiveProjectId,
		onSelect: () => {
			setSelectedProjectId(item.project.id);
			setSource(MAIN_SOURCE);
			setSourceKind("main");
		},
	}));
	const projectLabel =
		selectedProject?.name ?? (loading ? "Loading projects" : "Project");

	const firstSourceForKind = (kind: NewChatSourceKind): NewChatSource =>
		sourceOptionsForKind(kind, branches, prs)[0]?.source ?? MAIN_SOURCE;
	const sourceChoices = useMemo(
		() => sourceOptionsForKind(sourceKind, branches, prs),
		[branches, prs, sourceKind],
	);
	useEffect(() => {
		const next = reconcileNewChatSource({
			source,
			sourceKind,
			choices: sourceChoices,
			choicesLoaded: sourcesLoaded,
		});
		if (next !== source) setSource(next);
	}, [source, sourceChoices, sourceKind, sourcesLoaded]);
	const workModeOptions = WORK_MODE_OPTIONS.map((option) => ({
		key: option.kind,
		label: option.label,
		selected: sourceKind === option.kind,
		onSelect: () => {
			setSourceKind(option.kind);
			setSource(firstSourceForKind(option.kind));
		},
	}));

	const defaultBranchLabel =
		branches.find((branch) => branch.current)?.name ?? "main";
	const emptyBranchLabel =
		sourceKind === "worktree"
			? "Default branch"
			: sourceKind === "pr"
				? "No pull requests"
				: "No branches";
	const branchOptions = sourceChoices.map((option) => ({
		key: option.key,
		label: option.label,
		selected:
			option.source.kind === source.kind &&
			option.source.label === source.label,
		onSelect: () => setSource(option.source),
	}));
	const branchLabel =
		sourceKind === "main"
			? defaultBranchLabel
			: source.kind === sourceKind
				? source.label
				: emptyBranchLabel;

	const cloudProjectOptions = cloudCatalog.projects.map((project) => ({
		key: project.projectId,
		label: project.displayName,
		selected: project.projectId === cloudProject?.projectId,
		onSelect: () => setCloudProjectId(project.projectId),
	}));
	const cloudCanSubmit =
		account !== null &&
		cloudProject !== undefined &&
		!cloudBlocked &&
		cloudProviders.some((id) => id === effectiveModelMode.providerId) &&
		text.trim().length > 0 &&
		!submitting;
	const canSubmit = cloudMode
		? cloudCanSubmit
		: effectiveConnectionKey !== null &&
			selectedOptions !== null &&
			effectiveProjectId !== null &&
			text.trim().length > 0 &&
			!submitting &&
			(threadMode || sourceKind === "main" || source.kind === sourceKind) &&
			(!threadMode || threadContext !== null);

	const performCloudSubmit = async () => {
		if (!cloudCanSubmit || account === null || cloudProject === undefined)
			return;
		if (attachments.length > 0) {
			setError("Attachments aren't supported when starting a cloud chat yet.");
			return;
		}
		if (submitGateRef.current) return;
		submitGateRef.current = true;
		setSubmitting(true);
		setError(null);
		try {
			const result = await launchMobileCloudChat({
				accountId: account.id,
				draftKey,
				project: cloudProject,
				providerId: cloudSandboxId,
				agent: effectiveModelMode.providerId,
				model: effectiveModelMode.model,
				runtimeMode: effectiveModelMode.runtimeMode,
				text,
			});

			await KeyboardController.dismiss();
			router.replace({
				pathname: "/c/[conn]/session/[sessionId]",
				params: { conn: result.connectionKey, sessionId: result.sessionId },
			});
		} catch (cause) {
			setError(connectionErrorMessage(cause));
		} finally {
			submitGateRef.current = false;
			setSubmitting(false);
		}
	};

	const performSubmit = useCallback(async () => {
		const payload = buildNewChatCreatePayload({
			connectionKey: effectiveConnectionKey,
			projectId: effectiveProjectId,
			providerId: effectiveModelMode.providerId,
			model: effectiveModelMode.model,
			runtimeMode: effectiveModelMode.runtimeMode,
			permissionMode: effectiveModelMode.permissionMode,
			modelOptions: effectiveModelMode.modelOptions,
			source,
			text,
		});
		if (
			payload === null ||
			effectiveConnectionKey === null ||
			selectedOptions === null
		) {
			return;
		}
		if (submitGateRef.current) return;
		submitGateRef.current = true;
		setSubmitting(true);
		setError(null);
		try {
			const requiresRichSend = attachments.length > 0 || goalMode;
			if (threadMode && threadContext !== null) {
				const session = await createSession(
					effectiveConnectionKey,
					selectedOptions,
					{
						chatId: threadContext.chat.id,
						providerId: payload.providerId,
						model: payload.model,
						initialPrompt: requiresRichSend ? "" : payload.initialPrompt,
						runtimeMode: payload.runtimeMode,
						permissionMode: payload.permissionMode,
						modelOptions: payload.modelOptions,
					},
				);
				if (requiresRichSend) {
					const uploaded = await Promise.all(
						attachments.map((attachment) =>
							uploadComposerAttachment(selectedOptions, session.id, attachment),
						),
					);
					await Effect.runPromise(
						sendMessage({
							connection: selectedOptions,
							sessionId: session.id,
							input: makeTextInput(payload.initialPrompt, uploaded, goalMode),
							asGoal: goalMode,
						}),
					);
				}
				await KeyboardController.dismiss();
				clearComposerDraft(draftKey);
				router.replace(
					`/c/${encodeURIComponent(effectiveConnectionKey)}/session/${encodeURIComponent(session.id)}`,
				);
				return;
			}

			const worktreeId = payload.createWorktree
				? (
						await Effect.runPromise(
							createWorktree({
								connection: selectedOptions,
								projectId: payload.projectId,
								...(payload.createSource === null
									? {}
									: { source: payload.createSource }),
							}),
						)
					).id
				: payload.worktreeId;
			const result = await createChat(effectiveConnectionKey, selectedOptions, {
				projectId: payload.projectId,
				providerId: payload.providerId,
				model: payload.model,
				initialPrompt: requiresRichSend ? "" : payload.initialPrompt,
				runtimeMode: payload.runtimeMode,
				permissionMode: payload.permissionMode,
				modelOptions: payload.modelOptions,
				worktreeId,
			});
			if (requiresRichSend) {
				const uploaded = await Promise.all(
					attachments.map((attachment) =>
						uploadComposerAttachment(
							selectedOptions,
							result.initialSession.id,
							attachment,
						),
					),
				);
				await Effect.runPromise(
					sendMessage({
						connection: selectedOptions,
						sessionId: result.initialSession.id,
						input: makeTextInput(payload.initialPrompt, uploaded, goalMode),
						asGoal: goalMode,
					}),
				);
			}
			await KeyboardController.dismiss();
			clearComposerDraft(draftKey);
			router.replace(
				`/c/${encodeURIComponent(effectiveConnectionKey)}/session/${encodeURIComponent(
					result.initialSession.id,
				)}`,
			);
		} catch (cause) {
			setError(connectionErrorMessage(cause));
		} finally {
			submitGateRef.current = false;
			setSubmitting(false);
		}
	}, [
		attachments,
		draftKey,
		goalMode,
		effectiveModelMode,
		effectiveConnectionKey,
		selectedOptions,
		effectiveProjectId,
		source,
		text,
		threadContext,
		threadMode,
	]);

	const submit = () => {
		if (cloudMode) {
			void performCloudSubmit();
			return;
		}
		if (!threadMode || threadContext === null) {
			void performSubmit();
			return;
		}
		const siblingRunning = hasRunningChatThread(
			threadContext.threads,
			(thread) =>
				statusBySession[
					connectionSessionKey(effectiveConnectionKey ?? "", thread.id)
				] ?? thread.status,
		);
		if (!siblingRunning) {
			void performSubmit();
			return;
		}
		Alert.alert(
			"Another thread is running",
			"Both threads share this workspace and may edit the same files. Start this thread anyway?",
			[
				{ text: "Cancel", style: "cancel" },
				{ text: "Start thread", onPress: () => void performSubmit() },
			],
		);
	};
	const addAttachments = (
		pick: () => Promise<LocalComposerAttachment[]>,
	): void => {
		void pick()
			.then((items) => setAttachments((current) => [...current, ...items]))
			.catch((cause) => setError(connectionErrorMessage(cause)));
	};

	return (
		<KeyboardAvoidingView
			behavior="padding"
			keyboardVerticalOffset={keyboardOffset}
			className="flex-1 bg-background"
		>
			<Stack.Screen
				options={{
					title: threadMode ? "New Thread" : "New Chat",
					headerBackVisible: false,
				}}
			/>
			<Stack.Toolbar placement="left">
				<Stack.Toolbar.Button
					icon={toolbarIcon("chevron.left")}
					separateBackground
					onPress={() => router.back()}
				/>
			</Stack.Toolbar>
			<ScrollView
				className="flex-1"
				contentInsetAdjustmentBehavior="automatic"
				keyboardShouldPersistTaps="handled"
				contentContainerStyle={{
					padding: 18,
					paddingHorizontal: 18 + readableInset,
					paddingBottom: 24,
					gap: 18,
					flexGrow: 1,
				}}
			>
				<View className="flex-1" />

				{error === null ? null : (
					<Text
						selectable
						className="font-sans text-[13px] leading-5 text-danger"
					>
						{error}
					</Text>
				)}
			</ScrollView>

			<View
				className="pt-2"
				style={{
					paddingHorizontal: 12 + readableInset,
					paddingBottom: insets.bottom > 0 ? insets.bottom : 12,
				}}
			>
				{threadMode && threadContext !== null ? (
					<View className="mb-4 gap-1 px-2">
						<Text className="font-sans-medium text-[15px] text-foreground">
							{threadContext.chat.title}
						</Text>
						<Text className="font-sans text-[12px] text-muted-foreground">
							{threadContext.project.name} · current workspace
						</Text>
					</View>
				) : (
					<View className="mb-4 gap-1 px-1">
						<SelectorRow
							symbol={cloudMode ? "cloud" : "laptopcomputer"}
							label={machineLabel}
							options={destinationOptions}
							emptyLabel="No machines"
						/>
						{cloudMode ? (
							<>
								<SelectorRow
									symbol="folder"
									label={cloudProject?.displayName ?? "No cloud repositories"}
									options={cloudProjectOptions}
									emptyLabel="No cloud repositories"
								/>
								{cloudBlocked && cloudStatus.label !== null ? (
									<View className="flex-row items-center gap-2 px-1">
										<Text className="flex-1 font-sans text-[13px] text-muted-foreground">
											{cloudStatus.label}
										</Text>
										{cloudStatus.setup === "update-image" ||
										cloudStatus.setup === "rebuild-authentication" ? (
											<Button
												size="sm"
												variant="ghost"
												disabled={rebuildingImage}
												onPress={() =>
													cloudStatus.setup === "rebuild-authentication"
														? router.push("/cloud-auth")
														: void rebuildCloudImage()
												}
											>
												{cloudStatus.setup === "rebuild-authentication"
													? "Reconnect"
													: "Update Image"}
											</Button>
										) : null}
									</View>
								) : null}
								{cloudCatalog.providers.length === 0 &&
								cloudCatalog.providersError ? (
									<Text className="px-1 font-sans text-[13px] text-danger">
										{cloudCatalog.providersError}
									</Text>
								) : null}
							</>
						) : (
							<>
								<SelectorRow
									symbol="folder"
									label={projectLabel}
									options={projectOptions}
									emptyLabel={loading ? "Loading projects" : "No projects"}
								/>
								<SelectorRow
									symbol="desktopcomputer"
									label={workModeLabel(sourceKind)}
									options={workModeOptions}
								/>
								<SelectorRow
									symbol="arrow.triangle.branch"
									label={branchLabel}
									options={branchOptions}
									disabled={sourceKind === "main"}
									emptyLabel={emptyBranchLabel}
								/>
							</>
						)}
					</View>
				)}
				<GlassSurface
					style={{
						gap: 8,
						padding: 10,
					}}
				>
					<ComposerAttachmentStrip
						attachments={attachments}
						onRemove={(id) =>
							setAttachments((current) =>
								current.filter((item) => item.id !== id),
							)
						}
					/>
					<ComposerInputFrame
						input={
							<TextInput
								className="max-h-36 min-h-12 px-1 py-2 font-sans text-[17px] leading-6 text-foreground"
								multiline
								placeholder="Ask Zuse"
								placeholderTextColor={colors.tertiaryFg}
								value={text}
								onChangeText={setText}
							/>
						}
						leadingAction={
							<View className="flex-row items-center gap-1">
								<ComposerActionSlot>
									<ComposerPlusMenu
										goalMode={goalMode}
										goalSupported={goalSupported}
										planMode={effectiveModelMode.permissionMode === "plan"}
										onCaptureImage={() => addAttachments(captureComposerImage)}
										onPickImages={() => addAttachments(pickComposerImages)}
										onPickFiles={() => addAttachments(pickComposerFiles)}
										onToggleGoal={setGoalMode}
										onTogglePlan={(next) =>
											setModelMode((value) => ({
												...value,
												permissionMode: next ? "plan" : "default",
											}))
										}
									/>
								</ComposerActionSlot>
								<ComposerActionSlot>
									<ComposerApprovalMenu
										runtimeMode={effectiveModelMode.runtimeMode}
										onChange={(runtimeMode) =>
											setModelMode((value) => ({ ...value, runtimeMode }))
										}
									/>
								</ComposerActionSlot>
								{effectiveModelMode.permissionMode === "plan" ? (
									<ComposerModeChip
										label="Plan"
										plan
										onClear={() =>
											setModelMode((value) => ({
												...value,
												permissionMode: "default",
											}))
										}
									/>
								) : null}
								{goalMode ? (
									<ComposerModeChip
										label="Goal"
										onClear={() => setGoalMode(false)}
									/>
								) : null}
							</View>
						}
						trailingAction={
							<View className="min-w-0 flex-row items-center gap-1.5">
								<ModelSheetTrigger
									value={effectiveModelMode}
									onPress={() => setModelSheetOpen(true)}
								/>
								<ComposerSendButton
									online={cloudMode || selectedOptions !== null}
									busy={submitting}
									disabled={!canSubmit}
									onPress={() => void submit()}
								/>
							</View>
						}
					/>
				</GlassSurface>
				<ModelSheet
					open={modelSheetOpen}
					onOpenChange={setModelSheetOpen}
					value={effectiveModelMode}
					availableProviders={availableProviders}
					strictProviders={
						cloudMode || selectedOptions?.cloudWorkspaceId !== undefined
					}
					canChangeProvider
					canChangeReasoning
					onChange={setModelMode}
				/>
			</View>
		</KeyboardAvoidingView>
	);
}
