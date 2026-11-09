import { describe, expect, test, vi } from "vitest";

import {
	approvalMenuItems,
	composerPlusMenuItems,
	forkFromMessageMenuItems,
	selectorMenuItems,
	sessionActionMenuItems,
} from "../../../src/lib/action-menus";
import type { MenuItem } from "../../../src/lib/menu-items";

const ids = (items: readonly MenuItem[]) => items.map((item) => item.id);

describe("approval menu", () => {
	test("lists runtime modes, checks the current one, and flags full access", () => {
		const onChange = vi.fn();
		const items = approvalMenuItems("auto", onChange);
		expect(ids(items)).toEqual(["approval-required", "auto", "full-access"]);
		expect(items.map((item) => item.checked)).toEqual([false, true, false]);
		expect(items.find((item) => item.destructive)?.id).toBe("full-access");
		items[2]?.onSelect?.();
		expect(onChange).toHaveBeenCalledWith("full-access");
	});
});

describe("composer plus menu", () => {
	const props = {
		goalMode: false,
		goalSupported: true,
		planMode: true,
		onCaptureImage: vi.fn(),
		onPickImages: vi.fn(),
		onPickFiles: vi.fn(),
		onToggleGoal: vi.fn(),
		onTogglePlan: vi.fn(),
	};

	test("offers attachments and both send modes", () => {
		const items = composerPlusMenuItems(props);
		expect(items.map((item) => item.title)).toEqual([
			"Take photo",
			"Choose photos",
			"Choose files",
			"Add goal",
			"Plan mode",
		]);
		expect(items.find((item) => item.id === "plan")?.checked).toBe(true);
		expect(items.find((item) => item.id === "goal")?.checked).toBe(false);
	});

	test("toggles modes and hides Goal when unsupported", () => {
		const items = composerPlusMenuItems(props);
		items.find((item) => item.id === "goal")?.onSelect?.();
		items.find((item) => item.id === "plan")?.onSelect?.();
		expect(props.onToggleGoal).toHaveBeenCalledWith(true);
		expect(props.onTogglePlan).toHaveBeenCalledWith(false);
		expect(
			ids(composerPlusMenuItems({ ...props, goalSupported: false })),
		).not.toContain("goal");
	});
});

describe("selector menu", () => {
	const options = [
		{ key: "a", label: "Alpha", selected: false, onSelect: vi.fn() },
		{ key: "b", label: "Beta", selected: true, onSelect: vi.fn() },
	];

	test("checks the selected option", () => {
		const items = selectorMenuItems(options, {
			disabled: false,
			emptyLabel: "None",
		});
		expect(items.map((item) => [item.id, item.checked])).toEqual([
			["a", false],
			["b", true],
		]);
	});

	test("shows the empty label when disabled or empty", () => {
		for (const [list, disabled] of [
			[options, true],
			[[], false],
		] as const) {
			expect(
				selectorMenuItems(list, { disabled, emptyLabel: "No projects" }),
			).toEqual([{ id: "empty", title: "No projects" }]);
		}
	});
});

describe("session actions menu", () => {
	const labels = { checkpoints: "Checkpoints", attempts: "Attempts" };
	const required = {
		isPinned: false,
		onNewChat: vi.fn(),
		onThreads: vi.fn(),
		onChanges: vi.fn(),
		onFiles: vi.fn(),
		onArchive: vi.fn(),
	};

	test("always offers navigation and archive, last and destructive", () => {
		const items = sessionActionMenuItems(required, labels);
		expect(ids(items)).toEqual(["threads", "changes", "files", "archive"]);
		expect(items.at(-1)?.destructive).toBe(true);
	});

	test("adds each optional action only when it has a handler", () => {
		const noop = vi.fn();
		const items = sessionActionMenuItems(
			{
				...required,
				isPinned: true,
				onPin: noop,
				onRenameChat: noop,
				onRenameSession: noop,
				onRenameBranch: noop,
				onAttempts: noop,
				onCheckpoints: noop,
				onTerminal: noop,
				onOpenOnDesktop: noop,
				onMergeBack: noop,
			},
			labels,
		);
		expect(ids(items)).toEqual([
			"pin",
			"rename-chat",
			"rename-session",
			"rename-branch",
			"threads",
			"changes",
			"files",
			"checkpoints",
			"attempts",
			"terminal",
			"open-on-desktop",
			"merge-back",
			"archive",
		]);
		expect(items[0]?.title).toBe("Unpin");
		expect(items.find((item) => item.id === "attempts")?.title).toBe(
			"Attempts",
		);
	});
});

describe("fork from message menu", () => {
	test("nests the new-chat destinations in a submenu", () => {
		const props = {
			onForkInChat: vi.fn(),
			onForkInCurrentWorktree: vi.fn(),
			onForkInNewWorktree: vi.fn(),
		};
		const [inChat, toNewChat] = forkFromMessageMenuItems(props);
		expect(inChat?.title).toBe("Fork in this chat");
		expect(toNewChat?.title).toBe("Fork to a new chat");
		expect(toNewChat?.inline).toBeUndefined();
		expect(toNewChat?.children?.map((item) => item.title)).toEqual([
			"Use current worktree",
			"Create isolated worktree",
		]);
		toNewChat?.children?.[1]?.onSelect?.();
		expect(props.onForkInNewWorktree).toHaveBeenCalledTimes(1);
	});
});
