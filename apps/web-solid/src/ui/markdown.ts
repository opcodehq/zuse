// The one sanitizer for markdown nobody here wrote: agent output and provider content (PR bodies, review comments).
import DOMPurify from "dompurify";
import { Marked, type MarkedExtension, type Tokens } from "marked";

export const escapeHtml = (text: string) =>
  text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

export const untrustedRenderer: MarkedExtension = {
  renderer: {
    // Raw HTML is never trusted markup: it shows as text.
    html: (token: Tokens.HTML | Tokens.Tag) => escapeHtml(token.text),
    // Remote images would load third-party content (and report the viewer); show their alt text as a link.
    image: (token: Tokens.Image) => `<a href="${escapeHtml(token.href)}">${escapeHtml(token.text || token.href)}</a>`,
  },
};

let hooked = false;
export function purify(html: string) {
  if (!hooked) {
    hooked = true;
    DOMPurify.addHook("afterSanitizeAttributes", (node) => {
      if (node.tagName === "A") {
        const href = node.getAttribute("href") ?? "";
        if (/^https?:|^mailto:/i.test(href)) {
          node.setAttribute("target", "_blank");
          node.setAttribute("rel", "noopener noreferrer");
          node.setAttribute("data-web", "");
        } else {
          // Workspace paths and relative links have no web destination; keep the label and show the path on hover.
          node.removeAttribute("href");
          if (href) node.setAttribute("title", href);
          node.setAttribute("data-local", "");
        }
      }
      if (node.tagName === "INPUT") node.setAttribute("disabled", "");
    });
  }
  return DOMPurify.sanitize(html, {
    FORBID_TAGS: ["style", "img", "form", "iframe", "svg", "math", "button", "textarea", "select"],
    FORBID_ATTR: ["style"],
  });
}

const provider = new Marked({ gfm: true, breaks: true });
provider.use(untrustedRenderer);

/** Sanitized HTML for provider markdown, with GitHub's line breaks. */
export function providerMarkdownHtml(text: string): string {
  return purify(provider.parse(text, { async: false }) as string);
}
