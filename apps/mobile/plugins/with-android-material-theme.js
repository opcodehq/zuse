const {
	withAndroidColors,
	withAndroidColorsNight,
	withAndroidStyles,
	withAppBuildGradle,
	withDangerousMod,
} = require("@expo/config-plugins");
const fs = require("node:fs");
const path = require("node:path");

/**
 * Native Android chrome (alerts, pickers, text selection, the window behind
 * React Native) follows Material 3 in the app palette from `global.css`
 * instead of the template's AppCompat theme and blue primary.
 */
const MATERIAL_DEPENDENCY = "com.google.android.material:material:1.13.0";

const PALETTE = {
	light: {
		zuse_primary: "#486900",
		zuse_background: "#F7F7F5",
		zuse_surface: "#FFFFFF",
		zuse_on_surface: "#171716",
		zuse_on_surface_variant: "#6F6F6B",
		zuse_outline: "#1A171716",
	},
	dark: {
		zuse_primary: "#89CA21",
		zuse_background: "#0F0F0F",
		zuse_surface: "#1F1F1F",
		zuse_on_surface: "#EDEDED",
		zuse_on_surface_variant: "#ADADAD",
		zuse_outline: "#1AFFFFFF",
	},
};

const THEME_ITEMS = {
	colorPrimary: "@color/zuse_primary",
	colorOnPrimary: "#FFFFFF",
	colorSecondary: "@color/zuse_primary",
	colorSurface: "@color/zuse_surface",
	colorSurfaceContainer: "@color/zuse_surface",
	colorSurfaceContainerHigh: "@color/zuse_surface",
	colorSurfaceContainerHighest: "@color/zuse_surface",
	colorOnSurface: "@color/zuse_on_surface",
	colorOnSurfaceVariant: "@color/zuse_on_surface_variant",
	colorOutline: "@color/zuse_outline",
	"android:windowBackground": "@color/zuse_background",
	alertDialogTheme: "@style/ZuseAlertDialog",
	materialAlertDialogTheme: "@style/ZuseAlertDialog",
};

const DIALOG_BACKGROUND = `<?xml version="1.0" encoding="utf-8"?>
<inset xmlns:android="http://schemas.android.com/apk/res/android"
  android:insetLeft="24dp"
  android:insetTop="24dp"
  android:insetRight="24dp"
  android:insetBottom="24dp">
  <shape android:shape="rectangle">
    <corners android:radius="28dp" />
    <solid android:color="@color/zuse_surface" />
  </shape>
</inset>
`;

const setColors = (resources, colors) => {
	const existing = (resources.color ?? []).filter(
		(color) => !(color.$.name in colors),
	);
	resources.color = [
		...existing,
		...Object.entries(colors).map(([name, value]) => ({
			$: { name },
			_: value,
		})),
	];
};

const upsertStyle = (resources, name, parent, items) => {
	const styles = resources.style ?? [];
	let style = styles.find((entry) => entry.$.name === name);
	if (style === undefined) {
		style = { $: { name, parent }, item: [] };
		styles.push(style);
	}
	style.$.parent = parent;
	const kept = (style.item ?? []).filter((item) => !(item.$.name in items));
	style.item = [
		...kept,
		...Object.entries(items).map(([itemName, value]) => ({
			$: { name: itemName },
			_: value,
		})),
	];
	resources.style = styles;
};

module.exports = function withAndroidMaterialTheme(config) {
	config = withAndroidColors(config, (mod) => {
		setColors(mod.modResults.resources, PALETTE.light);
		return mod;
	});
	config = withAndroidColorsNight(config, (mod) => {
		setColors(mod.modResults.resources, PALETTE.dark);
		return mod;
	});
	config = withAndroidStyles(config, (mod) => {
		const resources = mod.modResults.resources;
		upsertStyle(
			resources,
			"AppTheme",
			"Theme.Material3.DayNight.NoActionBar",
			THEME_ITEMS,
		);
		upsertStyle(
			resources,
			"ZuseAlertDialog",
			"ThemeOverlay.Material3.MaterialAlertDialog",
			{
				colorPrimary: "@color/zuse_primary",
				"android:windowBackground": "@drawable/zuse_dialog_background",
			},
		);
		return mod;
	});
	config = withAppBuildGradle(config, (mod) => {
		if (
			!mod.modResults.contents.includes("com.google.android.material:material")
		) {
			mod.modResults.contents = mod.modResults.contents.replace(
				/dependencies\s*\{/,
				(match) => `${match}\n    implementation("${MATERIAL_DEPENDENCY}")`,
			);
		}
		return mod;
	});
	return withDangerousMod(config, [
		"android",
		(mod) => {
			const drawable = path.join(
				mod.modRequest.platformProjectRoot,
				"app/src/main/res/drawable",
			);
			fs.mkdirSync(drawable, { recursive: true });
			fs.writeFileSync(
				path.join(drawable, "zuse_dialog_background.xml"),
				DIALOG_BACKGROUND,
			);
			return mod;
		},
	]);
};
