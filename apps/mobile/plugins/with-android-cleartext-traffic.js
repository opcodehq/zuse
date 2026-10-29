const { AndroidConfig, withAndroidManifest } = require("@expo/config-plugins");

/**
 * Paired computers are reached over plain `http://` and `ws://` on the local
 * network, mirroring iOS `NSAllowsLocalNetworking`. Android blocks cleartext
 * traffic in release builds unless the application opts in.
 */
module.exports = function withAndroidCleartextTraffic(config) {
	return withAndroidManifest(config, (mod) => {
		const application = AndroidConfig.Manifest.getMainApplicationOrThrow(
			mod.modResults,
		);
		application.$["android:usesCleartextTraffic"] = "true";
		return mod;
	});
};
