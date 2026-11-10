import {
	Text as ComposeText,
	DropdownMenu,
	DropdownMenuItem,
	HorizontalDivider,
	Host,
	Icon,
	RNHostView,
} from "@expo/ui/jetpack-compose";
import type { SFSymbol } from "expo-symbols";
import { type ReactNode, useCallback, useState } from "react";
import { type StyleProp, View, type ViewStyle } from "react-native";
import { useUniwind } from "uniwind";

import type { MenuItem } from "~/lib/menu-items";
import { type ToolbarIcon, useToolbarIcons } from "~/lib/toolbar-icons";
import { colors } from "~/theme";

type IconSource = (name: SFSymbol) => ToolbarIcon | undefined;

/**
 * Anchored Material dropdown for platforms without SwiftUI menus. Draws the same
 * `MenuItem` tree the iOS menus use, in the app palette rather than the
 * Material baseline.
 *
 * `children` receives `open` so the trigger can be a regular `Pressable` with
 * its own pressed state and accessibility label. With `stretch`, the trigger
 * spans the available width (the native anchor otherwise hugs its content).
 */
export function ActionMenu({
	items,
	stretch = false,
	style,
	children,
}: {
	items: readonly MenuItem[];
	stretch?: boolean;
	style?: StyleProp<ViewStyle>;
	children: (open: () => void) => ReactNode;
}) {
	const [expanded, setExpanded] = useState(false);
	const [width, setWidth] = useState<number | undefined>(undefined);
	const { theme } = useUniwind();
	const icon = useToolbarIcons();
	const open = useCallback(() => setExpanded(true), []);
	const close = useCallback(() => setExpanded(false), []);

	return (
		<View
			style={style}
			onLayout={
				stretch
					? (event) => setWidth(event.nativeEvent.layout.width)
					: undefined
			}
		>
			<Host matchContents colorScheme={theme === "dark" ? "dark" : "light"}>
				<DropdownMenu
					expanded={expanded}
					onDismissRequest={close}
					color={colors.cardElevated}
				>
					<DropdownMenu.Trigger>
						<RNHostView matchContents>
							<View style={stretch ? { width } : undefined}>
								{children(open)}
							</View>
						</RNHostView>
					</DropdownMenu.Trigger>
					<DropdownMenu.Items>
						<MenuEntries items={items} icon={icon} dismiss={close} />
					</DropdownMenu.Items>
				</DropdownMenu>
			</Host>
		</View>
	);
}

function MenuEntries({
	items,
	icon,
	dismiss,
}: {
	items: readonly MenuItem[];
	icon: IconSource;
	dismiss: () => void;
}) {
	return items.map((item, index) =>
		item.children !== undefined && item.inline === true ? (
			<View key={item.id}>
				{index === 0 ? null : <HorizontalDivider color={colors.border} />}
				<MenuEntries items={item.children} icon={icon} dismiss={dismiss} />
				{index === items.length - 1 ? null : (
					<HorizontalDivider color={colors.border} />
				)}
			</View>
		) : item.children !== undefined ? (
			<Submenu key={item.id} item={item} icon={icon} dismiss={dismiss} />
		) : (
			<MenuEntry key={item.id} item={item} icon={icon} dismiss={dismiss} />
		),
	);
}

const imageOf = (icon: IconSource, name: string | undefined) => {
	if (name === undefined) return undefined;
	const source = icon(name as SFSymbol);
	return typeof source === "string" ? undefined : source;
};

function itemColors(item: MenuItem) {
	const text = item.destructive === true ? colors.danger : colors.fg;
	return {
		textColor: text,
		disabledTextColor: colors.tertiaryFg,
		leadingIconColor:
			item.destructive === true ? colors.danger : colors.secondaryFg,
		disabledLeadingIconColor: colors.tertiaryFg,
		trailingIconColor: colors.accent,
	};
}

function ItemContent({
	item,
	icon,
	trailing,
}: {
	item: MenuItem;
	icon: IconSource;
	trailing?: SFSymbol;
}) {
	const leading = imageOf(icon, item.systemImage);
	const trailingImage = imageOf(icon, trailing);
	return (
		<>
			<DropdownMenuItem.Text>
				<ComposeText>{item.title}</ComposeText>
			</DropdownMenuItem.Text>
			{leading === undefined ? null : (
				<DropdownMenuItem.LeadingIcon>
					<Icon source={leading} size={20} />
				</DropdownMenuItem.LeadingIcon>
			)}
			{trailingImage === undefined ? null : (
				<DropdownMenuItem.TrailingIcon>
					<Icon source={trailingImage} size={20} />
				</DropdownMenuItem.TrailingIcon>
			)}
		</>
	);
}

function MenuEntry({
	item,
	icon,
	dismiss,
}: {
	item: MenuItem;
	icon: IconSource;
	dismiss: () => void;
}) {
	return (
		<DropdownMenuItem
			elementColors={itemColors(item)}
			onClick={() => {
				dismiss();
				item.onSelect?.();
			}}
		>
			<ItemContent
				item={item}
				icon={icon}
				trailing={item.checked === true ? "checkmark" : undefined}
			/>
		</DropdownMenuItem>
	);
}

function Submenu({
	item,
	icon,
	dismiss,
}: {
	item: MenuItem;
	icon: IconSource;
	dismiss: () => void;
}) {
	const [expanded, setExpanded] = useState(false);
	return (
		<DropdownMenu
			expanded={expanded}
			onDismissRequest={() => setExpanded(false)}
			color={colors.cardElevated}
		>
			<DropdownMenu.Trigger>
				<DropdownMenuItem
					elementColors={itemColors(item)}
					onClick={() => setExpanded(true)}
				>
					<ItemContent item={item} icon={icon} trailing="chevron.right" />
				</DropdownMenuItem>
			</DropdownMenu.Trigger>
			<DropdownMenu.Items>
				<MenuEntries
					items={item.children ?? []}
					icon={icon}
					dismiss={() => {
						setExpanded(false);
						dismiss();
					}}
				/>
			</DropdownMenu.Items>
		</DropdownMenu>
	);
}
