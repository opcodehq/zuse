const { withGradleProperties } = require("@expo/config-plugins");

const PROPERTY = "reactNativeArchitectures";
const ARCHITECTURES = "arm64-v8a,x86_64";

module.exports = function withAndroidArchitectures(config) {
	return withGradleProperties(config, (mod) => {
		mod.modResults = mod.modResults.filter(
			(item) => item.type !== "property" || item.key !== PROPERTY,
		);
		mod.modResults.push({
			type: "property",
			key: PROPERTY,
			value: ARCHITECTURES,
		});
		return mod;
	});
};

module.exports.ARCHITECTURES = ARCHITECTURES;
