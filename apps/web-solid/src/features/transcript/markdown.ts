// Incremental markdown: a streamed append re-lexes only the open tail block, settled blocks keep identity.
import { Marked, type Token, type Tokens } from "marked";
import { purify, untrustedRenderer } from "@/ui/markdown";

const md = new Marked({ gfm: true, breaks: false });
md.use(untrustedRenderer);

export interface Block {
  /** Stable while the block's source is unchanged. */
  id: number;
  raw: string;
  /** Offset of `raw` in the document. */
  start: number;
  token: Token;
}

let nextBlockId = 1;

/** Lexes a growing document. `update` returns the non-space blocks, updated in place and reusing unchanged block objects. */
export class IncrementalMarkdown {
  private text = "";
  private blocks: Block[] = [];

  update(text: string): Block[] {
    if (text === this.text) return this.blocks;
    // Everything before the tail block is settled: an append can only extend the tail.
    const tail = this.blocks.length - 1;
    const append = tail >= 0 && text.length > this.text.length && text.startsWith(this.text);
    const keep = append ? tail : 0;
    const offset = append ? this.blocks[tail]!.start : 0;
    const tokens = md.Lexer.lex(text.slice(offset), md.defaults);
    const blocks = this.blocks;
    let at = offset;
    let next = keep;
    for (const token of tokens) {
      const start = at;
      at += token.raw.length;
      if (token.type === "space") continue;
      const previous = blocks[next];
      blocks[next++] =
        previous && previous.start === start && previous.raw === token.raw && previous.token.type === token.type ? previous : { id: nextBlockId++, raw: token.raw, start, token };
    }
    blocks.length = next;
    this.text = text;
    return blocks;
  }
}

const htmlCache = new WeakMap<Token, string>();

/** Sanitized HTML for one non-code block. Cached per token object. */
export function blockHtml(token: Token): string {
  let html = htmlCache.get(token);
  if (html === undefined) {
    html = purify(md.Parser.parse([token], md.defaults));
    if (token.type === "table") html = `<div class="md-table">${html}</div>`;
    htmlCache.set(token, html);
  }
  return html;
}

/** Sanitized inner HTML of one list item: a streaming list re-renders only the item that grew. */
export function listItemHtml(item: Tokens.ListItem): string {
  return purify(md.Parser.parse(item.tokens, md.defaults));
}

/** Sanitized inner HTML of one table cell. */
export function tableCellHtml(cell: Tokens.TableCell): string {
  return purify(md.Parser.parseInline(cell.tokens, md.defaults));
}
