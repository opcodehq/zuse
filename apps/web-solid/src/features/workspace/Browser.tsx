// The browser surface: the project's running previews and an embedded page. A web client can only
// embed what the page allows, so this uses an isolated, credentialless iframe and says plainly when a
// page can't be shown here. Rules follow apps/web/src/browser_frame.js.
import { createMemo, createSignal, For, Show } from "solid-js";
import { area } from "@/features/connection/status";
import { Icon } from "@/ui/Icon";
import { Button, IconButton, Spinner } from "@/ui/primitives";
import { Banner, Empty } from "./parts";
import { workspace, type Target } from "./store";
import type { PreviewService } from "./types";

const credentialless = typeof HTMLIFrameElement !== "undefined" && "credentialless" in HTMLIFrameElement.prototype;
const LOOPBACK = ["localhost", "127.0.0.1", "[::1]", "::1"];

/** Preview proxy addresses resolve only on the engine's own computer. */
const browserLocal = () => area.mode === "owner" && LOOPBACK.includes(location.hostname);

function normalize(text: string): URL {
  const value = text.trim();
  if (!value || value.length > 8192) throw new Error("Enter a web address.");
  if (/^(javascript|data|file|blob|about|vbscript):/i.test(value)) throw new Error("Use an HTTP or HTTPS address.");
  const loopback = /^(localhost|127\.0\.0\.1|\[::1\]|[a-z0-9-]+\.localhost)(?::\d+)?(?:[/?#]|$)/i.test(value);
  const url = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? value : `${loopback ? "http" : "https"}://${value}`);
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password) throw new Error("Use an HTTP or HTTPS address without embedded credentials.");
  if (url.origin === location.origin) throw new Error("Open a project or website address, rather than the Zuse application.");
  return url;
}

export function previewUrl(service: PreviewService, proxyPort: number) {
  return `http://${service.hostname}:${proxyPort}`;
}

export function Browser(props: { target: Target }) {
  const [address, setAddress] = createSignal("");
  const [url, setUrl] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  // A refused address leaves the current page in place; only page failures replace it.
  const [invalid, setInvalid] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);
  const [generation, setGeneration] = createSignal(0);
  let frame: HTMLIFrameElement | undefined;

  const previews = () => workspace.previews[props.target.chatId];
  const services = createMemo(() => previews()?.services ?? []);
  const mixedContent = () => location.protocol === "https:" && url()?.startsWith("http:");

  const navigate = (text: string, preview = false) => {
    setInvalid(null);
    let next: URL;
    try {
      next = normalize(text);
      if (preview && !browserLocal()) throw new Error("This preview uses the execution computer's local proxy. Enter its published URL to open it from this browser.");
    } catch (e) {
      setInvalid(e instanceof Error ? e.message : "The page could not open.");
      return;
    }
    setError(null);
    setAddress(next.href);
    setUrl(next.href);
    if (!credentialless) {
      setError("This browser can't isolate an embedded page. Open the address in a new tab.");
      return;
    }
    setLoading(true);
    setGeneration((g) => g + 1);
  };

  const reload = () => {
    if (!frame || !url()) return;
    setLoading(true);
    // Reassigning src reloads a cross-origin frame; its location is not readable from here.
    frame.src = url()!;
  };

  return (
    <div class="ws-surface-col ws-browser">
      <form
        class="ws-panel-header"
        onSubmit={(e) => {
          e.preventDefault();
          navigate(address());
        }}
      >
        <IconButton icon="refresh" label="Reload page" size="sm" iconSize={14} disabled={!url() || !credentialless} onClick={reload} />
        <div class="ws-search ws-address">
          <Show when={loading()} fallback={<Icon name="globe" size={14} class="ws-faint" />}>
            <Spinner label="Loading page" />
          </Show>
          <input
            class="ws-search-input"
            type="text"
            inputmode="url"
            spellcheck={false}
            autocomplete="off"
            placeholder="Enter a web address"
            aria-label="Web address"
            value={address()}
            onInput={(e) => {
              setAddress(e.currentTarget.value);
              setInvalid(null);
            }}
          />
        </div>
        <IconButton icon="arrow-right" label="Open address" size="sm" iconSize={14} type="submit" disabled={!address().trim()} />
        <IconButton icon="arrow-up-right" label="Open page in a new tab" size="sm" iconSize={14} disabled={!url()} onClick={() => url() && window.open(url()!, "_blank", "noopener,noreferrer")} />
      </form>
      <Show when={services().length}>
        <div class="ws-preview-strip" role="list" aria-label="Running previews">
          <For each={services()}>
            {(service) => (
              <div role="listitem">
                <button
                  type="button"
                  class="ws-preview-chip"
                  data-current={url()?.startsWith(previewUrl(service, previews()!.proxyPort)) ? "true" : undefined}
                  title={`${service.name} on port ${service.port}${service.deviceName ? ` · ${service.deviceName}` : ""}`}
                  onClick={() => navigate(previewUrl(service, previews()!.proxyPort), true)}
                >
                  <span class="ws-dot" data-tone="success" aria-hidden="true" />
                  <span class="ws-truncate">{service.name}</span>
                  <span class="ws-faint">{service.port}</span>
                </button>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={previews()?.error || previews()?.failed}>
        <Banner tone="warning">{previews()?.error || previews()?.failed}</Banner>
      </Show>
      <Show when={invalid()}>
        <Banner tone="danger">{invalid()}</Banner>
      </Show>
      <Show when={error()}>
        <Banner tone="danger" actions={<Show when={url()}><Button size="sm" variant="ghost" onClick={() => window.open(url()!, "_blank", "noopener,noreferrer")}>Open in new tab</Button></Show>}>
          {error()}
        </Banner>
      </Show>
      <Show when={mixedContent() && !error()}>
        <Banner tone="warning" actions={<Button size="sm" variant="ghost" onClick={() => window.open(url()!, "_blank", "noopener,noreferrer")}>Open in new tab</Button>}>
          This app is served over HTTPS, so the browser blocks embedding an HTTP page.
        </Banner>
      </Show>
      <Show
        when={url() && credentialless && !error()}
        fallback={
          <Empty
            icon="globe"
            title="Web preview"
            detail={
              !credentialless
                ? "Embedded pages are unavailable in this browser. Open the address in a new tab."
                : browserLocal()
                  ? services().length
                    ? "Open a running project preview above, or enter a website address."
                    : "Start the project's dev server and its preview appears here, or enter a website address."
                  : "Project preview addresses are local to the computer running the agent. Enter a published URL to open it from this browser."
            }
          />
        }
      >
        <Show when={generation()} keyed>
          {(g: number) => (
            <iframe
              data-generation={g}
              ref={(el) => {
                frame = el;
                // Isolation must be in place before the first navigation.
                (el as HTMLIFrameElement & { credentialless: boolean }).credentialless = true;
                queueMicrotask(() => (el.src = url() ?? "about:blank"));
              }}
              class="ws-browser-frame"
              title="Web preview"
              referrerPolicy="no-referrer"
              sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads"
              allow="camera 'none'; microphone 'none'; geolocation 'none'"
              onLoad={(e) => e.currentTarget.getAttribute("src") && setLoading(false)}
              onError={() => {
                setLoading(false);
                setError("The page could not load. Try opening it in a new tab.");
              }}
            />
          )}
        </Show>
        <div class="ws-browser-note ws-faint ws-small">Pages that refuse embedding show blank here; open them in a new tab.</div>
      </Show>
    </div>
  );
}
