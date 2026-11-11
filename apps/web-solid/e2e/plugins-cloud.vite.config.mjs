// Builds the app with e2e/plugins-cloud-settings.tsx in place of the Settings area, for e2e/plugins-cloud.mjs.
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const app = fileURLToPath(new URL("..", import.meta.url));
const repo = fileURLToPath(new URL("../../..", import.meta.url));

export default defineConfig({
  root: app,
  plugins: [solid()],
  resolve: {
    alias: [
      { find: /^@\/features\/settings$/, replacement: fileURLToPath(new URL("./plugins-cloud-settings.tsx", import.meta.url)) },
      { find: /^@\//, replacement: `${app}src/` },
      { find: /^@repo\//, replacement: repo },
    ],
  },
  build: { outDir: process.env.ZUSE_WEB_OUT, emptyOutDir: true, target: "es2022" },
});
