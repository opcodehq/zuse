// Translations from crates/i18n, loaded per locale on demand. English copy is inline at each call site.
import { createEffect, createRoot, createSignal } from "solid-js";
import { view } from "@/core/view";

export const LOCALES = [
  { code: "en", name: "English" },
  { code: "fr", name: "Français" },
  { code: "de", name: "Deutsch" },
  { code: "zh-Hans", name: "简体中文" },
  { code: "zh-Hant", name: "繁體中文" },
  { code: "ja", name: "日本語" },
  { code: "ko", name: "한국어" },
] as const;

export type LocaleCode = (typeof LOCALES)[number]["code"];

// Only the catalogs the settings and usage pages read; each becomes its own lazy chunk.
const files = import.meta.glob(
  [
    "../../../../../crates/i18n/locales/*/settings.ftl",
    "../../../../../crates/i18n/locales/*/native.ftl",
    "../../../../../crates/i18n/locales/*/usage.ftl",
    "../../../../../crates/i18n/locales/*/common.ftl",
    "../../../../../crates/i18n/locales/*/providers.ftl",
    "../../../../../crates/i18n/locales/*/projects.ftl",
    "../../../../../crates/i18n/locales/*/plugins.ftl",
    "!../../../../../crates/i18n/locales/en/*.ftl",
  ],
  { query: "?raw", import: "default" },
) as Record<string, () => Promise<string>>;

type Node = string | { variable: string } | { ref: string } | { selector: string; variants: { key: string; fallback: boolean; nodes: Node[] }[] };
type Bundle = Map<string, Node[]>;

/** The locale a preference resolves to, following the browser for "system". */
export function resolveLocale(preference: string): LocaleCode {
  if (preference !== "system" && LOCALES.some((l) => l.code === preference)) return preference as LocaleCode;
  for (const tag of typeof navigator === "undefined" ? [] : navigator.languages) {
    const lower = tag.toLowerCase();
    if (lower.startsWith("zh")) return /hant|-tw|-hk|-mo/.test(lower) ? "zh-Hant" : "zh-Hans";
    const code = lower.split("-")[0];
    const match = LOCALES.find((l) => l.code === code);
    if (match) return match.code;
  }
  return "en";
}

const [bundle, setBundle] = createSignal<{ locale: LocaleCode; messages: Bundle } | null>(null);
const [loadError, setLoadError] = createSignal<string | null>(null);
export { loadError as languageLoadError };
export const activeLocale = () => resolveLocale(view.preferences.language);

const cache = new Map<LocaleCode, Promise<Bundle>>();
function load(locale: LocaleCode): Promise<Bundle> {
  let pending = cache.get(locale);
  if (!pending) {
    const loaders = Object.entries(files).filter(([path]) => path.includes(`/locales/${locale}/`));
    pending = Promise.all(loaders.map(([, read]) => read())).then((texts) => {
      const messages: Bundle = new Map();
      for (const text of texts) parse(text, messages);
      return messages;
    });
    cache.set(locale, pending);
    pending.catch(() => cache.delete(locale));
  }
  return pending;
}

createRoot(() =>
  createEffect(() => {
    const locale = activeLocale();
    document.documentElement.lang = locale;
    if (locale === "en") {
      setBundle(null);
      setLoadError(null);
      return;
    }
    load(locale).then(
      (messages) => {
        if (activeLocale() !== locale) return;
        setBundle({ locale, messages });
        setLoadError(null);
      },
      () => setLoadError("Translations could not be loaded. Showing English."),
    );
  }),
);

type Args = Record<string, string | number>;

/** Message `id` from the catalogs (e.g. `settings-settings_page_general`), else the English `fallback`. */
export function t(id: string, fallback: string, args?: Args): string {
  const nodes = bundle()?.messages.get(id);
  if (!nodes) return fallback.replace(/\{\s*\$(\w+)\s*\}/g, (_, name: string) => String(args?.[name] ?? ""));
  const current = bundle()!;
  return format(nodes, args ?? {}, current.locale, current.messages, 0);
}

