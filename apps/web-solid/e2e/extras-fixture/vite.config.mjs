// Builds the test-only extras fixture page into a private directory.
import { fileURLToPath } from "node:url";
import { defineConfig, mergeConfig } from "vite";
import base from "../../vite.config.ts";

export default mergeConfig(base, defineConfig({
  root: fileURLToPath(new URL(".", import.meta.url)),
  build: { outDir: process.env.ZUSE_FIXTURE_OUT, emptyOutDir: true, rollupOptions: { input: fileURLToPath(new URL("./fixture.html", import.meta.url)) } },
}));
