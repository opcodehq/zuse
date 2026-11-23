import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import solid from "@solidjs/vite-plugin";

const repo = fileURLToPath(new URL("../..", import.meta.url));

export default defineConfig({
  define: { __APP_VERSION__: JSON.stringify(readFileSync(new URL("../../Cargo.toml", import.meta.url), "utf8").match(/^version = "([^"]+)"/m)?.[1] ?? "unknown") },
  base: process.env.ZUSE_WEB_BASE ?? "/",
  plugins: [solid({ dev: process.env.ZUSE_SOLID_DEV === "1" ? true : undefined })],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)), "@repo": repo } },
  // Desktop fonts and icons are read straight from crates/ui/assets.
  server: { fs: { allow: [repo] } },
  // The code highlighter's worker loads grammars on demand, which needs module workers.
  worker: { format: "es" },
  build: {
    minify: process.env.ZUSE_SOLID_DEV === "1" ? false : undefined,
    outDir: process.env.ZUSE_WEB_OUT ?? fileURLToPath(new URL("../../dist/web-solid", import.meta.url)),
    emptyOutDir: true,
    target: "es2022",
  },
});