function format(nodes: Node[], args: Args, locale: string, messages: Bundle, depth: number): string {
  let out = "";
  for (const node of nodes) {
    if (typeof node === "string") out += node;
    else if ("variable" in node) out += String(args[node.variable] ?? "");
    else if ("ref" in node) {
      const target = messages.get(node.ref);
      // Terms and message references; a cycle or missing target renders nothing.
      if (target && depth < 8) out += format(target, args, locale, messages, depth + 1);
    } else {
      const value = args[node.selector];
      const category = typeof value === "number" ? new Intl.PluralRules(locale).select(value) : String(value ?? "");
      const variant =
        node.variants.find((v) => v.key === String(value)) ??
        node.variants.find((v) => v.key === category) ??
        node.variants.find((v) => v.fallback);
      if (variant) out += format(variant.nodes, args, locale, messages, depth);
    }
  }
  return out;
}

/** A small Fluent subset: messages, multiline values, string literals, variables and select expressions. */
function parse(text: string, into: Bundle) {
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const match = /^(-?[a-zA-Z][\w-]*)\s*=\s?(.*)$/.exec(lines[i]!);
    if (!match) continue;
    const body = [match[2]!];
    while (i + 1 < lines.length && /^\s+\S/.test(lines[i + 1]!) && !/^\s+\./.test(lines[i + 1]!)) body.push(lines[++i]!.trim());
    const value = body.filter((line, index) => index > 0 || line.length > 0).join("\n");
    try {
      into.set(match[1]!, pattern(value, { at: 0 }, false));
    } catch {
      // A construct outside the subset leaves the English copy in place.
    }
  }
}

function pattern(source: string, cursor: { at: number }, inVariant: boolean): Node[] {
  const nodes: Node[] = [];
  let text = "";
  while (cursor.at < source.length) {
    const ch = source[cursor.at]!;
    if (inVariant && ch === "\n" && /^\n\s*(\*?\[|\})/.test(source.slice(cursor.at))) break;
    if (inVariant && ch === "}") break;
    if (ch === "{") {
      if (text) nodes.push(text);
      text = "";
      cursor.at++;
      nodes.push(...placeable(source, cursor));
      continue;
    }
    text += ch;
    cursor.at++;
  }
  if (text) nodes.push(text);
  return nodes;
}

function placeable(source: string, cursor: { at: number }): Node[] {
  const skip = () => {
    while (/\s/.test(source[cursor.at] ?? "")) cursor.at++;
  };
  skip();
  let node: Node;
  if (source[cursor.at] === '"') {
    const end = source.indexOf('"', cursor.at + 1);
    if (end < 0) throw new Error("unterminated");
    node = source.slice(cursor.at + 1, end);
    cursor.at = end + 1;
  } else if (source[cursor.at] === "$") {
    const name = /^\$(\w+)/.exec(source.slice(cursor.at));
    if (!name) throw new Error("variable");
    cursor.at += name[0].length;
    node = { variable: name[1]! };
  } else {
    const name = /^-?[a-zA-Z][\w-]*/.exec(source.slice(cursor.at));
    if (!name) throw new Error("unsupported");
    cursor.at += name[0].length;
    node = { ref: name[0] };
  }
  skip();
  if (source.startsWith("->", cursor.at)) {
    if (typeof node === "string" || !("variable" in node)) throw new Error("selector");
    cursor.at += 2;
    const variants: { key: string; fallback: boolean; nodes: Node[] }[] = [];
    for (;;) {
      skip();
      if (source[cursor.at] === "}") break;
      const head = /^(\*?)\[([^\]]+)\]\s?/.exec(source.slice(cursor.at));
      if (!head) throw new Error("variant");
      cursor.at += head[0].length;
      variants.push({ key: head[2]!.trim(), fallback: head[1] === "*", nodes: pattern(source, cursor, true) });
    }
    node = { selector: node.variable, variants };
  }
  skip();
  if (source[cursor.at] !== "}") throw new Error("close");
  cursor.at++;
  return [node];
}
