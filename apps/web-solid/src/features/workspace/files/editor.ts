// CodeMirror setup themed from tokens, so light and dark follow the app without a second theme.
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, foldGutter, foldKeymap, HighlightStyle, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import {
  crosshairCursor,
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

const highlight = HighlightStyle.define([
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: "var(--syntax-comment)", fontStyle: "italic" },
  { tag: [t.keyword, t.controlKeyword, t.moduleKeyword, t.operatorKeyword, t.definitionKeyword, t.modifier], color: "var(--syntax-keyword)" },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: "var(--syntax-string)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--syntax-number)" },
  { tag: [t.typeName, t.className, t.namespace, t.standard(t.typeName)], color: "var(--syntax-type)" },
  { tag: [t.function(t.variableName), t.function(t.propertyName), t.macroName], color: "var(--syntax-function)" },
  { tag: [t.tagName, t.heading], color: "var(--syntax-tag)", fontWeight: "500" },
  { tag: [t.attributeName, t.propertyName], color: "var(--syntax-attribute)" },
  { tag: t.link, textDecoration: "underline" },
  { tag: t.strong, fontWeight: "600" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.invalid, color: "var(--danger)" },
]);

const theme = EditorView.theme({
  "&": { height: "100%", backgroundColor: "var(--bg)", color: "var(--text)", fontSize: "12px" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.6" },
  ".cm-content": { caretColor: "var(--text-strong)", padding: "6px 0" },
  ".cm-gutters": { backgroundColor: "var(--bg)", color: "var(--text-faint)", border: "none" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--text-muted)" },
  ".cm-activeLine": { backgroundColor: "color-mix(in srgb, var(--text) 4%, transparent)" },
  "&.cm-focused": { outline: "none" },
  "&.cm-focused .cm-cursor": { borderLeftColor: "var(--text-strong)" },
  ".cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection": {
    backgroundColor: "color-mix(in srgb, var(--accent) 30%, transparent) !important",
  },
  ".cm-matchingBracket": { backgroundColor: "color-mix(in srgb, var(--accent) 22%, transparent)", outline: "none" },
  ".cm-searchMatch": { backgroundColor: "color-mix(in srgb, var(--warning) 30%, transparent)" },
  ".cm-panels": { backgroundColor: "var(--shell)", color: "var(--text)", borderColor: "var(--border)" },
  ".cm-panel input, .cm-panel button": { fontFamily: "var(--font-sans)", fontSize: "12px" },
  ".cm-foldGutter span": { color: "var(--text-faint)" },
});

async function languageFor(path: string): Promise<Extension> {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  switch (ext) {
    case "ts":
    case "mts":
    case "cts":
      return (await import("@codemirror/lang-javascript")).javascript({ typescript: true });
    case "tsx":
      return (await import("@codemirror/lang-javascript")).javascript({ typescript: true, jsx: true });
    case "js":
    case "mjs":
    case "cjs":
    case "jsx":
      return (await import("@codemirror/lang-javascript")).javascript({ jsx: ext === "jsx" });
    case "json":
      return (await import("@codemirror/lang-javascript")).javascript();
    case "rs":
      return (await import("@codemirror/lang-rust")).rust();
    case "css":
    case "scss":
      return (await import("@codemirror/lang-css")).css();
    case "html":
    case "htm":
    case "svg":
      return (await import("@codemirror/lang-html")).html();
    case "md":
    case "markdown":
    case "mdx":
      return (await import("@codemirror/lang-markdown")).markdown();
    default:
      return [];
  }
}

export interface EditorHandle {
  view: EditorView;
  setText(text: string): void;
  text(): string;
  setReadOnly(readOnly: boolean): void;
  setLanguage(path: string): void;
  destroy(): void;
}

// Read-only keeps the caret and selection for copying; assistive tech hears it through aria-readonly.
const readOnlyExtension = (value: boolean) => [EditorState.readOnly.of(value), EditorView.contentAttributes.of({ "aria-readonly": String(value) })];

export function createEditor(parent: HTMLElement, options: { onChange: () => void; onSave: () => void; label: string }): EditorHandle {
  const language = new Compartment();
  const readOnly = new Compartment();
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: "",
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        foldGutter(),
        drawSelection(),
        EditorState.allowMultipleSelections.of(true),
        indentOnInput(),
        bracketMatching(),
        rectangularSelection(),
        crosshairCursor(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        search({ top: true }),
        syntaxHighlighting(highlight),
        theme,
        language.of([]),
        readOnly.of(readOnlyExtension(false)),
        EditorView.contentAttributes.of({ "aria-label": options.label }),
        keymap.of([
          { key: "Mod-s", preventDefault: true, run: () => (options.onSave(), true) },
          indentWithTab,
          ...defaultKeymap,
          ...historyKeymap,
          ...foldKeymap,
          ...searchKeymap,
        ]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) options.onChange();
        }),
      ],
    }),
  });
  let languageSeq = 0;
  return {
    view,
    setText(text) {
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text }, selection: { anchor: 0 }, scrollIntoView: true });
    },
    text: () => view.state.doc.toString(),
    setReadOnly(value) {
      view.dispatch({ effects: readOnly.reconfigure(readOnlyExtension(value)) });
    },
    setLanguage(path) {
      const seq = ++languageSeq;
      void languageFor(path).then((ext) => seq === languageSeq && view.dispatch({ effects: language.reconfigure(ext) }));
    },
    destroy: () => view.destroy(),
  };
}
