// Desktop's harness_brand_icon (crates/ui pickers.rs), shared by every area that shows an agent mark.
import { hasIcon } from "./Icon";

const MARKS: Record<string, string> = {
  zuse: "zuse-logo",
  "claude-code": "claude-mark",
  mock: "claude-mark",
  codex: "codex-mark",
  cursor: "cursor-mark",
  devin: "devin-mark",
  gemini: "gemini-mark",
  "acp-gemini": "gemini-mark",
  grok: "grok-mark",
  hermes: "hermes-mark",
  kiro: "kiro-mark",
  muse: "muse-mark",
  opencode: "opencode-mark",
  pi: "pi-mark",
  antigravity: "antigravity-mark",
};

/** The bundled mark for a harness id; unknown ids try the `<id>-mark` convention, then `fallback`. */
export function harnessIcon(harness: string | null | undefined, fallback = "terminal") {
  if (!harness) return fallback;
  const known = MARKS[harness];
  if (known) return known;
  const id = harness.replace(/^acp-/, "");
  for (const name of [`${id}-mark`, `${id.split("-")[0]}-mark`]) if (hasIcon(name)) return name;
  return fallback;
}

/** Claude keeps its brand tint; every other mark takes the surface colour. */
export const harnessTint = (harness: string) => (harness === "claude-code" || harness === "mock" ? "#d97757" : undefined);
