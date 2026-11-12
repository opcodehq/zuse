import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const repo = fileURLToPath(new URL("../..", import.meta.url));

export default defineConfig({
  plugins: [solid()],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)), "@repo": repo } },
  // Desktop fonts and icons are read straight from crates/ui/assets.
  server: { fs: { allow: [repo] } },
  // The code highlighter's worker loads grammars on demand, which needs module workers.
  worker: { format: "es" },
  build: {
    outDir: process.env.ZUSE_WEB_OUT ?? fileURLToPath(new URL("../../dist/web-solid", import.meta.url)),
    emptyOutDir: true,
    target: "es2022",
  },
});
