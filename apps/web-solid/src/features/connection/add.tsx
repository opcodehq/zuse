// Adding a computer from a pasted private connection link.
import { createSignal, Show } from "solid-js";
import { openInThisTab } from "./computers";
import { loadLink } from "./link/wasm";
import { Btn } from "./ui";

/** Paste a private connection link from another computer. Shared with Settings → Devices. */
export function AddComputer(props: { onDone?: () => void; autofocus?: boolean }) {
  const [value, setValue] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const submit = async (event: Event) => {
    event.preventDefault();
    if (busy() || !value().trim()) return;
    setBusy(true);
    setError(null);
    try {
      await addComputer(value());
      setValue("");
      props.onDone?.();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : "Enter a valid private connection link from the other computer.");
    } finally {
      setBusy(false);
    }
  };
  return (
    <form class="cx-label" onSubmit={submit}>
      <div class="cx-screen-form">
        <input
          class="cx-field"
          data-size="lg"
          type="url"
          placeholder="Paste a private connection link"
          aria-label="Private connection link"
          aria-invalid={error() ? "true" : undefined}
          autocomplete="off"
          spellcheck={false}
          autofocus={props.autofocus}
          value={value()}
          onInput={(event) => setValue(event.currentTarget.value)}
        />
        <Btn type="submit" size="lg" tone="solid" disabled={busy() || !value().trim()}>{busy() ? "Connecting…" : "Connect"}</Btn>
      </div>
      <Show when={error()}>{(text) => <span class="cx-screen-error" role="alert">{text()}</span>}</Show>
    </form>
  );
}

/** Validate a pasted link, then open it. A generated LAN certificate has to be accepted on its own page first. */
export async function addComputer(pasted: string) {
  const module = await loadLink();
  const link = module.normalizeLink(pasted);
  const { kind, origin } = module.inspectLink(link);
  if (kind === "lan" && origin !== location.origin) {
    const child = window.open(link, "_blank");
    if (!child) throw new Error("Allow pop-ups to open the other computer in a new tab.");
    child.opener = null;
    return;
  }
  openInThisTab(link);
}
