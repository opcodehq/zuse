import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import { percentLeft, usageResetLabel } from "./usage-meter";
import "./usage-meter.css";

const [clock, setClock] = createSignal(0);
if (typeof window !== "undefined") setInterval(() => setClock((tick) => tick + 1), 15_000);

export function UsageMeter(props: {
  window?: { label: string; usedFraction: number; resetsAt?: string | null };
  variant: "compact" | "detail" | "card";
  heading?: JSX.Element;
  provider?: string;
}) {
  const left = createMemo(() => percentLeft(props.window?.usedFraction));
  const resetAt = createMemo(() => Date.parse(props.window?.resetsAt ?? ""));
  const reset = () => {
    if (left() === null) return "Usage unavailable";
    clock();
    return usageResetLabel(resetAt(), Date.now());
  };
  const value = () => left() === null ? "—" : `${left()}% left`;
  const tone = () => {
    const remaining = left();
    return remaining === null ? undefined : remaining <= 5 ? "critical" : remaining <= 20 ? "warn" : undefined;
  };

  return (
    <div class="usage-meter" data-variant={props.variant} data-tone={tone()}>
      <div class="usage-meter-label" title={props.variant === "card" ? undefined : props.window?.label}>
        {props.heading ?? props.window?.label}
      </div>
      <span class="usage-meter-value">{value()}</span>
      <span class="usage-meter-reset" title={left() !== null && Number.isFinite(resetAt()) ? new Date(resetAt()).toLocaleString() : undefined}>
        {reset()}
      </span>
      <Show when={left() !== null}>
        <div
          class="usage-meter-track"
          role="meter"
          aria-label={`${props.provider ? `${props.provider} ` : ""}${props.window?.label} allowance`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={left() ?? undefined}
          aria-valuetext={value()}
        >
          <div class="usage-meter-fill" style={{ width: `${left()}%` }} />
        </div>
      </Show>
    </div>
  );
}
