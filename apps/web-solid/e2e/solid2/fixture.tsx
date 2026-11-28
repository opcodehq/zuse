import { createEffect, createRoot, createMemo, createSignal, createStore, flush, For, onSettled, Show } from "solid-js";
import { render } from "@solidjs/web";
import type { SessionMessageEntry, TranscriptFrame } from "@/core/protocol";
import { applyTranscriptFrame, TranscriptDesync } from "@/core/transcript";
import { Dialog, Menu } from "@/ui/primitives";
import { createVirtualizer } from "@/ui/virtualizer";

import { rpc } from "@/core/connection";
import { M } from "@/core/methods";
import { setState, state } from "@/core/store";
import { useUsageResume } from "@/features/composer/usageResume";

const check = (value: unknown, message: string) => { if (!value) throw new Error(message); };
const entry = (id: string, text = id): SessionMessageEntry => ({
  id, role: "assistant", createdAt: 0, deviceId: "fixture", status: "complete",
  parts: [{ id: `${id}-text`, kind: "text", text }],
});

function Fixture() {
  const [messages, setMessages] = createStore<SessionMessageEntry[]>([]);
  const [mounted, setMounted] = createSignal(true);
  const [open, setOpen] = createSignal(false);
  const [choice, setChoice] = createSignal("");
  let scroller!: HTMLDivElement;
  let effectRuns = 0;
  let cleanups = 0;
  let rowMounts = 0;
  const [count, setCount] = createSignal(0);
  const virtualizer = createVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: 1000,
    getScrollElement: () => scroller,
    estimateSize: () => 24,
    overscan: 3,
  });
  const apply = (frame: TranscriptFrame) => flush(() => applyTranscriptFrame(setMessages, messages, frame));
  const lifecycle = () => {
    createEffect(count, () => { effectRuns++; return () => { cleanups++; }; });
    return <span>Effect mounted</span>;
  };

  function transcriptChecks() {
    apply({ reset: [entry("a"), entry("b"), entry("c")] });
    const first = document.querySelector('[data-transcript-row="a"]');
    apply({ reset: [entry("a"), entry("b"), entry("c", "changed")] });
    check(document.querySelector('[data-transcript-row="a"]') === first, "reset must preserve surviving row DOM");
    apply({ remove: ["b"], upsert: [{ after: "a", entry: entry("d") }, { after: "d", entry: entry("e") }],
      append: [{ entry: "e", part: "e-text", text: "!", len: 2 }], count: 4 });
    check(messages.map(row => row.id).join(",") === "a,d,e,c", "chained upserts must see earlier draft writes");
    check(messages[2]?.parts[0]?.kind === "text" && messages[2].parts[0].text === "e!", "append must find an entry inserted in the same frame");
    const before = JSON.stringify(messages);
    try {
      apply({ remove: ["a"], append: [{ entry: "e", part: "e-text", text: "bad", len: 999 }], count: 3 });
      throw new Error("invalid append was accepted");
    } catch (error) { check(error instanceof TranscriptDesync, "invalid frames must fail validation"); }
    check(JSON.stringify(messages) === before, "invalid frame must leave every row unchanged");
  }

  (window as unknown as { solid2: unknown }).solid2 = {
    transcriptChecks,
    recoveryChecks,
    counters: () => ({ effectRuns, cleanups, rowMounts }),
    update: () => { setCount(1); setCount(2); flush(); },
    unmount: () => { setMounted(false); flush(); },
    measure: () => virtualizer.measure(),
    choice,
  };
  const keys = createMemo(() => messages.map(row => row.id));
  return <>
    <div id="transcript"><For each={keys()}>{id => <span data-transcript-row={id}>{id}</span>}</For></div>
    <Show when={mounted()}>{lifecycle}</Show>
    <button id="dialog-trigger" onClick={() => setOpen(true)}>Open dialog</button>
    <Dialog open={open()} onOpenChange={setOpen} title="Migration dialog"><button id="dialog-close" onClick={() => setOpen(false)}>Close</button></Dialog>
    <Menu trigger={attrs => <button {...attrs} id="menu-trigger">Open menu</button>} items={[{ label: "Pick this", onSelect: () => setChoice("picked") }]} />
    <div ref={scroller} id="virtual" style={{ height: "240px", overflow: "auto" }}>
      <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
        <For each={virtualizer.getVirtualItems()}>{item => {
          rowMounts++;
          let row!: HTMLDivElement;
          onSettled(() => virtualizer.measureElement(row));
          return <div ref={row} class="virtual-row" data-index={item.index} style={{ position: "absolute", top: `${item.start}px`, height: "24px" }}>Row {item.index}</div>;
        }}</For>
      </div>
    </div>
  </>;
}
render(() => <Fixture />, document.getElementById("root")!);

/** A failed or skipped cancellation must never report success to the tray. */
function recoveryChecks() {
  const original = rpc.call;
  const connection = state.connection;
  setState(draft => { draft.connection = { kind: "connecting" }; });
  return createRoot(dispose => {
    const status = { enabled: true, pending: { enabled: false, resetsAt: null } };
    let mutations = 0;
    let rejectMutation: (error: Error) => void = () => {};
    rpc.call = (async (method: string) => {
      if (method === M.GET_USAGE_RESUME) return status;
      mutations++;
      return new Promise((_resolve, reject) => { rejectMutation = reject; });
    }) as typeof rpc.call;
    const recovery = useUsageResume(() => null);
    return (async () => {
      try {
        check(await recovery.setEnabled(false) === false, "offline cancellation must fail explicitly");
        check(mutations === 0, "offline cancellation must not send");
        setState(draft => { draft.connection = { kind: "connected" }; });
        flush();
        const pending = recovery.setEnabled(false);
        flush();
        check(await recovery.setEnabled(false) === false, "busy cancellation must fail explicitly");
        check(mutations === 1, "busy cancellation must not send twice");
        rejectMutation(new Error("Cancellation failed"));
        check(await pending === false, "RPC rejection must not dismiss a scheduled error");
        flush();
        rpc.call = (async () => status) as typeof rpc.call;
        check(await recovery.setEnabled(false) === true, "confirmed cancellation must report success");
        flush();
        let finish: (value: typeof status) => void = () => {};
        rpc.call = (() => new Promise(resolve => { finish = resolve; })) as typeof rpc.call;
        const stale = recovery.setEnabled(false);
        setState(draft => { draft.epoch++; });
        flush();
        finish(status);
        check(await stale === false, "old cancellation must not dismiss a different chat or connection");
      } finally {
        dispose();
        rpc.call = original;
        setState(draft => { draft.connection = connection; });
      }
    })();
  });
}
