import { omit } from "solid-js";
// Desktop icons, inlined so they inherit currentColor. Desktop's asset source (crates/ui/src/icons.rs
// `Assets::load`) serves the shared Hugeicons selection in crates/icons first and crates/ui/assets/icons
// only for names that set lacks, so the same precedence applies here.
// Desktop icons, inlined so they inherit currentColor. Desktop's asset source (crates/ui/src/icons.rs
// `Assets::load`) serves the shared Hugeicons selection in crates/icons first and crates/ui/assets/icons
// only for names that set lacks, so the same precedence applies here.
import { type JSX } from "@solidjs/web";

// Vite reads glob options statically, so each call spells them out.
const local = import.meta.glob("../../../../crates/ui/assets/icons/*.svg", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const shared = import.meta.glob("../../../../crates/icons/assets/free/*.svg", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

/** gpui rasterizes an icon at twice its device size and samples it down, so the 16-cell dither glyphs
 *  read as smooth tones there; crisp edges at 14px drop whole cells instead. The SVG's own size gives
 *  way to the box the Icon sets. */
const prepare = (svg: string) =>
  svg.replace(/<svg[^>]*>/, (tag) => tag.replace(/\s(?:width|height|shape-rendering)="[^"]*"/g, "").replace("<svg", '<svg width="100%" height="100%"'));

const icons = new Map(
  [...Object.entries(local), ...Object.entries(shared)].map(([path, svg]) => [path.slice(path.lastIndexOf("/") + 1, -4), prepare(svg)]),
);

export type IconName = string;

export function hasIcon(name: string) {
  return icons.has(name);
}

export function Icon(props: { name: IconName; size?: number; label?: string } & JSX.HTMLAttributes<HTMLSpanElement>) {
  const rest = omit(props, "name", "size", "label", "style");
  const size = () => `${props.size ?? 16}px`;
  return (
    <span
      {...rest}
      role={props.label ? "img" : undefined}
      aria-label={props.label}
      aria-hidden={(props.label ? undefined : true) == null ? undefined : (props.label ? undefined : true) ? "true" : "false"}
      style={{ display: "inline-flex", width: size(), height: size(), "flex-shrink": 0, ...(props.style as JSX.CSSProperties) }}
      // eslint-disable-next-line solid/no-innerhtml -- trusted SVGs bundled from the repository
      innerHTML={icons.get(props.name) ?? icons.get("info-circle") ?? ""}
    />
  );
}
