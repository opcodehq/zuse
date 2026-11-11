import solid from "eslint-plugin-solid/configs/typescript";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules", "src/features/connection/link/pkg"] },
  ...tseslint.configs.recommended,
  { files: ["src/**/*.{ts,tsx}"], ...solid, languageOptions: { parser: tseslint.parser } },
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      // These helpers run their callbacks later or inside their own tracked scope.
      "solid/reactivity": ["warn", { customReactiveFunctions: ["keyedWatch", "frameThrottled", "onConnected", "later", "queueMicrotask", "subscribe", "liveSubscribe"] }],
    },
  },
);
