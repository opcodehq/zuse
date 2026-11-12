// Smoke test: the built app connects to a real engine and receives workspace data.
import { buildApp, openBrowser, startEngine } from "./harness.mjs";
import { rmSync } from "node:fs";

const assets = buildApp();
const engine = await startEngine({ assets });
const page = await openBrowser(engine.url);
try {
  await page.waitFor(`window.__zuse?.state.connection.kind === "connected" && window.__zuse.state.workspaceSynced`);
  const local = await page.eval(`window.__zuse.state.localDeviceId`);
  if (!local) throw new Error("local device id missing");
  if (page.errors.length) throw new Error(`page errors:\n${page.errors.join("\n")}`);
  console.log("smoke ok", { localDevice: local });
} finally {
  await page.close();
  await engine.close();
  rmSync(assets, { recursive: true, force: true });
}
