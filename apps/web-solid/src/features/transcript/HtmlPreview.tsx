import { createEffect, createMemo, createSignal, onCleanup, Show, untrack } from "solid-js";
import type { JSX } from "@solidjs/web";
import { resolvedTheme } from "@/shell/theme";
import { uuid } from "@/core/util";
import { Icon } from "@/ui/Icon";
import { copyText } from "./clipboard";
import { HTML_PREVIEW_LIMIT, HTML_PREVIEW_SANDBOX, previewDocument } from "./html-preview";
import "./html-preview.css";

export function HtmlPreview(props: { code: string; ready: boolean; source: JSX.Element }) {
  let dialog!: HTMLDialogElement;
  let frame: HTMLIFrameElement | undefined;
  let expandButton!: HTMLButtonElement;
  const channel = uuid();
  const [source, setSource] = createSignal(false);
  const [expanded, setExpanded] = createSignal(false);
  const [height, setHeight] = createSignal(320);
  const [error, setError] = createSignal(false);
  const [copied, setCopied] = createSignal(false);
  const [revision, setRevision] = createSignal(0);
  const eligible = () => props.ready && props.code.length <= HTML_PREVIEW_LIMIT;
  // Theme updates travel by message, so changing appearance does not reset interactive state.
  const document = createMemo(() => { const version = revision(); return eligible() ? previewDocument(props.code, channel, untrack(resolvedTheme)) + `<!-- revision ${version} -->` : ""; });
  const collapse = () => {
    dialog.close();
    dialog.setAttribute("open", "");
    setExpanded(false);
    expandButton.focus();
  };
  const expand = () => {
    dialog.removeAttribute("open");
    dialog.showModal();
    setExpanded(true);
  };
  const sendTheme = () => {
    const style = getComputedStyle(window.document.documentElement);
    const colors = Object.fromEntries(Object.entries({ bg: "--card", text: "--text", muted: "--text-muted", accent: "--accent", panel: "--raised", border: "--border-strong" }).map(([name, token]) => [name, style.getPropertyValue(token).trim()]));
    frame?.contentWindow?.postMessage({ channel, type: "theme", theme: untrack(resolvedTheme), colors }, "*");
  };
  // Listen only to this frame; never interpret author messages as app commands or markup.
  const message = (event: MessageEvent) => {
    if (!frame || event.source !== frame.contentWindow || event.data?.channel !== channel) return;
    const data = event.data;
    if (data.type === "size" && !expanded() && typeof data.height === "number" && Number.isFinite(data.height)) setHeight(Math.min(640, Math.max(200, data.height)));
    if (data.type === "ready") sendTheme();
    if (data.type === "error") setError(true);
    if (data.type === "escape" && expanded()) collapse();
  };
  window.addEventListener("message", message);
  let reset: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => { window.removeEventListener("message", message); clearTimeout(reset); });
  createEffect(resolvedTheme, sendTheme);
  createEffect(() => props.code, () => { setError(false); setHeight(320); });
  const copy = async () => {
    if (!await copyText(props.code)) return;
    setCopied(true);
    clearTimeout(reset);
    reset = setTimeout(() => setCopied(false), 1500);
  };
  return (
    <dialog ref={dialog} open class="html-preview" role={expanded() ? "dialog" : "region"} aria-label="HTML preview" onCancel={event => { event.preventDefault(); collapse(); }}>
      <div class="html-preview-head">
        <div class="html-preview-label"><Icon name="file-code" size={14} /><span>HTML</span><span class="html-preview-dot" /><small>{!props.ready ? "Writing…" : "Interactive preview"}</small></div>
        <div class="html-preview-controls">
          <div class="html-preview-switch" role="group" aria-label="HTML view">
            <button type="button" aria-pressed={!source() ? "true" : "false"} onClick={() => setSource(false)}>Preview</button>
            <button type="button" aria-pressed={source() ? "true" : "false"} onClick={() => setSource(true)}>Source</button>
          </div>
          <button type="button" class="html-preview-action" aria-label="Restart preview" title="Restart preview" disabled={!eligible()} onClick={() => { setError(false); setRevision(revision() + 1); }}><Icon name="refresh" size={14} /></button>
          <button type="button" class="html-preview-action" aria-label={copied() ? "Copied HTML" : "Copy HTML"} title={copied() ? "Copied" : "Copy HTML"} onClick={copy}><Icon name={copied() ? "check" : "copy"} size={14} /></button>
          <button ref={expandButton} type="button" class="html-preview-action" aria-label={expanded() ? "Collapse preview" : "Expand preview"} title={expanded() ? "Collapse preview" : "Expand preview"} onClick={() => expanded() ? collapse() : expand()}><Icon name={expanded() ? "collapse-arrows" : "expand-arrows"} size={14} /></button>
        </div>
      </div>
      <div class="html-preview-source" hidden={!source()}>{props.source}</div>
      <div class="html-preview-stage" hidden={source()}>
        <Show when={eligible()} fallback={<div class="html-preview-placeholder"><Icon name="file-code" size={22} /><b>{props.code.length > HTML_PREVIEW_LIMIT ? "This document is too large to preview" : "Putting the preview together"}</b><span>{props.code.length > HTML_PREVIEW_LIMIT ? "Use Source to read or copy the HTML." : "The preview opens when the HTML block is complete."}</span></div>}>
          <iframe ref={frame} title="Generated HTML preview" sandbox={HTML_PREVIEW_SANDBOX} referrerpolicy="no-referrer" allow="camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'" srcdoc={document()} style={{ height: `${height()}px` }} onLoad={sendTheme} />
        </Show>
      </div>
      <Show when={error()}><div class="html-preview-error" role="status">A script encountered an error. View the source or restart the preview.</div></Show>
      <div class="html-preview-foot"><span>HTML · CSS · JavaScript</span><span>Isolated preview</span></div>
    </dialog>
  );
}
