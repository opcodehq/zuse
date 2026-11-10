import { type SFSymbol, SymbolView } from "expo-symbols";
import type { LucideIcon } from "lucide-react-native";
import { Children, isValidElement } from "react";
import {
	Pressable,
	type PressableProps,
	Text,
	View,
	type ViewProps,
} from "react-native";

import { cn } from "~/lib/cn";
import { lightTap } from "~/lib/haptics";
import { ANDROID_RIPPLE_ROW, RIPPLE } from "~/lib/ripple";
import { platformSymbolName } from "~/lib/symbol-names";
import { colors } from "~/theme";

const CONTINUOUS = { borderCurve: "continuous" } as const;

type ListSectionProps = ViewProps & {
	header?: string;

	footer?: string;
};

export function ListSection({
	header,
	footer,
	children,
	className,
	...rest
}: ListSectionProps) {
	const rows = Children.toArray(children).filter(isValidElement);
	return (
		<View className={cn("gap-2", className)} {...rest}>
			{header ? (
				<Text className="px-2 font-sans text-[15px] text-muted-foreground">
					{header}
				</Text>
			) : null}
			<View style={CONTINUOUS} className="overflow-hidden rounded-3xl bg-card">
				{rows.map((row, index) => (
					<View key={row.key}>
						{index > 0 ? <View className="ml-4 h-px bg-border" /> : null}
						{row}
					</View>
				))}
			</View>
			{footer ? (
				<Text className="px-4 font-sans text-[13px] leading-4 text-muted-foreground">
					{footer}
				</Text>
			) : null}
		</View>
	);
}

type ListRowProps = Omit<PressableProps, "children"> & {
	title: string;
	subtitle?: string;

	icon?: LucideIcon;

	symbol?: SFSymbol;
	iconColor?: string;

	iconTone?: "brand" | "neutral";

	leading?: React.ReactNode;

	value?: string;

	trailing?: React.ReactNode;

	chevron?: boolean;
	destructive?: boolean;

	analyticsId?: string;
};

export function ListRow({
	title,
	subtitle,
	icon: Icon,
	symbol,
	iconColor,
	iconTone = "neutral",
	leading,
	value,
	trailing,
	chevron,
	destructive,
	analyticsId,
	onPress,
	disabled,
	className,
	...rest
}: ListRowProps) {
	const showChevron = chevron ?? onPress != null;
	return (
		<Pressable
			disabled={disabled}
			android_ripple={onPress ? RIPPLE : undefined}
			onPress={
				onPress
					? (event) => {
							if (analyticsId) {
								void import("~/lib/analytics").then(
									({ captureMobileControl }) =>
										captureMobileControl(analyticsId),
								);
							}
							lightTap();
							onPress(event);
						}
					: undefined
			}
			className={cn(
				"min-h-[54px] flex-row items-center gap-3 px-4 py-2.5 active:bg-card-elevated",
				disabled && "opacity-40",
				className,
				ANDROID_RIPPLE_ROW,
			)}
			{...rest}
		>
			{leading ??
				(symbol || Icon ? (
					<View
						style={CONTINUOUS}
						className={cn(
							"h-8 w-8 items-center justify-center rounded-lg",
							iconTone === "brand" ? "bg-primary" : "bg-muted",
						)}
					>
						{symbol ? (
							<SymbolView
								name={platformSymbolName(symbol)}
								size={17}
								weight="semibold"
								tintColor={
									iconColor ??
									(iconTone === "brand" ? colors.primaryForeground : colors.fg)
								}
							/>
						) : Icon ? (
							<Icon
								size={17}
								color={
									iconColor ??
									(iconTone === "brand" ? colors.primaryForeground : colors.fg)
								}
							/>
						) : null}
					</View>
				) : null)}
			<View className="min-w-0 flex-1">
				<Text
					className={cn(
						"font-sans text-[17px]",
						destructive ? "text-danger" : "text-foreground",
					)}
					numberOfLines={1}
				>
					{title}
				</Text>
				{subtitle ? (
					<Text
						className="mt-0.5 font-sans text-[13px] text-muted-foreground"
						numberOfLines={1}
					>
						{subtitle}
					</Text>
				) : null}
			</View>
			{value ? (
				<Text
					className="max-w-[45%] font-sans text-[17px] text-muted-foreground"
					numberOfLines={1}
				>
					{value}
				</Text>
			) : null}
			{trailing}
			{showChevron ? (
				<SymbolView
					name={platformSymbolName("chevron.right")}
					size={13}
					weight="semibold"
					tintColor={colors.tertiaryFg}
				/>
			) : null}
		</Pressable>
	);
}
