// Cloud plans and machine-minute copy (crates/protocol/src/cloud_billing.rs `plans`). Pure, so the copy is testable without a DOM.
import { CLOUD_MAX_OFFER, CLOUD_OFFER, count, dollars, hours } from "./money";
import type { BillingUsageItem, CloudCapabilities, PlanCatalog, PlanId, PlanOffer, PlanUsage } from "./types";

/** Machine minutes the default 4 vCPU / 16 GB machine uses per awake minute. */
export const DEFAULT_RATE = 4;
export const ENTERPRISE_CONTACT_FALLBACK = "mailto:swaraj@zuse.sh?subject=Zuse%20Enterprise";

/** Older servers send no catalog; these are the plans they sell. The trial is never offered without the server saying so. */
export const FALLBACK_CATALOG: PlanCatalog = {
  plans: [
    { offerId: CLOUD_OFFER, planId: "cloud", name: "Cloud", priceMicros: 20_000_000, includedMachineMinutes: 33_600, maxVcpus: 4, maxMemoryMib: 16384, payAsYouGo: true, available: true },
    { offerId: CLOUD_MAX_OFFER, planId: "cloud-max", name: "Cloud Max", priceMicros: 50_000_000, includedMachineMinutes: 84_000, maxVcpus: 8, maxMemoryMib: 32768, payAsYouGo: true, available: true },
  ],
  trialAvailable: false,
  trialDays: 7,
  trialPriceMicros: 1_000_000,
  trialMachineMinutes: 8_400,
  enterpriseContactUrl: ENTERPRISE_CONTACT_FALLBACK,
};

const num = (value: unknown, fallback: number) => (typeof value === "number" && Number.isFinite(value) ? value : fallback);

/** Reads `catalog` from the entitlements reply, falling back to the static plans when it is absent or malformed. */
export function readCatalog(value: unknown): PlanCatalog {
  const catalog = (value as { catalog?: Partial<PlanCatalog> } | null)?.catalog;
  if (!catalog || !Array.isArray(catalog.plans)) return FALLBACK_CATALOG;
  const plans = catalog.plans.filter(
    (p): p is PlanOffer => !!p && typeof p.offerId === "string" && typeof p.priceMicros === "number" && typeof p.includedMachineMinutes === "number",
  );
  return {
    plans: plans.length ? plans : FALLBACK_CATALOG.plans,
    trialAvailable: catalog.trialAvailable === true,
    trialDays: num(catalog.trialDays, FALLBACK_CATALOG.trialDays),
    trialPriceMicros: num(catalog.trialPriceMicros, FALLBACK_CATALOG.trialPriceMicros),
    trialMachineMinutes: num(catalog.trialMachineMinutes, FALLBACK_CATALOG.trialMachineMinutes),
    enterpriseContactUrl: typeof catalog.enterpriseContactUrl === "string" && catalog.enterpriseContactUrl ? catalog.enterpriseContactUrl : ENTERPRISE_CONTACT_FALLBACK,
  };
}

export const machine = (vcpus: number, memoryMib: number) => `${vcpus} vCPU / ${Math.round(memoryMib / 1024)} GB`;
/** The machine a rate describes: one vCPU and 4 GB per machine minute per minute. */
const rateMachine = (rate: number) => machine(rate, rate * 4096);
const safeRate = (rate: number | null | undefined) => (typeof rate === "number" && rate > 0 ? rate : DEFAULT_RATE);

/** "about 140 h": awake hours of a machine with `rate` that `machineMinutes` cover. */
export const approxHours = (machineMinutes: number, rate = DEFAULT_RATE) => `about ${hours(machineMinutes / safeRate(rate))}`;
export const machineMinutesLabel = (value: number) => `${count(value)} machine minute${Math.floor(value) === 1 ? "" : "s"}`;

