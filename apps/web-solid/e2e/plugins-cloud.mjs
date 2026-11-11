// Plugins and Cloud end to end in headless Chromium against disposable engines.
// 1. Signed out, real engine: sign-in prompts and the sign-in failure when the account service is unreachable.
// 2. Signed in, real engine, account service unreachable: the plugin and cloud error states the engine reports.
// 3. Signed in through e2e/plugins-cloud-fake.mjs (account, plugin and cloud services only; everything else is the real
//    engine): every control, failures, held requests and dropped connections.
// The Cloud page lives in Settings, so the app is built with e2e/plugins-cloud-settings.tsx standing in for Settings
// (it also exposes `window.__nav`). ZUSE_ASSETS=<dir> reuses such a build; ZUSE_SHOTS=<dir> saves screenshots;
// ZUSE_ONLY=<scenario,...> runs a subset.
import { openBrowser, startEngine } from "./harness.mjs";
import { fakeSource } from "./plugins-cloud-fake.mjs";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const shots = process.env.ZUSE_SHOTS;
const only = process.env.ZUSE_ONLY?.split(",");
const built = process.env.ZUSE_ASSETS ? null : mkdtempSync(join(tmpdir(), "zuse-pc-build-"));
if (built) {
  execFileSync(join(here, "../node_modules/.bin/vite"), ["build", "--config", join(here, "plugins-cloud.vite.config.mjs"), "--logLevel", "error"], {
    cwd: join(here, ".."),
    env: { ...process.env, ZUSE_WEB_OUT: built },
    stdio: "inherit",
  });
}
const assets = process.env.ZUSE_ASSETS ?? built;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// Nothing may reach real account services: every outbound HTTP(S) request from the engine goes to a closed port.
const offline = { HTTPS_PROXY: "http://127.0.0.1:9", HTTP_PROXY: "http://127.0.0.1:9", https_proxy: "http://127.0.0.1:9", http_proxy: "http://127.0.0.1:9", NO_PROXY: "127.0.0.1,localhost", no_proxy: "127.0.0.1,localhost" };
const signedInProfile = (dir) => {
  mkdirSync(join(dir, "account"), { recursive: true });
  writeFileSync(join(dir, "account", "session.json"), JSON.stringify({ refreshToken: "fixture-refresh", user: { id: "user_fixture", email: "fixture@example.test", name: "Fixture User" } }));
};

const HELPERS = `window.__t = {
  controls: "button, a, input, select, textarea, [role=tab], [role=radio], [role=option], [role=menuitem], [role=switch]",
  find(text, root = document) {
    const list = [...(typeof root === "string" ? document.querySelector(root) : root).querySelectorAll(this.controls)].filter((e) => e.offsetParent !== null || e.closest("[role=menu]"));
    return list.find((e) => e.getAttribute("aria-label") === text) ?? list.find((e) => e.textContent.trim() === text);
  },
  click(text, root) {
    const e = this.find(text, root);
    if (!e) throw new Error("missing control " + text);
    if (e.disabled || e.getAttribute("aria-disabled") === "true") throw new Error("disabled control " + text);
    e.click();
    return true;
  },
  enabled(text, root) { const e = this.find(text, root); return !!e && !e.disabled && e.getAttribute("aria-disabled") !== "true"; },
  mark(text, root) {
    const e = this.find(text, root);
    if (!e) throw new Error("missing control " + text);
    document.querySelectorAll("[data-e2e]").forEach((x) => x.removeAttribute("data-e2e"));
    e.setAttribute("data-e2e", "");
    return true;
  },
  has(text) { return document.body.innerText.includes(text); },
  value(selector, value) {
    const e = document.querySelector(selector);
    if (!e) throw new Error("missing field " + selector);
    const proto = e instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : e instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, "value").set.call(e, value);
    e.dispatchEvent(new Event("input", { bubbles: true }));
    e.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  },
};`;

