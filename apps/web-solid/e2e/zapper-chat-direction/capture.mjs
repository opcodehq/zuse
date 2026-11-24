import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { openBrowser } from "../harness.mjs";
const app = fileURLToPath(new URL("../..", import.meta.url));
const out = process.argv[2];
mkdirSync(out, { recursive: true });
const server = await createServer({ root: app, configFile: app + "/vite.config.ts", server: { host: "127.0.0.1", port: 0, open: false, hmr: { overlay: false } } });
await server.listen();
const base = "http://127.0.0.1:" + server.httpServer.address().port + "/e2e/zapper-chat-direction/fixture.html";
try {
  for (const [name, query, hash] of [["current", "?theme=dark", "#current"], ["proposed", "?theme=dark", "#proposed"], ["proposed-light", "?theme=light", "#proposed"]]) {
    const page = await openBrowser("about:blank", { width: 1440, height: 900 }); await page.call("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: query.includes("light") ? "light" : "dark" }] }); await page.goto(base + query + hash);
    try { await page.waitFor(hash === "#current" ? "!!document.querySelector('.t-entry')" : "!!document.querySelector('.zc-column')", 20000); } catch (e) { console.log(name, "timeout", JSON.stringify(page.errors.slice(0,5))); console.log(await page.eval("document.body.innerText.slice(0,600)")); }
    await new Promise(r => setTimeout(r, 1200));
    console.log(name, "errors:", JSON.stringify(page.errors.filter(e => !/favicon/.test(e)).slice(0, 3)));
    await page.screenshot(out + "/" + name + ".png");
    await page.close();
  }
} finally { await server.close(); }