/** The cloud plan to use when starting or comparing: a trial or legacy subscription compares as Cloud. */
export const tierOf = (planId: PlanId | null | undefined): "cloud" | "cloud-max" | null =>
  planId === "cloud-max" ? "cloud-max" : planId === "cloud" || planId === "legacy" || planId === "cloud-trial" ? "cloud" : null;

export function trialCopy(catalog: PlanCatalog) {
  const cloud = catalog.plans.find((p) => p.planId === "cloud") ?? FALLBACK_CATALOG.plans[0]!;
  const week = catalog.trialDays === 7 ? "a week" : `${catalog.trialDays} days`;
  return (
    `Try Cloud for ${dollars(catalog.trialPriceMicros)} for ${week} — ${count(catalog.trialMachineMinutes)} machine minutes ` +
    `(${approxHours(catalog.trialMachineMinutes)} on a 4 vCPU / 16 GB computer), then ${dollars(cloud.priceMicros)}/month for ${count(cloud.includedMachineMinutes)}.`
  );
}
export const TRIAL_COPY = trialCopy(FALLBACK_CATALOG);

export const priceLine = (priceMicros: number) => `${dollars(priceMicros)}/month`;

/** "33,600 machine minutes · about 140 h on 4 vCPU / 16 GB" for a plan on sale. */
export const offerAllowance = (offer: Pick<PlanOffer, "includedMachineMinutes">) =>
  `${machineMinutesLabel(offer.includedMachineMinutes)} · ${approxHours(offer.includedMachineMinutes)} on 4 vCPU / 16 GB`;

/** "Computers up to 8 vCPU / 32 GB". */
export const offerMachines = (offer: Pick<PlanOffer, "maxVcpus" | "maxMemoryMib">) => `Computers up to ${machine(offer.maxVcpus, offer.maxMemoryMib)}`;

/** "Cloud · $20/month"; a trial reads as its name alone, its price is in the trial line. */
export const planTitle = (p: Pick<PlanUsage, "planName" | "priceMicros" | "trial">) => (p.trial ? p.planName : `${p.planName} · ${priceLine(p.priceMicros)}`);

/** "33,600 machine minutes · about 140 h on your 4 vCPU / 16 GB machine". */
export const allowanceLine = (p: Pick<PlanUsage, "includedMachineMinutes" | "defaultMachineRate">) =>
  `${machineMinutesLabel(p.includedMachineMinutes)} · ${approxHours(p.includedMachineMinutes, p.defaultMachineRate)} on your ${rateMachine(safeRate(p.defaultMachineRate))} machine`;

export const shortDate = (ms: number, locale?: string) => new Date(ms).toLocaleDateString(locale, { month: "short", day: "numeric" });

export const renewLine = (p: Pick<PlanUsage, "renewsAt">, locale?: string) => `Renews ${shortDate(p.renewsAt, locale)}`;

/** "12,480 of 33,600 machine minutes used · 21,120 left". */
export const meterLine = (p: Pick<PlanUsage, "usedMachineMinutes" | "includedMachineMinutes" | "remainingMachineMinutes">) =>
  `${count(Math.min(p.usedMachineMinutes, p.includedMachineMinutes))} of ${machineMinutesLabel(p.includedMachineMinutes)} used · ${count(p.remainingMachineMinutes)} left`;

/** "about 88 h left on 4 vCPU / 16 GB". */
export const meterHint = (p: Pick<PlanUsage, "remainingMachineMinutes" | "defaultMachineRate">) =>
  `${approxHours(p.remainingMachineMinutes, p.defaultMachineRate)} left on ${rateMachine(safeRate(p.defaultMachineRate))}`;

/** Share of the allowance used, 0 to 1. */
export const usedFraction = (p: Pick<PlanUsage, "usedMachineMinutes" | "includedMachineMinutes">) =>
  p.includedMachineMinutes > 0 ? Math.max(0, Math.min(1, p.usedMachineMinutes / p.includedMachineMinutes)) : p.usedMachineMinutes > 0 ? 1 : 0;

