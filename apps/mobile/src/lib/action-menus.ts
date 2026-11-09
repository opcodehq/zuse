import type { RuntimeMode } from "@zuse/contracts";

import type { MenuItem } from "./menu-items";
import { RUNTIME_OPTIONS } from "./model-options";

/**
 * The single definition of each anchored action menu. The iOS (SwiftUI) and
 * Android (Material) renderers draw these trees, so both platforms always offer
 * the same actions, labels, and selection state.
 */

export const approvalMenuItems = (
	runtimeMode: RuntimeMode,
	onChange: (mode: RuntimeMode) => void,
): MenuItem[] =>
	RUNTIME_OPTIONS.map((option) => ({
		id: option.value,
		title: option.label,
		systemImage: option.systemImage,
		checked: runtimeMode === option.value,
		destructive: option.value === "full-access",
		onSelect: () => onChange(option.value),
	}));

export type ComposerPlusMenuProps = {
	goalMode: boolean;
	goalSupported: boolean;
	planMode: boolean;
	onCaptureImage: () => void;
	onPickImages: () => void;
	onPickFiles: () => void;
	onToggleGoal: (next: boolean) => void;
	onTogglePlan: (next: boolean) => void;
};

export const composerPlusMenuItems = (
	props: ComposerPlusMenuProps,
): MenuItem[] => [
	{
		id: "take-photo",
		title: "Take photo",
		systemImage: "camera",
		onSelect: props.onCaptureImage,
	},
	{
		id: "choose-photos",
		title: "Choose photos",
		systemImage: "photo.on.rectangle",
		onSelect: props.onPickImages,
	},
	{
		id: "choose-files",
		title: "Choose files",
		systemImage: "doc",
		onSelect: props.onPickFiles,
	},
	...(props.goalSupported
		? [
				{
					id: "goal",
					title: "Add goal",
					systemImage: "target",
					checked: props.goalMode,
					onSelect: () => props.onToggleGoal(!props.goalMode),
				},
			]
		: []),
	{
		id: "plan",
		title: "Plan mode",
		systemImage: "checklist",
		checked: props.planMode,
		onSelect: () => props.onTogglePlan(!props.planMode),
	},
];

export type SelectorOption = {
	key: string;
	label: string;
	selected: boolean;
	onSelect: () => void;
};

export type SelectorRowProps = {
	symbol: string;
	label: string;
	options: readonly SelectorOption[];
	disabled?: boolean;
	emptyLabel?: string;
	compact?: boolean;
};

/** A disabled or empty selector still opens, showing why nothing is listed. */
export const selectorMenuItems = (
	options: readonly SelectorOption[],
	{ disabled, emptyLabel }: { disabled: boolean; emptyLabel: string },
): MenuItem[] =>
	disabled || options.length === 0
		? [{ id: "empty", title: emptyLabel }]
		: options.map((option) => ({
				id: option.key,
				title: option.label,
				checked: option.selected,
				onSelect: option.onSelect,
			}));

export type SessionActionsMenuProps = {
	isPinned: boolean;
	onNewChat: () => void;
	onPin?: () => void;
	onRenameChat?: () => void;
	onRenameSession?: () => void;
	onRenameBranch?: () => void;
	onThreads: () => void;
	onChanges: () => void;
	onFiles: () => void;
	onAttempts?: () => void;
	onCheckpoints?: () => void;
	onTerminal?: () => void;
	onOpenOnDesktop?: () => void;
	onMergeBack?: () => void;
	onArchive: () => void;
};

const optionalItem = (
	onSelect: (() => void) | undefined,
	item: Omit<MenuItem, "onSelect">,
): MenuItem[] => (onSelect === undefined ? [] : [{ ...item, onSelect }]);

/** The header overflow menu; "New chat" stays a separate header button. */
export const sessionActionMenuItems = (
	props: SessionActionsMenuProps,
	labels: { checkpoints: string; attempts: string },
): MenuItem[] => [
	...optionalItem(props.onPin, {
		id: "pin",
		title: props.isPinned ? "Unpin" : "Pin",
		systemImage: props.isPinned ? "pin.slash" : "pin",
	}),
	...optionalItem(props.onRenameChat, {
		id: "rename-chat",
		title: "Rename chat",
		systemImage: "pencil",
	}),
	...optionalItem(props.onRenameSession, {
		id: "rename-session",
		title: "Rename session",
		systemImage: "rectangle.and.pencil.and.ellipsis",
	}),
	...optionalItem(props.onRenameBranch, {
		id: "rename-branch",
		title: "Rename branch",
		systemImage: "arrow.triangle.branch",
	}),
	{
		id: "threads",
		title: "Threads",
		systemImage: "rectangle.stack",
		onSelect: props.onThreads,
	},
	{
		id: "changes",
		title: "Changes",
		systemImage: "arrow.triangle.branch",
		onSelect: props.onChanges,
	},
	{
		id: "files",
		title: "Files",
		systemImage: "folder",
		onSelect: props.onFiles,
	},
	...optionalItem(props.onCheckpoints, {
		id: "checkpoints",
		title: labels.checkpoints,
		systemImage: "clock.arrow.circlepath",
	}),
	...optionalItem(props.onAttempts, {
		id: "attempts",
		title: labels.attempts,
		systemImage: "clock.arrow.circlepath",
	}),
	...optionalItem(props.onTerminal, {
		id: "terminal",
		title: "Terminal",
		systemImage: "terminal",
	}),
	...optionalItem(props.onOpenOnDesktop, {
		id: "open-on-desktop",
		title: "Open on desktop",
		systemImage: "macbook",
	}),
	...optionalItem(props.onMergeBack, {
		id: "merge-back",
		title: "Merge back to parent",
		systemImage: "arrow.triangle.merge",
	}),
	{
		id: "archive",
		title: "Archive",
		systemImage: "archivebox",
		destructive: true,
		onSelect: props.onArchive,
	},
];

export type ForkFromMessageMenuProps = {
	onForkInChat: () => void;
	onForkInCurrentWorktree: () => void;
	onForkInNewWorktree: () => void;
};

export const forkFromMessageMenuItems = (
	props: ForkFromMessageMenuProps,
): MenuItem[] => [
	{
		id: "fork-in-chat",
		title: "Fork in this chat",
		systemImage: "rectangle.stack.badge.plus",
		onSelect: props.onForkInChat,
	},
	{
		id: "fork-to-new-chat",
		title: "Fork to a new chat",
		systemImage: "plus.bubble",
		children: [
			{
				id: "fork-current-worktree",
				title: "Use current worktree",
				systemImage: "folder",
				onSelect: props.onForkInCurrentWorktree,
			},
			{
				id: "fork-new-worktree",
				title: "Create isolated worktree",
				systemImage: "arrow.triangle.branch",
				onSelect: props.onForkInNewWorktree,
			},
		],
	},
];
