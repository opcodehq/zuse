// TanStack's published Solid adapter still targets Solid 1. Keep the framework
// boundary here until upstream publishes its Solid 2 adapter.
import type { PartialKeys,VirtualizerOptions } from "@tanstack/virtual-core";
import { Virtualizer,elementScroll,observeElementOffset,observeElementRect } from "@tanstack/virtual-core";
import { createEffect,createMemo,createSignal,createStore,reconcile,onSettled,untrack } from "solid-js";

export { defaultRangeExtractor } from "@tanstack/virtual-core";

export function createVirtualizer<TScrollElement extends Element, TItemElement extends Element>(
  options: PartialKeys<VirtualizerOptions<TScrollElement, TItemElement>, "observeElementRect" | "observeElementOffset" | "scrollToFn">,
): Virtualizer<TScrollElement, TItemElement> {
  const resolved = createMemo(() => ({
    observeElementRect,
    observeElementOffset,
    scrollToFn: elementScroll,
    ...options,
  }));
  // Construction is a one-time seed; the effect below owns option updates.
  const instance = new Virtualizer<TScrollElement, TItemElement>(untrack(resolved));
  const [items, setItems] = createStore(instance.getVirtualItems());
  const [size, setSize] = createSignal(instance.getTotalSize());
  const update = () => {
    setItems(draft => { reconcile(instance.getVirtualItems(), "index")(draft); });
    setSize(instance.getTotalSize());
  };
  createEffect(resolved, value => {
    instance.setOptions({
      ...value,
      onChange: (virtualizer, sync) => {
        update();
        value.onChange?.(virtualizer, sync);
      },
    });
    instance._willUpdate();
    update();
  });
  onSettled(() => {
    const cleanup = instance._didMount();
    instance._willUpdate();
    update();
    return cleanup;
  });
  return new Proxy(instance, {
    get(target, key, receiver) {
      if (key === "getVirtualItems") return () => items;
      if (key === "getTotalSize") return size;
      return Reflect.get(target, key, receiver);
    },
  });
}