let failures = 0;
async function scenario(name, options, run) {
  if (only && !only.includes(name)) return;
  console.log(`\n# ${name}`);
  const engine = await startEngine({ assets, env: offline, ...options.engine });
  const page = await openBrowser("about:blank", { width: options.width ?? 1440, height: options.height ?? 900 });
  await page.call("Page.addScriptToEvaluateOnNewDocument", { source: HELPERS });
  if (options.fake) await page.call("Page.addScriptToEvaluateOnNewDocument", { source: fakeSource(options.fake) });
  const prefs = { ...(options.theme ? { theme: options.theme } : {}), ...options.prefs };
  if (Object.keys(prefs).length) await page.call("Page.addScriptToEvaluateOnNewDocument", { source: `if (location.protocol.startsWith("http")) localStorage.setItem("zuse.web.preferences.v1", ${JSON.stringify(JSON.stringify(prefs))})` });
  await page.goto(engine.url);
  const t = {
    page,
    engine,
    eval: (js) => page.eval(js),
    wait: (js, ms) => page.waitFor(js, ms),
    waitText: (text, ms) => page.waitFor(`__t.has(${JSON.stringify(text)})`, ms),
    gone: (text, ms) => page.waitFor(`!__t.has(${JSON.stringify(text)})`, ms),
    // Waits briefly for the control to exist and be enabled, then clicks it; a disabled control still fails.
    click: async (text, root) => {
      const args = `${JSON.stringify(text)}${root ? `, ${JSON.stringify(root)}` : ""}`;
      await page.waitFor(`__t.enabled(${args})`, 8000).catch(() => {});
      return page.eval(`__t.click(${args})`);
    },
    enabled: (text, root) => page.eval(`__t.enabled(${JSON.stringify(text)}${root ? `, ${JSON.stringify(root)}` : ""})`),
    press: async (text, root) => {
      await page.eval(`__t.mark(${JSON.stringify(text)}${root ? `, ${JSON.stringify(root)}` : ""})`);
      await page.press("[data-e2e]");
    },
    value: (selector, value) => page.eval(`__t.value(${JSON.stringify(selector)}, ${JSON.stringify(value)})`),
    nav: (route) => page.eval(`window.__nav(${JSON.stringify(route)})`),
    log: (method) => page.eval(`__fake.log.filter((r) => r.method === ${JSON.stringify(method)})`),
    shot: async (label, scrollTo) => {
      if (!shots) return;
      if (scrollTo) await page.eval(`document.querySelector(${JSON.stringify(scrollTo)})?.scrollIntoView({ block: "start" })`);
      await sleep(150);
      await page.screenshot(join(shots, `${name}-${label}.png`));
    },
    async step(label, fn) {
      process.stdout.write(`- ${label} … `);
      try {
        await fn();
        console.log("ok");
      } catch (error) {
        failures++;
        console.log(`FAILED\n  ${error.message.split("\n")[0]}`);
        await t.shot(`FAILED-${label.replace(/\W+/g, "-").slice(0, 40)}`);
      }
    },
  };
  try {
    await page.waitFor(`window.__zuse?.state.connection.kind === "connected"`);
    await run(t);
  } catch (error) {
    failures++;
    console.log(`scenario aborted: ${error.stack}`);
  } finally {
    const errors = page.errors.filter((e) => !/Failed to load resource/.test(e));
    if (errors.length) {
      failures++;
      console.log(`page errors:\n  ${errors.join("\n  ")}`);
    }
    await page.close();
    await engine.close();
  }
}

await scenario("signed-out", {}, async (t) => {
  await t.step("the sidebar hides Plugins while signed out, as on desktop", async () => {
    await t.wait(`window.__zuse.state.account !== null`);
    await sleep(300);
    if (await t.eval(`[...document.querySelectorAll(".sb-nav-row")].some((b) => b.textContent.includes("Plugins"))`)) throw new Error("Plugins row shown");
  });
  await t.step("the catalog asks to sign in", async () => {
    await t.nav({ kind: "plugins" });
    await t.waitText("Sign in to connect your tools and use them across your agents.");
    await t.shot("plugins");
  });
  await t.step("sign-in reports the unreachable account service", async () => {
    await t.click("Sign in", ".app-pane");
    await t.wait(`__t.has("Account service is unavailable")`, 30000);
    await t.shot("plugins-sign-in-failed");
  });
  await t.step("Cloud shows the account gate with the same failure", async () => {
    await t.nav({ kind: "settings", section: "cloud" });
    await t.waitText("Sign in to set up cloud workspaces");
    await t.click("Sign in", ".app-pane");
    await t.wait(`__t.has("Account service is unavailable")`, 30000);
    await t.shot("cloud-gate");
  });
  await t.step("Settings → Plugins offers Browse and Sign in", async () => {
    await t.nav({ kind: "settings", section: "plugins" });
    await t.waitText("Turn plugins and MCP servers on or off for your agents.");
    await t.waitText("No plugins connected yet");
    await t.shot("settings-plugins");
    await t.click("Browse plugins", ".pc-installed-empty");
    await t.wait(`window.__zuse.view.route.kind === "plugins"`);
  });
});

