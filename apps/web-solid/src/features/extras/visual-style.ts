// Visual style tokens mapped onto the app's theme. DOM nodes use CSS variables; canvases need resolved colors.
import { resolvedTheme } from "@/shell/theme";
import { isObject, type Json } from "./visual-session";

const TOKEN_VARS: Record<string, string> = {
  Text: "--text",
  Muted: "--text-muted",
  Faint: "--text-faint",
  Accent: "--accent",
  OnAccent: "--on-accent",
  Success: "--success",
  Warning: "--warning",
  Danger: "--danger",
  Border: "--border-strong",
};
export const CHART_COLORS = ["#89ca21", "#69b8c9", "#c69ee0", "#e2b66f", "#d78393", "#78b6a2"];

function tokenVar(token: Json | undefined): string {
  if (typeof token === "string") return `var(${TOKEN_VARS[token] ?? "--text"})`;
  if (isObject(token) && typeof token.Chart === "number") return CHART_COLORS[Math.abs(Math.floor(token.Chart)) % CHART_COLORS.length]!;
  return "var(--text)";
}

/** A `{ token, alpha?, mix? }` color as a CSS value. */
export function cssColor(spec: Json | undefined, fallback = "var(--text)"): string {
  if (!isObject(spec)) return fallback;
  let color = tokenVar(spec.token);
  if (Array.isArray(spec.mix) && spec.mix.length === 2) {
    const t = typeof spec.mix[1] === "number" ? Math.min(1, Math.max(0, spec.mix[1])) : 0;
    color = `color-mix(in srgb, ${color}, ${tokenVar(spec.mix[0])} ${Math.round(t * 100)}%)`;
  }
  const alpha = typeof spec.alpha === "number" ? Math.min(1, Math.max(0, spec.alpha)) : 1;
  return alpha < 1 ? `color-mix(in srgb, ${color} ${Math.round(alpha * 100)}%, transparent)` : color;
}

let cache: { theme: string; values: Map<string, [number, number, number]> } | null = null;

function rgbOf(cssValue: string): [number, number, number] {
  const theme = resolvedTheme();
  if (!cache || cache.theme !== theme) cache = { theme, values: new Map() };
  const hit = cache.values.get(cssValue);
  if (hit) return hit;
  let value = cssValue;
  const variable = /^var\((--[\w-]+)\)$/.exec(cssValue);
  if (variable) value = getComputedStyle(document.documentElement).getPropertyValue(variable[1]!).trim();
  const hex = /^#([0-9a-f]{6})$/i.exec(value);
  const rgb: [number, number, number] = hex ? [0, 2, 4].map((i) => parseInt(hex[1]!.slice(i, i + 2), 16)) as [number, number, number] : [232, 232, 232];
  cache.values.set(cssValue, rgb);
  return rgb;
}

/** The same color resolved for a 2D canvas. */
export function canvasColor(spec: Json | undefined): string {
  if (!isObject(spec)) return `rgb(${rgbOf("var(--text)").join(",")})`;
  let [r, g, b] = rgbOf(tokenVar(spec.token));
  if (Array.isArray(spec.mix) && spec.mix.length === 2) {
    const t = typeof spec.mix[1] === "number" ? Math.min(1, Math.max(0, spec.mix[1])) : 0;
    const [r2, g2, b2] = rgbOf(tokenVar(spec.mix[0]));
    [r, g, b] = [r + (r2 - r) * t, g + (g2 - g) * t, b + (b2 - b) * t];
  }
  const alpha = typeof spec.alpha === "number" ? Math.min(1, Math.max(0, spec.alpha)) : 1;
  return `rgba(${Math.round(r)},${Math.round(g)},${Math.round(b)},${alpha})`;
}

export function textSize(size: Json | undefined) {
  switch (size) {
    case "Xs":
      return 11;
    case "Sm":
      return 12;
    case "Lg":
      return 15;
    case "Xl":
      return 16;
    case "Xxl":
      return 19;
    default:
      return 14;
  }
}

export function fontWeight(weight: Json | undefined) {
  return weight === "Medium" ? 500 : weight === "Semibold" ? 600 : weight === "Bold" ? 700 : 400;
}

export const num = (value: Json | undefined, fallback: number) => (typeof value === "number" && Number.isFinite(value) ? value : fallback);

/** zuse_visual::NumberFormat::format: magnitude-based decimals, grouped thousands, a spaced unit unless it is a percent. */
export function formatNumber(n: number, format: Json | undefined) {
  const f = isObject(format) ? format : {};
  const magnitude = Math.abs(n);
  const auto = magnitude === 0 || magnitude >= 100 || Number.isInteger(n) ? 0 : magnitude >= 10 ? 1 : 2;
  const decimals = typeof f.decimals === "number" ? Math.min(6, Math.max(0, Math.floor(f.decimals))) : auto;
  const text = n.toFixed(decimals);
  const sign = text.startsWith("-") ? "-" : "";
  const [int, frac] = (sign ? text.slice(1) : text).split(".");
  const grouped = int!.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const prefix = typeof f.prefix === "string" ? f.prefix : "";
  const unit = typeof f.unit === "string" ? f.unit : "";
  const space = unit === "" || unit.startsWith("%") ? "" : " ";
  return `${sign}${prefix}${grouped}${frac ? `.${frac}` : ""}${space}${unit}`;
}