/** "2,880 extra machine minutes this month ($1.71) · cap $25"; null when there is no extra use. */
export function extraLine(p: Pick<PlanUsage, "extraMachineMinutes" | "extraChargeMicros" | "payAsYouGo" | "overageCapMicros">) {
  if (p.extraMachineMinutes <= 0) return null;
  const line = `${count(p.extraMachineMinutes)} extra machine minutes this month (${dollars(p.extraChargeMicros)})`;
  return p.payAsYouGo ? `${line} · cap ${dollars(p.overageCapMicros)}` : line;
}

/** "Trial · ends Nov 26 · 3,200 of 8,400 machine minutes left". */
export function trialLine(p: Pick<PlanUsage, "trialEndsAt" | "remainingMachineMinutes" | "includedMachineMinutes">, locale?: string) {
  const parts = ["Trial"];
  if (p.trialEndsAt) parts.push(`ends ${shortDate(p.trialEndsAt, locale)}`);
  parts.push(`${count(p.remainingMachineMinutes)} of ${machineMinutesLabel(p.includedMachineMinutes)} left`);
  return parts.join(" · ");
}

export type HoldAction = "start-cloud" | "raise-cap" | "upgrade-cloud-max";
export interface Hold {
  message: string;
  actions: HoldAction[];
}
/** Why cloud computers are paused, and what clears it. Null when nothing is on hold. */
export function holdNotice(p: Pick<PlanUsage, "usageHold" | "holdReason" | "overageCapMicros" | "planId" | "renewsAt">, locale?: string): Hold | null {
  if (!p.usageHold) return null;
  switch (p.holdReason) {
    case "trial-allowance-used":
      return { message: "Your trial minutes are used up. Cloud computers are paused.", actions: ["start-cloud"] };
    case "trial-unavailable":
      return { message: "This card or account has already used a trial.", actions: ["start-cloud"] };
    case "cap-reached":
      return {
        message: `You reached your ${dollars(p.overageCapMicros)} pay-as-you-go cap. Cloud computers are paused until ${shortDate(p.renewsAt, locale)}.`,
        actions: p.planId === "cloud-max" ? ["raise-cap"] : ["raise-cap", "upgrade-cloud-max"],
      };
    default:
      return { message: "Cloud computers are paused.", actions: [] };
  }
}

/** Machine minutes a usage entry used; entries without a count fall back to awake time times the rate. */
export function usageMinutes(item: Pick<BillingUsageItem, "machineMinutes" | "machineRate" | "awakeMs">): number | null {
  if (typeof item.machineMinutes === "number") return Math.max(0, item.machineMinutes);
  if (typeof item.awakeMs === "number") return Math.floor((Math.max(0, item.awakeMs) * safeRate(item.machineRate)) / 60_000);
  return null;
}

/** Hover hint for usage: "about 5.2 h on 4 vCPU / 16 GB", with the rate when it is not the default machine. */
export function usageHint(machineMinutes: number, rate: number | null | undefined = DEFAULT_RATE) {
  const r = safeRate(rate);
  const text = `${approxHours(machineMinutes, r)} on ${rateMachine(r)}`;
  return r === DEFAULT_RATE ? text : `${text} · ${r} per minute`;
}

export { cloudErrorText } from "./errors";

/** The internal provider id new cloud computers run on: the server's default, else the first ready provider. Never shown. */
export function defaultProvider(capabilities: CloudCapabilities | null | undefined): string | null {
  if (!capabilities) return null;
  if (typeof capabilities.defaultProvider === "string" && capabilities.defaultProvider) return capabilities.defaultProvider;
  const ready = capabilities.providers?.find((p) => p.configured && p.managedBaseReady && p.preservesDisk !== false && (p.sizes?.length ?? 1) > 0);
  return ready?.id ?? null;
}
