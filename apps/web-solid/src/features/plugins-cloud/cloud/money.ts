// Exact money conversion from crates/model/src/cloud_billing.rs. Amounts are integers; never floats.
export const MAX_SAFE_MICROS = Number.MAX_SAFE_INTEGER;

export function capMicros(input: string): number | null {
  const parts = input.trim().split(".");
  if (parts.length > 2) return null;
  const whole = parts[0] ?? "";
  const fraction = parts[1] ?? "";
  if (!whole || fraction.length > 6 || !/^\d*$/.test(whole + fraction)) return null;
  const micros = BigInt(whole) * 1_000_000n + BigInt(fraction.padEnd(6, "0") || "0");
  return micros <= BigInt(MAX_SAFE_MICROS) ? Number(micros) : null;
}

export function capDollars(micros: number) {
  const whole = Math.trunc(micros / 1_000_000);
  const fraction = micros % 1_000_000;
  return fraction === 0 ? String(whole) : `${whole}.${String(fraction).padStart(6, "0")}`.replace(/0+$/, "");
}

export function usd(micros: number) {
  const value = BigInt(Math.trunc(micros));
  const abs = value < 0n ? -value : value;
  const cents = (abs + 5000n) / 10000n;
  return `${value < 0n ? "−" : ""}$${cents / 100n}.${String(cents % 100n).padStart(2, "0")}`;
}

export function cents(value: number) {
  const amount = Math.abs(Math.trunc(value));
  return `${value < 0 ? "-" : ""}$${Math.floor(amount / 100)}.${String(amount % 100).padStart(2, "0")}`;
}

export const STANDARD_OFFER = "cloud-workspace-standard-v1";
export const CREDIT_AMOUNTS = [1000, 2500, 5000, 10000] as const;

interface Entitlement {
  offerId?: string;
  status?: string;
  paidThrough?: number | null;
}
/** Presentation only; checkout reconciles canonical provider state. */
export function cloudSubscribed(value: unknown, now: number): boolean | null {
  const list = (value as { entitlements?: unknown } | null)?.entitlements;
  if (!Array.isArray(list)) return null;
  return (list as Entitlement[]).some(
    (ent) => ent.offerId === STANDARD_OFFER && ["pending", "active", "grace"].includes(ent.status ?? "") && (typeof ent.paidThrough !== "number" || ent.paidThrough > now),
  );
}
