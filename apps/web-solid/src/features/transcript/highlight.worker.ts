// Shiki runs here, off the main thread: grammar loading, regex compilation and tokenizing never block the UI.
// Replies are compact: one Int32Array of [offset, length, colour index, font style] per token, transferred.
import type { HighlighterCore } from "shiki/core";

let core: Promise<HighlighterCore> | null = null;
const loaded = new Map<string, Promise<string | null>>();

function highlighter() {
  core ??= (async () => {
    const [{ createHighlighterCore, createCssVariablesTheme }, { createJavaScriptRegexEngine }] = await Promise.all([
      import("shiki/core"),
      import("shiki/engine/javascript"),
    ]);
    return createHighlighterCore({
      themes: [createCssVariablesTheme({ name: "zuse", variablePrefix: "--shiki-", fontStyle: true })],
      langs: [],
      engine: createJavaScriptRegexEngine({ forgiving: true }),
    });
  })();
  return core;
}

/** Resolves a fence's info string to a loaded grammar id, or null when Shiki has none. */
function language(lang: string): Promise<string | null> {
  const id = lang.trim().toLowerCase().split(/\s+/)[0] ?? "";
  let pending = loaded.get(id);
  if (!pending) {
    pending = (async () => {
      if (!id || id === "text" || id === "plaintext" || id === "txt") return null;
      const [{ bundledLanguages }, hl] = await Promise.all([import("shiki/langs"), highlighter()]);
      const load = (bundledLanguages as Record<string, (() => Promise<{ default: unknown }>) | undefined>)[id];
      if (!load) return null;
      const grammar = (await load()).default as { name: string }[];
      await hl.loadLanguage(grammar as unknown as Parameters<HighlighterCore["loadLanguage"]>[0]);
      // Grammar modules list embedded languages first and the requested one last.
      return grammar.at(-1)?.name ?? null;
    })().catch(() => null);
    loaded.set(id, pending);
  }
  return pending;
}

export interface TokenizeRequest {
  id: number;
  code: string;
  lang: string;
}
export interface TokenizeReply {
  id: number;
  result: { colors: string[]; data: Int32Array } | null;
}

const reply = (message: TokenizeReply, transfer: Transferable[] = []) => self.postMessage(message, { transfer });

self.onmessage = async (event: MessageEvent<TokenizeRequest>) => {
  const { id, code, lang } = event.data;
  try {
    const grammar = await language(lang);
    if (!grammar) return reply({ id, result: null });
    const lines = (await highlighter()).codeToTokensBase(code, { lang: grammar, theme: "zuse" });
    const colors: string[] = [];
    const index = new Map<string, number>();
    let count = 0;
    for (const line of lines) count += line.length;
    const data = new Int32Array(count * 4);
    let at = 0;
    for (const line of lines) {
      for (const token of line) {
        let colour = -1;
        if (token.color) {
          colour = index.get(token.color) ?? colors.push(token.color) - 1;
          index.set(token.color, colour);
        }
        data[at++] = token.offset;
        data[at++] = token.content.length;
        data[at++] = colour;
        data[at++] = token.fontStyle ?? 0;
      }
    }
    reply({ id, result: { colors, data } }, [data.buffer]);
  } catch {
    reply({ id, result: null });
  }
};
