import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { resolve } from "node:path";
const require = createRequire(
  resolve(
    process.env.PLAYWRIGHT_NODE_MODULES || "node_modules",
    "../monitor.cjs",
  ),
);
const { chromium } = require("playwright");
const browser = await chromium.launch({
  executablePath: process.env.CHROMIUM || "/usr/bin/chromium",
});
try {
  const page = await browser.newPage({
    viewport: { width: 1920, height: 1080 },
  });
  const port = Number(process.env.MONITOR_VIEWER_PORT || 6901);
  const password = execFileSync("zuse-monitor", ["password"], {
    encoding: "utf8",
  }).trim();
  await page.goto(`http://127.0.0.1:${port}/#${password}`);
  await page.waitForFunction(() => window.zuseMonitorStats?.fps > 0);
  const result = await page.evaluate(
    () =>
      new Promise((resolve) => {
        const source = document.querySelector("canvas");
        const sampler = document.createElement("canvas");
        sampler.width = 256;
        sampler.height = 1;
        const ctx = sampler.getContext("2d", { willReadFrequently: true });
        let previous,
          distinct = 0;
        const start = performance.now();
        function sample(now) {
          ctx.drawImage(source, 0, 8, 256, 1, 0, 0, 256, 1);
          const pixels = ctx.getImageData(0, 0, 256, 1).data;
          let id = 0;
          for (let i = 0; i < 16; i++)
            if (pixels[(i * 16 + 8) * 4] > 128) id |= 1 << i;
          if (previous !== undefined && id !== previous) distinct++;
          previous = id;
          if (now - start < 10000) requestAnimationFrame(sample);
          else
            resolve({
              distinct,
              distinctFps: +((1000 * distinct) / (now - start)).toFixed(1),
            });
        }
        requestAnimationFrame(sample);
      }),
  );
  console.log(JSON.stringify(result));
} finally {
  await browser.close();
}