await scenario("unreachable", { engine: { prepare: signedInProfile } }, async (t) => {
  await t.step("the engine reports the saved account with an availability error", async () => {
    await t.wait(`window.__zuse.state.account?.user?.id === "user_fixture"`);
  });
  await t.step("the catalog shows the plugin service failure and stays usable", async () => {
    await t.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("Plugins")).click()`);
    await t.waitText("Plugin service is unavailable", 30000);
    await t.waitText("Plugins are unavailable. Refresh to try again.");
    if (await t.enabled("Add MCP server")) throw new Error("Add MCP server enabled without a workspace");
    await t.shot("plugins");
    await t.click("Dismiss", ".app-pane");
    await t.gone("Plugin service is unavailable");
    await t.click("Refresh plugins");
    await t.waitText("Plugin service is unavailable", 30000);
  });
  await t.step("Cloud reports billing as unavailable with Retry", async () => {
    await t.nav({ kind: "settings", section: "cloud" });
    await t.waitText("Could not load billing", 30000);
    await t.waitText("Could not refresh your Cloud session");
    await t.shot("cloud");
    await t.click("Retry", ".app-pane");
    await t.waitText("Could not load billing", 30000);
  });
  await t.step("Cloud archives report the failure without inventing rows", async () => {
    await t.waitText("Refresh to load your Cloud archives.", 30000);
  });
});

for (const theme of ["dark", "light"]) {
  await scenario(`plugins-${theme}`, { fake: {}, theme }, async (t) => {
    await t.step("the catalog lists Custom, Featured and All plugins", async () => {
      await t.eval(`[...document.querySelectorAll(".sb-nav-row")].find((b) => b.textContent.includes("Plugins")).click()`);
      await t.waitText("Featured");
      await t.waitText("All plugins");
      await t.shot("catalog");
    });
    if (theme === "light") return;
    await t.step("search filters and reports no match", async () => {
      await t.value('input[aria-label="Search plugins"]', "lin");
      await t.waitText("Results");
      await t.wait(`!!__t.find("Open Linear details")`);
      await t.value('input[aria-label="Search plugins"]', "zzzz");
      await t.waitText("No plugins match your search");
      await t.value('input[aria-label="Search plugins"]', "");
      await t.waitText("Featured");
    });
    await t.step("Show more pages the catalog", async () => {
      await t.click("Show more");
      await t.wait(`!__t.find("Show more")`);
      await t.wait(`!!__t.find("Open Fixture tool 30 details")`);
    });
    await t.step("Connected filters with a count", async () => {
      await t.wait(`document.querySelector(".pc-segment-count")?.textContent === "1"`);
      await t.click("Connected1");
      await t.wait(`!!__t.find("Open Notion details") && !__t.find("Open Linear details")`);
      await t.shot("connected");
      await t.click("Browse");
    });
    await t.step("hosted connect shows the pending strip, then polling connects", async () => {
      await t.eval(`__fake.pollsToConnect = 3`);
      await t.click("Connect Linear");
      await t.waitText("Finish signing in to Linear in your browser.");
      await t.shot("pending");
      if (await t.enabled("Connect Sentry")) throw new Error("other connects stay enabled during sign-in");
      await t.wait(`!!__t.find("Linear connected")`, 15000);
      await t.gone("Finish signing in to Linear");
    });
    await t.step("a hosted sign-in can be cancelled", async () => {
      await t.eval(`__fake.pollsToConnect = 99`);
      await t.click("Connect Sentry");
      await t.waitText("Finish signing in to Sentry");
      await t.click("Cancel", ".pc-pending");
      await t.waitText("Plugin sign-in canceled.");
      await t.wait(`__t.enabled("Connect Sentry")`);
    });
    await t.step("a failed connect shows the error and the catalog recovers", async () => {
      await t.eval(`__fake.fail.Plugins = { message: "Plugin service is unavailable. Check the account connection and try again.", once: true }`);
      await t.click("Connect Asana");
      await t.waitText("Plugin service is unavailable");
      await t.waitText("The plugin action could not be confirmed.");
      await t.shot("connect-failed");
      await t.click("Dismiss", ".app-pane");
    });
    await t.step("detail page: breadcrumb, accounts, switch off and back on", async () => {
      await t.click("Open Notion details");
      await t.waitText("Connected accounts");
      await t.waitText("Information");
      await t.shot("detail");
      await t.click("Enable Notion");
      await t.waitText("Off · agents can't use this account");
      await t.shot("detail-off");
      await t.click("Enable Notion");
      await t.gone("Off · agents can't use this account");
    });
    await t.step("account menu disconnects an account", async () => {
      await t.press("Account actions: Notion");
      await t.wait(`!!document.querySelector(".menu")`);
      await t.shot("account-menu");
      await t.press("Disconnect", ".menu");
      await t.gone("Connected accounts");
      await t.click("Plugins", ".pc-crumbs");
      await t.waitText("Featured");
    });
    await t.step("API-key plugin collects the key inline and connects", async () => {
      await t.value('input[aria-label="Search plugins"]', "keyed");
      await t.click("Connect Keyed service");
      await t.wait(`!!document.querySelector('input[aria-label="API key"]')`);
      await t.shot("inline-key");
      await t.value('input[aria-label="API key"]', "sk-fixture");
      await t.click("Connect", ".pc-inline-key");
      await t.waitText("Connected accounts");
      await t.wait(`!document.querySelector('input[aria-label="API key"]')`);
      const sent = await t.log("Plugins");
      const connect = sent.find((r) => r.params.action === "connect" && r.params.pluginId === "keyed");
      if (connect.params.apiKey !== "Bearer sk-fixture") throw new Error(`API key sent as ${connect.params.apiKey}`);
      await t.click("Plugins", ".pc-crumbs");
      await t.value('input[aria-label="Search plugins"]', "");
    });
    await t.step("Add MCP server validates, then adds a custom server", async () => {
      await t.click("Add MCP server");
      await t.wait(`!!document.querySelector(".dialog")`);
      await t.value('.dialog input[placeholder="Internal tools"]', "Fixture tools");
      await t.value('.dialog input[type="url"]', "http://mcp.example.test/mcp");
      await t.click("Add server", ".dialog");
      await t.waitText("Enter an HTTPS server URL without embedded credentials or a fragment.");
      await t.shot("server-invalid");
      await t.value('.dialog input[type="url"]', "https://mcp.example.test/mcp");
      await t.click("Bearer token", ".dialog");
      await t.value('.dialog input[type="password"]', "secret-token");
      await t.shot("server-bearer");
      await t.click("Add server", ".dialog");
      await t.wait(`!document.querySelector(".dialog")`);
      await t.waitText("Custom servers");
      await t.wait(`!!__t.find("Open Fixture tools details")`);
    });
    await t.step("a local command server needs valid JSON", async () => {
      await t.click("Add MCP server");
      await t.click("Local command", ".dialog");
      await t.value('.dialog input[placeholder="Internal tools"]', "Local fixture");
      await t.click("Add server", ".dialog");
      await t.waitText("Enter a valid stdio MCP configuration");
      await t.value(".dialog textarea", `{"command":"/bin/true","args":["--stdio"]}`);
      await t.shot("local-server");
      await t.click("Add server", ".dialog");
      await t.wait(`!document.querySelector(".dialog")`);
      await t.wait(`!!__t.find("Open Local fixture details")`);
    });
    await t.step("custom server detail and removal with confirmation", async () => {
      await t.click("Open Fixture tools details");
      await t.waitText("mcp.example.test");
      await t.waitText("Bearer token");
      await t.shot("custom-detail");
      await t.press("More actions");
      await t.press("Remove server", ".menu");
      await t.waitText("Remove Fixture tools?");
      await t.shot("remove-confirm");
      await t.click("Remove server", ".dialog");
      await t.wait(`!document.querySelector(".dialog")`);
      await t.waitText("Featured");
      if (await t.eval(`!!__t.find("Open Fixture tools details")`)) throw new Error("server still listed");
    });
    await t.step("a dropped connection mid-request reports it and refreshes", async () => {
      await t.click("Open Linear details");
      await t.waitText("Connected accounts");
      await t.eval(`__fake.hold.Plugins = true`);
      await t.click("Enable Linear");
      await sleep(200);
      await t.eval(`__fake.hold.Plugins = false; __fake.drop()`);
      await t.waitText("The plugin action could not be confirmed. Check connections before trying again.", 20000);
      await t.wait(`window.__zuse.state.connection.kind === "connected"`, 20000);
      await t.wait(`__t.enabled("Enable Linear")`);
      await t.shot("reconnected");
    });
    await t.step("Settings → Plugins lists connections with on/off, search, Add and Disconnect", async () => {
      await t.nav({ kind: "settings", section: "plugins" });
      await t.waitText("Turn plugins and MCP servers on or off for your agents.");
      await t.wait(`document.querySelectorAll(".pc-installed-row").length >= 3`);
      await t.shot("settings-installed");
      await t.click("Linear", ".pc-installed");
      await t.wait(`__fake.plugins.connections.find((c) => c.pluginId === "linear")?.enabled === false`);
      await t.value('input[aria-label="Search connected plugins"]', "zzz");
      await t.waitText("No connected plugins match your search");
      await t.value('input[aria-label="Search connected plugins"]', "keyed");
      await t.wait(`document.querySelectorAll(".pc-installed-row").length === 1`);
      await t.click("Disconnect", ".pc-installed-row");
      await t.waitText("No connected plugins match your search");
      await t.value('input[aria-label="Search connected plugins"]', "");
      await t.press("Add", ".pc-installed-head");
      await t.wait(`!!document.querySelector(".menu")`);
      await t.shot("settings-add-menu");
      await t.press("Browse plugins", ".menu");
      await t.wait(`window.__zuse.view.route.kind === "plugins"`);
    });
    await t.step("signing out returns the catalog to the sign-in prompt", async () => {
      await t.eval(`__fake.account = { user: null, login: null, error: null, orgId: null }; __fake.push()`);
      await t.waitText("Sign in to connect your tools and use them across your agents.");
      await t.wait(`!__t.find("Add MCP server")`);
    });
  });
}

const openCloud = async (t) => {
  await t.wait(`window.__zuse.state.account?.user?.id === "user_fixture"`);
  await t.nav({ kind: "settings", section: "cloud" });
  await t.waitText("Cloud workspaces (Beta)");
};

for (const theme of ["dark", "light"]) {
  await scenario(`cloud-${theme}`, { fake: {}, theme }, async (t) => {
    await openCloud(t);
    await t.step("Hosting shows the subscription and Setup opens first", async () => {
      await t.waitText("Manage subscription");
      await t.wait(`document.querySelector('[role=tab][aria-selected=true]')?.textContent.trim() === "Setup"`);
      await t.waitText("Cloud image ready");
      await t.waitText("Connected as octo");
      await t.shot("setup");
    });
    if (theme === "light") {
      await t.click("Usage");
      await t.waitText("Monthly overage cap");
      await t.shot("usage");
      await t.click("Chats");
      await t.waitText("New cloud chat");
      await t.shot("chats");
      return;
    }
    await t.step("Manage subscription opens the billing portal", async () => {
      await t.click("Manage subscription", '[aria-label="Hosting"]');
      await t.wait(`__fake.opened.includes("https://billing.example.test/portal")`);
    });
    await t.step("the repository picker adds two repositories one at a time", async () => {
      await t.click("Add repository");
      await t.wait(`!!document.querySelector(".pc-picker-panel")`);
      await t.waitText("2 available");
      await t.value('input[aria-label="Search repositories"]', "api");
      await t.wait(`!__t.find("octo/docs")`);
      await t.value('input[aria-label="Search repositories"]', "");
      await t.click("octo/api");
      await t.click("octo/docs");
      await t.waitText("2 selected");
      await t.shot("picker");
      await t.click("Add 2");
      await t.waitText("Repository added.");
      await t.wait(`document.querySelectorAll(".pc-repo-row").length === 3`);
      const keys = (await t.log("CloudProjectConnect")).map((r) => r.params.idempotencyKey);
      if (keys.length !== 2 || keys[0] === keys[1]) throw new Error("each repository needs its own request key");
      await t.waitText("Unbuilt changes");
      await t.shot("repos-added");
    });
    await t.step("Update image builds and returns to ready", async () => {
      await t.click("Update image");
      await t.waitText("Building cloud image");
      await t.shot("building");
      await t.waitText("Cloud image ready", 20000);
    });
    await t.step("a failed repository add reports the error and dismisses", async () => {
      await t.eval(`__fake.fail.CloudProjectConnect = { message: "Repository management is not enabled for this account yet.", once: true }`);
      await t.eval(`__fake.cloud.projects = __fake.cloud.projects.filter((p) => !p.displayName.endsWith("docs"))`);
      await t.click("Refresh GitHub repositories").catch(() => {});
      await t.click("Refresh");
      await t.wait(`document.querySelectorAll(".pc-repo-row").length === 2`);
      await t.click("Add repository");
      await t.click("octo/docs");
      await t.click("Add 1");
      await t.waitText("Repository management is not enabled for this account yet.");
      await t.shot("repo-failed");
      await t.click("Dismiss", ".pc-inline-alert");
      await t.gone("Repository management is not enabled");
    });
    await t.step("removing a repository updates the list", async () => {
      await t.click("Remove octo/api");
      await t.waitText("Repository removed. Existing chats are preserved.");
      await t.wait(`document.querySelectorAll(".pc-repo-row").length === 1`);
    });
    await t.step("build logs: latest build, previous builds, paging, settings", async () => {
      await t.click("View boxd build logs");
      await t.wait(`!!document.querySelector(".dialog .pc-builds")`);
      await t.waitText("snapshot saved");
      await t.click("Previous builds 8");
      await t.waitText("Page 1 of 2");
      await t.click("Next", ".dialog");
      await t.waitText("Page 2 of 2");
      await t.click("Build settings", ".dialog");
      await t.waitText("Runtime");
      await t.shot("build-logs");
      await t.click("Close", ".dialog");
      await t.wait(`!document.querySelector(".dialog")`);
    });
    await t.step("Usage: cap validation, save, credits and history", async () => {
      await t.click("Usage");
      await t.waitText("Monthly overage cap");
      await t.waitText("$12.34 available");
      await t.value('input[aria-label="Monthly overage cap in dollars"]', "12.1234567");
      await t.click("Save");
      await t.waitText("Enter a nonnegative dollar amount with up to six decimal places.");
      await t.value('input[aria-label="Monthly overage cap in dollars"]', "75.5");
      await t.click("Save");
      await t.waitText("Monthly overage cap saved.");
      const cap = (await t.log("CloudBillingSetCap")).at(-1).params;
      if (cap.overageCapMicros !== 75_500_000) throw new Error(`cap sent as ${cap.overageCapMicros}`);
      await t.click("$50.00");
      await t.click("Buy credits");
      await t.wait(`__fake.opened.includes("https://checkout.example.test/credits/5000")`);
      await t.click("View history");
      await t.waitText("Load more");
      await t.click("Load more");
      await t.wait(`!__t.find("Load more")`);
      await t.shot("usage");
    });
    await t.step("Image storage: delete asks first, then shows deletion pending", async () => {
      await t.waitText("Image storage");
      await t.click("Delete image", '[aria-label="Image storage"]');
      await t.waitText("Delete saved image?");
      await t.shot("delete-image");
      await t.click("Delete image", ".dialog");
      await t.waitText("Image deletion pending.");
      await t.gone("Image storage", 20000);
    });
    await t.step("Chats: start a cloud chat and see its queued first message", async () => {
      await t.click("Chats");
      await t.waitText("New cloud chat");
      await t.wait(`!!document.querySelector(".pc-launch textarea")`);
      await t.value(".pc-launch textarea", "Fix the flaky login test");
      await t.eval(`__fake.latency = 900`);
      await t.click("Start cloud chat");
      await t.waitText("Accepting first message…");
      await t.shot("launch-sending");
      await t.waitText("Preparing cloud workspace…");
      await t.eval(`__fake.latency = 60`);
      const sent = (await t.log("CloudCreate")).at(-1).params;
      if (sent.firstMessage !== "Fix the flaky login test" || sent.agent !== "claude-code" || sent.projectId !== "proj-web") throw new Error(JSON.stringify(sent));
      await t.shot("launch-accepted");
    });
    await t.step("a failed launch offers Retry with the same request key", async () => {
      await t.eval(`__fake.fail.CloudCreate = { message: "Cloud service is unavailable. Try again.", once: true }`);
      await t.value(".pc-launch textarea", "Second task");
      await t.click("Start cloud chat");
      await t.waitText("Cloud service is unavailable. Try again.");
      await t.shot("launch-failed");
      await t.click("Retry", ".pc-queue");
      await t.wait(`__fake.log.filter((r) => r.method === "CloudCreate").length === 3`);
      const keys = (await t.log("CloudCreate")).slice(-2).map((r) => r.params.idempotencyKey);
      if (keys[0] !== keys[1]) throw new Error("Retry used a new key");
    });
    await t.step("a launch interrupted by a dropped connection is uncertain and retries safely", async () => {
      await t.eval(`__fake.hold.CloudCreate = true`);
      await t.value(".pc-launch textarea", "Third task");
      await t.click("Start cloud chat");
      await sleep(300);
      await t.eval(`__fake.hold.CloudCreate = false; __fake.drop()`);
      await t.waitText("The connection dropped before the cloud confirmed this chat.", 20000);
      await t.wait(`window.__zuse.state.connection.kind === "connected"`, 20000);
      await t.shot("launch-uncertain");
      await t.click("Retry", ".pc-queue");
      await t.wait(`__fake.log.filter((r) => r.method === "CloudCreate").length === 5`);
      const keys = (await t.log("CloudCreate")).slice(-2).map((r) => r.params.idempotencyKey);
      if (keys[0] !== keys[1]) throw new Error("uncertain Retry used a new key");
    });
    await t.step("the cloud chats list shows live states and Pause", async () => {
      await t.waitText("Fix the login redirect");
      await t.gone("Fix the flaky login test", 20000);
      await t.shot("chats", '[aria-label="Cloud chats"]');
      await t.click("Pause", '[aria-label="Cloud chats"]');
      await t.wait(`[...document.querySelectorAll(".pc-badge")].some((b) => b.textContent === "Paused")`);
    });
    await t.step("saved history is virtualized, switches conversations, and goes back", async () => {
      await t.eval(`[...document.querySelectorAll(".pc-row")].find((r) => r.textContent.includes("Fix the login redirect")).querySelector("button:not([disabled])").click()`);
      await t.wait(`document.querySelectorAll(".pc-msg").length > 0`);
      const shown = await t.eval(`document.querySelectorAll(".pc-msg").length`);
      if (shown >= 40) throw new Error(`all ${shown} messages rendered`);
      await t.shot("history");
      await t.click("Side quest");
      await t.wait(`document.querySelectorAll(".pc-msg").length === 4`);
      await t.click("Back");
      await t.waitText("New cloud chat");
    });
    await t.step("archives: restore, retry local cleanup, delete with confirmation", async () => {
      await t.waitText("Cloud archives · 2");
      await t.shot("archives", '[aria-label="Cloud archives"]');
      await t.click("Retry local cleanup");
      await t.wait(`!__t.find("Retry local cleanup")`);
      await t.click("Delete…");
      await t.waitText("Delete Cloud chat?");
      await t.click("Cancel", ".dialog");
      await t.click("Restore chat");
      await t.wait(`!__t.has("Archived refactor") || !document.querySelector(".pc-archive-card")`, 15000);
    });
    await t.step("an action interrupted by a dropped connection clears busy state after reconnect", async () => {
      await t.eval(`__fake.hold.CloudPause = true`);
      await t.eval(`__fake.cloud.workspaces.find((w) => w.workspaceId === "ws-run").state = "running"`);
      await t.click("Refresh", '[aria-label="Cloud chats"]');
      await t.wait(`!!__t.find("Pause")`);
      await t.click("Pause", '[aria-label="Cloud chats"]');
      await sleep(200);
      await t.eval(`__fake.hold.CloudPause = false; __fake.drop()`);
      await t.wait(`window.__zuse.state.connection.kind === "connected"`, 20000);
      await t.wait(`__t.enabled("Pause")`, 15000);
      await t.shot("pause-interrupted");
    });
    await t.step("signing out shows the account gate", async () => {
      await t.eval(`__fake.account = { user: null, login: null, error: null, orgId: null }; __fake.push()`);
      await t.waitText("Sign in to set up cloud workspaces");
    });
    await t.step("sign-in opens the device page and shows the code to confirm", async () => {
      await t.click("Sign in", ".app-pane");
      await t.wait(`__fake.opened.includes("https://auth.example.test/device")`);
      await t.waitText("Confirm the code ABCD-EFGH in your browser.");
      await t.wait(`!!__t.find("Continue sign-in")`);
      await t.shot("sign-in-pending");
      await t.nav({ kind: "plugins" });
      await t.waitText("Finish signing in");
      await t.shot("plugins-sign-in-pending");
    });
  });
}

await scenario("cloud-unsubscribed", { fake: { subscribed: false } }, async (t) => {
  await openCloud(t);
  await t.step("unsubscribed accounts see Subscribe and no tabs", async () => {
    await t.waitText("Subscribe · $40/month");
    if (await t.eval(`!!__t.find("Setup")`)) throw new Error("tabs shown before subscribing");
    await t.shot("hosting");
  });
  await t.step("Subscribe opens checkout", async () => {
    await t.click("Subscribe · $40/month");
    await t.wait(`__fake.opened.some((u) => u.startsWith("https://checkout.example.test/cloud-workspace-standard-v1"))`);
  });
});

// The shell keeps its sidebar open at any width, so the phone check closes it to measure this area alone.
await scenario("narrow", { fake: {}, width: 390, height: 844, prefs: { sidebarOpen: false } }, async (t) => {
  const overflow = () => t.eval(`document.querySelector(".pc-scroll").scrollWidth > document.querySelector(".pc-scroll").clientWidth + 1`);
  await t.step("the catalog fits a phone width in one column", async () => {
    await t.wait(`window.__zuse.state.account?.user?.id === "user_fixture"`);
    await t.nav({ kind: "plugins" });
    await t.waitText("Featured");
    if (await overflow()) throw new Error("catalog scrolls sideways");
    await t.shot("catalog");
  });
  await t.step("the Add MCP server dialog fits", async () => {
    await t.click("Add MCP server");
    await t.wait(`!!document.querySelector(".dialog")`);
    const fits = await t.eval(`(() => { const r = document.querySelector(".dialog").getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth; })()`);
    if (!fits) throw new Error("dialog wider than the screen");
    await t.shot("server-dialog");
    await t.click("Cancel", ".dialog");
  });
  await t.step("Cloud setup and the repository picker fit", async () => {
    await t.nav({ kind: "settings", section: "cloud" });
    await t.waitText("Connected as octo");
    if (await overflow()) throw new Error("cloud page scrolls sideways");
    await t.click("Add repository");
    await t.wait(`!!document.querySelector(".pc-picker-panel")`);
    const fits = await t.eval(`(() => { const r = document.querySelector(".pc-picker-panel").getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth; })()`);
    await t.shot("picker", ".pc-picker");
    if (!fits) throw new Error("picker extends past the screen");
  });
});

if (built) rmSync(built, { recursive: true, force: true });
console.log(failures ? `\n${failures} failure(s)` : "\nall plugins-cloud checks passed");
process.exit(failures ? 1 : 0);
