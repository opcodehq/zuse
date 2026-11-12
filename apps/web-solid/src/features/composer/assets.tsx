// Composer-only glyphs from crates/icons, agent brand marks, and the desktop's ordered-dither key texture.
import { type JSX, Show, splitProps } from "solid-js";
import { Icon } from "@/ui/Icon";

const extra = import.meta.glob("../../../../../crates/icons/assets/free/{access-ask,access-auto,access-full,access-read-only,chat-import,loading}.svg", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;
const extraIcons = new Map(Object.entries(extra).map(([path, svg]) => [path.slice(path.lastIndexOf("/") + 1, -4), svg]));

/** An icon from the shared set, or one of the composer's extra glyphs. */
export function Glyph(props: { name: string; size?: number; class?: string; style?: JSX.CSSProperties }) {
  const [local] = splitProps(props, ["name", "size", "class", "style"]);
  const size = () => `${local.size ?? 16}px`;
  return (
    <Show when={extraIcons.get(local.name)} fallback={<Icon name={local.name} size={local.size} class={local.class} style={local.style} />}>
      {(svg) => (
        <span
          class={local.class}
          aria-hidden="true"
          style={{ display: "inline-flex", width: size(), height: size(), "flex-shrink": 0, ...local.style }}
          // eslint-disable-next-line solid/no-innerhtml -- trusted SVGs bundled from the repository
          innerHTML={svg().replace("<svg ", '<svg width="100%" height="100%" ')}
        />
      )}
    </Show>
  );
}

const BRANDS: Record<string, string> = {
  "claude-code": "claude-mark", mock: "claude-mark", codex: "codex-mark", cursor: "cursor-mark", devin: "devin-mark",
  grok: "grok-mark", hermes: "hermes-mark", pi: "pi-mark", muse: "muse-mark", opencode: "opencode-mark",
  antigravity: "antigravity-mark", zuse: "zuse-logo", kiro: "kiro-mark", gemini: "gemini-mark", "acp-gemini": "gemini-mark",
};
export const brandIcon = (harness: string | undefined) => (harness ? BRANDS[harness] ?? "terminal" : "terminal");
/** Only Claude's mark keeps its brand colour; the rest follow the surface. */
export const brandTint = (harness: string | undefined) => (harness === "claude-code" || harness === "mock" ? "#d97757" : undefined);

export function BrandMark(props: { harness: string | undefined; size?: number }) {
  return <Icon name={brandIcon(props.harness)} size={props.size ?? 14} style={{ color: brandTint(props.harness) ?? "currentColor" }} />;
}

const BAYER = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
const cache = new Map<string, string>();

/** The desktop key's 2px ordered dither, denser toward the bottom, as a CSS background image. */
export function ditherBackground(rgb: [number, number, number], width: number, height: number, radius: number) {
  const key = `${rgb.join(",")}|${width}|${height}|${radius}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const columns = Math.max(4, Math.round(width / 2));
  const rows = Math.max(4, Math.round(height / 2));
  const cw = width / columns;
  const ch = height / rows;
  let rects = "";
  for (let y = 0; y < rows; y++) {
    for (let x = 0; x < columns; x++) {
      const cx = (x + 0.5) * cw;
      const cy = (y + 0.5) * ch;
      const dx = Math.max(radius - cx, cx - (width - radius), 0);
      const dy = Math.max(radius - cy, cy - (height - radius), 0);
      if (dx * dx + dy * dy > radius * radius) continue;
      const edge = x === 0 || y === 0 || x + 1 === columns || y + 1 === rows;
      const density = 0.25 + 0.75 * ((y + 0.5) / rows);
      const lit = density > BAYER[(y & 3) * 4 + (x & 3)]! / 16 + 0.5 / 16;
      const alpha = edge ? 0.5 : Math.min(1, (0.3 + density * 0.7) * (lit ? 1 : 0.4));
      rects += `<rect x="${(x * cw).toFixed(2)}" y="${(y * ch).toFixed(2)}" width="${cw.toFixed(2)}" height="${ch.toFixed(2)}" fill-opacity="${alpha.toFixed(2)}"/>`;
    }
  }
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" fill="rgb(${rgb.join(",")})">${rects}</svg>`;
  const url = `url("data:image/svg+xml;base64,${btoa(svg)}")`;
  cache.set(key, url);
  return url;
}

export const SEND_DITHER: [number, number, number] = [151, 183, 76];
