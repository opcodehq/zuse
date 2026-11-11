// One DOM update per animation frame for streamed values, however many frames arrive from the engine.
import { type Accessor, createEffect, createSignal, onCleanup, untrack } from "solid-js";

const queue = new Set<() => void>();
let scheduled = 0;

function flush() {
  scheduled = 0;
  const jobs = [...queue];
  queue.clear();
  for (const job of jobs) job();
}

/** Run `job` on the next animation frame, coalescing repeats. */
export function onNextFrame(job: () => void) {
  queue.add(job);
  scheduled ||= requestAnimationFrame(flush);
}

/** Mirrors `source` but publishes at most once per frame while `live()` is true; otherwise immediately. */
export function frameThrottled<T>(source: Accessor<T>, live: Accessor<boolean>): Accessor<T> {
  const [value, setValue] = createSignal<T>(untrack(source), { equals: false });
  const publish = () => setValue(() => untrack(source));
  createEffect(() => {
    source();
    if (live()) onNextFrame(publish);
    else {
      queue.delete(publish);
      publish();
    }
  });
  onCleanup(() => queue.delete(publish));
  return value;
}
