# Inline HTML previews

The Solid web chat renders complete, top-level fenced `html` and `html-preview` blocks in assistant text as interactive preview cards. No protocol change is required: the original markdown remains the stored and copied content. Other clients continue to show code until their own renderer supports previews.

The card provides Preview/Source, Copy HTML, Restart, and Expand/Collapse controls. Expansion uses the same iframe in a native modal dialog, preserving JavaScript state. Escape closes the expanded view, including from inside the frame. Light/dark theme changes are sent to the frame without reloading it. Inline height is bounded between 200 and 640 pixels, with scrolling for longer documents.

Use a closed fence, including for fragments:

````markdown
```html
<h2>Explore the model</h2>
<p>The count is <output id="count">0</output>.</p>
<button id="advance">Advance</button>
<script>
  let count = 0;
  document.querySelector('#advance').addEventListener('click', () => {
    document.querySelector('#count').textContent = String(++count);
  });
</script>
```
````

Built-in styles supply readable typography, controls, tables and code blocks. Authors can use `--bg`, `--text`, `--muted`, `--accent`, `--panel`, and `--border` for app-matched colors. Author CSS may override these defaults. Use inline stylesheets and scripts. Both `addEventListener` and inline HTML event handlers work inside the isolated frame. External libraries are not loaded.

## Isolation

`html-preview.ts` builds a separate document using a private DOMPurify instance. It removes nested browsing contexts, author CSP/refresh metadata, base tags, and external resource attributes. Inline script elements and event handlers are deliberately retained for interactive content, exclusively inside the opaque-origin frame. The trusted CSP precedes author content. It blocks fetch/XHR/WebSocket, external scripts, nested frames, form submissions, and ordinary external image/font/style loads. Inline scripts, inline styles, and embedded images work.

The iframe has only `sandbox="allow-scripts"`: it receives an opaque origin and cannot access the app DOM, cookies, or storage. It cannot navigate the top page, open popups, download files, or use the app's bridge. Message handling checks both the frame window and a per-card identifier, accepts only bounded size information, a generic error notice, ready, and Escape. No message invokes an engine action or inserts HTML into the host.

This is browser isolation, not a CPU or memory quota. Author scripts can hang their frame and may navigate their own frame; CSP is not a universal network or self-navigation firewall. Do not treat it as an offline execution environment. The parent continues to deny access even after a frame navigation.

Only assistant text opts into previews. User messages, reasoning, issue descriptions, provider markdown, and file previews keep their existing source rendering. Raw HTML outside code fences still appears as text. Unfinished fences never execute. Documents above 256,000 characters show source access instead of mounting a frame.

## Validation

Run `bun run typecheck`, targeted ESLint, and `node e2e/html-preview.mjs` from `apps/web-solid`. Set `ZUSE_CHROME` to a Chromium executable if it differs from the harness default. The browser fixture covers streamed completion, script execution, parent/storage denial, CSP, sanitization, message-source validation, source switching, modal state preservation, theme changes, restart, responsive layout, and ordinary markdown behavior. Set `HTML_PREVIEW_PRODUCTION=1` to exercise the minified production fixture. Run `node e2e/html-preview/dither.mjs` for the interactive Settings-style chart example. Screenshots are written to `/tmp/zuse-html-preview`; reviewed captures live in `internal-docs/images/inline-html-previews`.

## Dither chart style

Use the Settings usage-chart raster, rather than uniform checkerboard fills or decorative pixel grids. `src/ui/dither-chart.ts` now owns the area raster used by both Settings and inline previews: logical 2px cells, a 4×4 Bayer threshold, density varying through the fill, two distinct edge rows, and alternating dotted series. The preview exposes the self-contained functions as `window.zuseCharts.paintAreas` and `window.zuseCharts.paintColumns`; neither has host access. Columns follow the viewport column algorithm: a dense floor, a dissolving body and a solid cap.

```js
const canvas = document.querySelector('canvas');
const ctx = canvas.getContext('2d');
// Set backing resolution and DPR transform before painting; dimensions are CSS pixels.
window.zuseCharts.paintAreas(ctx, width, height, maximum,
  [[20, 8], [55, 17], [40, 12]],
  [[53, 143, 243], [40, 210, 110]]);
```

Labels, gridlines, legends and inspection controls remain clean. Canvas examples should repaint on resize and the `zuse:theme` event. `sandbox-guide/dither-charts.html` demonstrates the actual Settings raster in an inline card, including per-day inspection, an area/column switch, responsive axes, and a generated standalone fallback.
