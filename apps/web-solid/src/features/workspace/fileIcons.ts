// Desktop file identity icons (crates/ui/assets/file-icons) resolved like crates/ui/src/file_icons.rs:
// exact basename first, then the longest compound extension, then the generic document.
import { createSignal } from "solid-js";

const urls = import.meta.glob("../../../../../crates/ui/assets/file-icons/files/*.svg", {
  query: "?no-inline",
  import: "default",
  eager: true,
}) as Record<string, string>;

const byIcon = new Map(Object.entries(urls).map(([path, url]) => [path.slice(path.lastIndexOf("/") + 1, -4), url]));

interface Mapping {
  iconDefinitions: Record<string, { iconPath: string }>;
  fileExtensions: Record<string, string>;
  fileNames: Record<string, string>;
}

const [mapping, setMapping] = createSignal<Mapping | null>(null);
let loading = false;

function ensureMapping() {
  if (loading) return;
  loading = true;
  // The theme file is JSONC; a separate chunk keeps it off the first paint.
  void import("../../../../../crates/ui/src/file-icons.json?raw").then((module: { default: string }) => {
    const json = module.default.replace(/,(\s*[}\]])/g, "$1");
    try {
      setMapping(JSON.parse(json) as Mapping);
    } catch {
      // Generic icons remain.
    }
  });
}

function definitionUrl(map: Mapping, id: string | undefined) {
  if (!id) return undefined;
  const path = map.iconDefinitions[id]?.iconPath;
  return path ? byIcon.get(path.slice(path.lastIndexOf("/") + 1, -4)) : undefined;
}

/** URL of a file's identity icon, reactive to the mapping arriving. */
export function fileIconUrl(name: string): string | undefined {
  ensureMapping();
  const map = mapping();
  const fallback = byIcon.get("document");
  if (!map) return fallback;
  const lower = name.toLowerCase();
  const exact = definitionUrl(map, map.fileNames[lower] ?? map.fileNames[name]);
  if (exact) return exact;
  const parts = lower.split(".");
  for (let i = 1; i < parts.length; i++) {
    const url = definitionUrl(map, map.fileExtensions[parts.slice(i).join(".")]);
    if (url) return url;
  }
  return fallback;
}
