// English display rules shared by the overview, axes, tooltip and tables (crates/viewport/src/usage_dashboard/format.rs).
const UNITS: [number, string][] = [
  [1e3, "K"],
  [1e6, "M"],
  [1e9, "B"],
  [1e12, "T"],
];

const grouped = (digits: string) => digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");

export function tokens(value: number): string {
  let unit = -1;
  for (let i = UNITS.length - 1; i >= 0; i--) if (value >= UNITS[i]![0]) { unit = i; break; }
  if (unit < 0) return String(Math.round(value));
  const tenths = (u: number) => Math.round((value * 10) / UNITS[u]![0]);
  if (tenths(unit) >= 10_000 && unit + 1 < UNITS.length) unit++;
  const t = tenths(unit);
  const whole = Math.floor(t / 10);
  return `${whole >= 10_000 ? grouped(String(whole)) : whole}${t % 10 ? `.${t % 10}` : ""}${UNITS[unit]![1]}`;
}

const currency = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: 2 });
export const money = (value: number | null | undefined) => (value === null || value === undefined || !Number.isFinite(value) ? "N/A" : currency.format(value));

export const share = (value: number) => `${value.toFixed(1)}%`;

export function delta(value: number | null | undefined, previous: number | null | undefined): string | null {
  if (value === null || value === undefined || previous === null || previous === undefined || previous === 0) return null;
  const d = Math.floor(((value - previous) / previous) * 100 + 0.5);
  return `${d > 0 ? "+" : ""}${d}% vs prior`;
}
