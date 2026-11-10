import { describe, expect, test, vi } from "vitest";

import {
	type MenuItem,
	menuItemSystemImage,
} from "../../../src/lib/menu-items";

const items: MenuItem[] = [
	{ id: "plain", title: "Plain", systemImage: "folder", onSelect: vi.fn() },
	{ id: "picked", title: "Picked", systemImage: "target", checked: true },
	{ id: "danger", title: "Danger", destructive: true },
	{
		id: "group",
		title: "Group",
		systemImage: "plus.bubble",
		children: [{ id: "leaf", title: "Leaf", onSelect: vi.fn() }],
	},
	{
		id: "section",
		title: "Section",
		inline: true,
		children: [{ id: "inline-leaf", title: "Inline leaf" }],
	},
];

describe("menu items", () => {
	test("iOS swaps a checked item's icon for a checkmark", () => {
		expect(menuItemSystemImage(items[0] as MenuItem)).toBe("folder");
		expect(menuItemSystemImage(items[1] as MenuItem)).toBe("checkmark");
		expect(menuItemSystemImage(items[2] as MenuItem)).toBeUndefined();
	});
});
