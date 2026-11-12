// Text with matched ranges marked, as desktop's search highlight does.
import { For, Show } from "solid-js";

export function Highlight(props: { text: string; ranges: [number, number][] }) {
  const pieces = () => {
    const out: { text: string; hit: boolean }[] = [];
    let at = 0;
    for (const [start, end] of props.ranges) {
      if (start < at) continue;
      if (start > at) out.push({ text: props.text.slice(at, start), hit: false });
      out.push({ text: props.text.slice(start, end), hit: true });
      at = end;
    }
    if (at < props.text.length) out.push({ text: props.text.slice(at), hit: false });
    return out;
  };
  return (
    <Show when={props.ranges.length} fallback={props.text}>
      <For each={pieces()}>{(piece) => (piece.hit ? <mark class="x-hit">{piece.text}</mark> : piece.text)}</For>
    </Show>
  );
}
