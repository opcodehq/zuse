import { requireOptionalNativeModule } from "expo";
import { Platform, Share } from "react-native";

type NativeModule = {
	readonly presentQuickLook: (uri: string) => boolean;
	readonly beginBackgroundTask: () => Promise<number>;
	readonly endBackgroundTask: (id: number) => Promise<void>;
	readonly saveImageToPhotos: (uri: string) => Promise<boolean>;
	/** Android only: system share sheet for a local file. */
	readonly shareFile?: (uri: string, title: string | null) => Promise<void>;
};

const Native = requireOptionalNativeModule<NativeModule>("ZuseMobilePlatform");

export const presentQuickLook = (uri: string): boolean =>
	Native?.presentQuickLook(uri) === true;

export const beginBestEffortBackgroundTask = async (): Promise<number> =>
	(await Native?.beginBackgroundTask()) ?? 0;

export const endBestEffortBackgroundTask = async (
	id: number,
): Promise<void> => {
	if (id !== 0) await Native?.endBackgroundTask(id);
};

/**
 * Saves a local image to the system photo library (iOS Photos, Android
 * Pictures/Zuse). `false` means the platform refused: missing photo access on
 * iOS, or an Android version without scoped-storage saving.
 */
export const saveImageToPhotos = async (uri: string): Promise<boolean> =>
	(await Native?.saveImageToPhotos(uri)) === true;

export const photoLibraryName = (): string =>
	Platform.OS === "ios" ? "Photos" : "Pictures";

/** Explains a `false` result from {@link saveImageToPhotos}. */
export const saveImageRefusedMessage = (): string =>
	Platform.OS === "ios"
		? "Allow photo-library access in iPhone Settings and try again."
		: "Saving images requires Android 10 or later.";

/**
 * The one file-sharing path for the app. React Native's `Share` ignores `url`
 * on Android, so Android hands the file to the system share sheet through the
 * native module's FileProvider instead.
 */
export const shareFile = async (
	uri: string,
	options: { readonly title?: string } = {},
): Promise<void> => {
	if (Platform.OS !== "android") {
		await Share.share(
			options.title === undefined
				? { url: uri }
				: { url: uri, title: options.title },
		);
		return;
	}
	if (Native?.shareFile === undefined) {
		throw new Error("Sharing files is unavailable in this app build.");
	}
	try {
		await Native.shareFile(uri, options.title ?? null);
	} catch {
		throw new Error("This file could not be shared.");
	}
};

type DeviceModelPlatform = {
	readonly OS: string;
	readonly constants?: {
		readonly Manufacturer?: string;
		readonly Model?: string;
	};
};

const MAX_DEVICE_MODEL_LENGTH = 80;

const capitalized = (value: string): string =>
	`${value.charAt(0).toUpperCase()}${value.slice(1)}`;

/**
 * Human-readable hardware model sent when pairing (for example "Google Pixel
 * 8"), so the Mac lists the real device rather than a hard-coded "iPhone".
 */
const currentPlatform = (): DeviceModelPlatform =>
	Platform.OS === "android"
		? { OS: Platform.OS, constants: Platform.constants }
		: { OS: Platform.OS };

export const deviceModelName = (
	platform: DeviceModelPlatform = currentPlatform(),
): string => {
	if (platform.OS === "ios") return "iPhone";
	if (platform.OS !== "android") return "Mobile device";
	const manufacturer = platform.constants?.Manufacturer?.trim() ?? "";
	const model = platform.constants?.Model?.trim() ?? "";
	const name =
		manufacturer.length === 0 ||
		model.toLowerCase().startsWith(manufacturer.toLowerCase())
			? model
			: model.length === 0
				? capitalized(manufacturer)
				: `${capitalized(manufacturer)} ${model}`;
	return (name.length === 0 ? "Android phone" : name).slice(
		0,
		MAX_DEVICE_MODEL_LENGTH,
	);
};
