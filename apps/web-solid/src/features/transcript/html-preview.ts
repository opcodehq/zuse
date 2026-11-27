// Preview documents never share the app origin. Keep this policy separate from markdown sanitization.
import createDOMPurify from "dompurify";
import { paintDitherAreas, paintDitherColumns } from "@/ui/dither-chart";

export const HTML_PREVIEW_LIMIT = 256_000;
export const HTML_PREVIEW_SANDBOX = "allow-scripts";
export const HTML_PREVIEW_CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'";
export const isHtmlPreviewLanguage = (language?: string) => /^(html|html-preview)$/i.test(language?.trim().split(/\s+/)[0] ?? "");

/** Marked also emits code tokens for unfinished fences. Never run an unfinished document. */
export function hasClosedFence(raw: string): boolean {
  const lines = raw.trimEnd().split("\n");
  const open = /^ {0,3}(`{3,}|~{3,})/.exec(lines[0] ?? "")?.[1];
  return !!open && lines.length > 1 && new RegExp(`^ {0,3}${open[0]}{${open.length},}\\s*$`).test(lines.at(-1)!);
}

// A private instance: the shared markdown purifier deliberately disables form controls.
let purifier: ReturnType<typeof createDOMPurify> | undefined;
function previewPurifier() {
  if (purifier) return purifier;
  purifier = createDOMPurify(window);
  purifier.addHook("afterSanitizeAttributes", node => {
    for (const attribute of [...node.attributes]) {
      const name = attribute.name.toLowerCase();
      // Only embedded raster/vector images and local anchors survive. Scripts are inline only.
      if (["src", "href", "xlink:href", "action", "formaction", "poster", "background"].includes(name)) {
        const image = node.tagName === "IMG" && name === "src" && /^data:image\//i.test(attribute.value);
        const anchor = (name === "href" || name === "xlink:href") && attribute.value.startsWith("#");
        if (!image && !anchor) node.removeAttribute(attribute.name);
      }
      if (["srcset", "target", "ping", "download", "nonce", "integrity"].includes(name)) node.removeAttribute(attribute.name);
    }
  });
  return purifier;
}

export type PreviewTheme = "dark" | "light";

/** Fragment or full document → one sandboxed document, with trusted policy before all author content. */
export function previewDocument(source: string, channel: string, theme: PreviewTheme): string {
  if (source.length > HTML_PREVIEW_LIMIT) throw new Error("HTML preview exceeds 256,000 characters");
  const sanitizer = previewPurifier();
  // DOMPurify removes literal-text elements containing markup probes, including valid JS
  // strings used to draw SVG. Sanitize the element/attributes, then restore only its text.
  // These scripts are intentionally executable solely in the opaque-origin preview.
  const scripts = new Map<string, string>();
  sanitizer.addHook("beforeSanitizeElements", node => {
    if (!(node instanceof Element) || node.nodeName !== "SCRIPT" || node.namespaceURI !== "http://www.w3.org/1999/xhtml") return;
    const marker = `/* preview-inline-script-${scripts.size} */`;
    scripts.set(marker, node.hasAttribute("src") ? "" : node.textContent ?? "");
    node.textContent = marker;
  });
  let clean: string;
  try {
    clean = sanitizer.sanitize(source, {
      WHOLE_DOCUMENT: true,
      ADD_TAGS: ["script", "style"],
      // Event handlers have the same intentional execution privilege as inline scripts, only in this sandbox.
      ADD_ATTR: name => /^on[a-z]+$/.test(name),
      FORBID_TAGS: ["base", "meta", "link", "iframe", "frame", "frameset", "object", "embed", "portal"],
      FORBID_ATTR: ["srcdoc", "autofocus", "is"],
    });
  } finally {
    sanitizer.removeHook("beforeSanitizeElements");
  }
  const doc = new DOMParser().parseFromString(clean, "text/html");
  for (const script of doc.querySelectorAll("script")) {
    const original = scripts.get(script.textContent ?? "");
    if (original !== undefined) script.textContent = original;
  }
  const policy = doc.createElement("meta");
  policy.httpEquiv = "Content-Security-Policy";
  policy.content = HTML_PREVIEW_CSP;
  const charset = doc.createElement("meta");
  charset.setAttribute("charset", "utf-8");
  const viewport = doc.createElement("meta");
  viewport.name = "viewport";
  viewport.content = "width=device-width, initial-scale=1";
  const style = doc.createElement("style");
  style.textContent = `:root { color-scheme: ${theme}; --bg: ${theme === "dark" ? "#181818" : "#ffffff"}; --text: ${theme === "dark" ? "#e8e8e8" : "#2a2a2a"}; --muted: ${theme === "dark" ? "#a3a3a3" : "#737373"}; --accent: #89ca21; --panel: ${theme === "dark" ? "#222222" : "#f4f4f4"}; --border: ${theme === "dark" ? "#323232" : "#dcdcdc"}; }
* { box-sizing: border-box; } html { background: var(--bg); color: var(--text); } body { margin: 0; padding: 24px; font: 15px/1.65 system-ui, sans-serif; overflow-wrap: anywhere; } h1,h2,h3 { line-height: 1.25; letter-spacing: -.025em; } a { color: var(--accent); } button,input,select,textarea { font: inherit; accent-color: var(--accent); } button { color: var(--text); background: var(--panel); border: 1px solid var(--border); border-radius: 7px; padding: 7px 12px; cursor: pointer; } :focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; } pre { overflow: auto; padding: 16px; background: var(--panel); border-radius: 8px; } img,svg,canvas { max-width: 100%; } table { border-collapse: collapse; } td,th { padding: 8px 12px; border-bottom: 1px solid var(--border); } @media (max-width: 480px) { body { padding: 16px; } }`;
  // This is a literal generated identifier, never source HTML or author-controlled JavaScript.
  const safeChannel = JSON.stringify(channel).replace(/</g, "\\u003c");
  const bootstrap = doc.createElement("script");
  bootstrap.textContent = `(() => {
    const channel = ${safeChannel};
    window.zuseCharts = Object.freeze({
      paintAreas: ${paintDitherAreas.toString()},
      paintColumns: ${paintDitherColumns.toString()}
    });
    const send = data => parent.postMessage({ channel, ...data }, '*');
    let pending = false;
    const report = () => { if (pending) return; pending = true; requestAnimationFrame(() => { pending = false; send({ type: 'size', height: document.body.scrollHeight }); }); };
    addEventListener('message', event => {
      if (event.source !== parent || event.data?.channel !== channel || event.data.type !== 'theme') return;
      const { theme, colors } = event.data;
      if (theme !== 'dark' && theme !== 'light') return;
      document.documentElement.style.colorScheme = theme;
      document.documentElement.dataset.theme = theme;
      for (const key of ['bg', 'text', 'muted', 'accent', 'panel', 'border']) {
        if (typeof colors?.[key] === 'string' && CSS.supports('color', colors[key])) document.documentElement.style.setProperty('--' + key, colors[key]);
      }
      dispatchEvent(new Event('zuse:theme'));
    });
    addEventListener('error', () => send({ type: 'error' }));
    addEventListener('unhandledrejection', () => send({ type: 'error' }));
    addEventListener('keydown', event => { if (event.key === 'Escape') send({ type: 'escape' }); });
    addEventListener('DOMContentLoaded', () => { new ResizeObserver(report).observe(document.body); report(); send({ type: 'ready' }); });
  })();`;
  doc.head.prepend(policy, charset, viewport, style, bootstrap);
  return "<!doctype html>\n" + doc.documentElement.outerHTML;
}
