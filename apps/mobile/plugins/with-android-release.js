const {
	AndroidConfig,
	withAndroidManifest,
	withAppBuildGradle,
} = require("@expo/config-plugins");

/**
 * Store-ready Android release settings that must not affect iOS.
 *
 * - Zuse records voice input but never plays audio in the background, so the
 *   expo-audio playback service and its foreground-service permissions are
 *   dropped (Play requires a declaration and demo video for them).
 * - Release builds are signed with an upload key kept outside the repo. The
 *   keystore is read from Gradle properties (e.g. ~/.gradle/gradle.properties):
 *   ZUSE_UPLOAD_STORE_FILE, ZUSE_UPLOAD_STORE_PASSWORD, ZUSE_UPLOAD_KEY_ALIAS,
 *   ZUSE_UPLOAD_KEY_PASSWORD. Without them, release falls back to the debug key.
 */
const PLAYBACK_SERVICE = "expo.modules.audio.service.AudioControlsService";
const REMOVED_PERMISSIONS = [
	"android.permission.FOREGROUND_SERVICE",
	"android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK",
	"android.permission.SYSTEM_ALERT_WINDOW",
];

const RELEASE_SIGNING = `
        release {
            if (project.hasProperty('ZUSE_UPLOAD_STORE_FILE')) {
                storeFile file(ZUSE_UPLOAD_STORE_FILE)
                storePassword ZUSE_UPLOAD_STORE_PASSWORD
                keyAlias ZUSE_UPLOAD_KEY_ALIAS
                keyPassword ZUSE_UPLOAD_KEY_PASSWORD
            }
        }`;

const withoutPlaybackService = (config) =>
	withAndroidManifest(config, (mod) => {
		const application = AndroidConfig.Manifest.getMainApplicationOrThrow(
			mod.modResults,
		);
		application.service = (application.service ?? []).filter(
			(service) => service.$["android:name"] !== PLAYBACK_SERVICE,
		);
		return mod;
	});

const withReleaseSigning = (config) =>
	withAppBuildGradle(config, (mod) => {
		let gradle = mod.modResults.contents;
		if (!gradle.includes("ZUSE_UPLOAD_STORE_FILE")) {
			gradle = gradle.replace(
				/signingConfigs\s*\{/,
				(match) => `${match}${RELEASE_SIGNING}`,
			);
			gradle = gradle.replace(
				/(release\s*\{[^{}]*?)signingConfig signingConfigs\.debug/,
				"$1signingConfig project.hasProperty('ZUSE_UPLOAD_STORE_FILE') ? signingConfigs.release : signingConfigs.debug",
			);
		}
		mod.modResults.contents = gradle;
		return mod;
	});

module.exports = function withAndroidRelease(config) {
	config = AndroidConfig.Permissions.withBlockedPermissions(
		config,
		REMOVED_PERMISSIONS,
	);
	config = withoutPlaybackService(config);
	return withReleaseSigning(config);
};
