import { createMemo, createSignal, For, Show } from "solid-js";
import type { JSX } from "@solidjs/web";
import { openExternal, owner } from "../account";
import { Icon } from "@/ui/Icon";
import { Action, Badge, Group, Note, Row } from "../ui";
import { DitherColumns, DitherMeter, DitherShare } from "./Dither";
import { cents, CLOUD_MAX_OFFER, CLOUD_OFFER, CLOUD_TRIAL_OFFER, count, CREDIT_AMOUNTS, dollars, usd } from "./money";
import {
  allowanceLine,
  extraLine,
  FALLBACK_CATALOG,
  holdNotice,
  machineMinutesLabel,
  meterHint,
  meterLine,
  offerAllowance,
  offerMachines,
  planTitle,
  renewLine,
  tierOf,
  trialCopy,
  trialLine,
  usageHint,
  usedFraction,
  type HoldAction,
} from "./plans";
import { checkout, cloud, clearBillingNotice, loadBilling, loadCredits, openBilling, saveCap, selectCredit, setCapInput } from "./state";
import { byChat, byDay, bySize, type Bucket } from "./usage";
import type { BillingSummary, BillingUsageItem, PlanOffer, PlanUsage } from "./types";

const personal = () => !owner()?.organizationId;
const catalog = () => cloud.billing.catalog ?? FALLBACK_CATALOG;
const plan = () => cloud.billing.summary?.plan ?? null;
const CAP_INPUT = "pc-cloud-cap";
const MACHINE_MINUTE_HELP =
  "A machine minute is one minute of a 1 vCPU / 4 GB machine. A 4 vCPU / 16 GB computer uses 4 per awake minute and an 8 vCPU / 32 GB computer uses 8. Asleep computers use none, and their storage is included.";

function openContact(url: string) {
  if (url.startsWith("https://")) openExternal(url);
  else if (url.startsWith("mailto:")) window.location.href = url;
}

function raiseCap() {
  const input = document.getElementById(CAP_INPUT) as HTMLInputElement | null;
  input?.scrollIntoView({ block: "center", behavior: "smooth" });
  input?.focus();
  input?.select();
}

/** A checkout action. During a trial Cloud starts at once; a plan change opens the billing portal. */
function Checkout(props: { offer: string; primary?: boolean; disabled?: boolean; children: JSX.Element }) {
  return (
    <Action
      primary={props.primary}
      busy={cloud.billing.checkoutBusy === props.offer}
      disabled={props.disabled || !!cloud.billing.checkoutBusy}
      onClick={() => void checkout(props.offer)}
    >
      {props.children}
    </Action>
  );
}
const StartCloud = (props: { primary?: boolean }) => (
  <Checkout offer={CLOUD_OFFER} primary={props.primary ?? true}>
    Start Cloud now
  </Checkout>
);
const UpgradeMax = (props: { primary?: boolean }) => (
  <Checkout offer={CLOUD_MAX_OFFER} primary={props.primary}>
    Upgrade to Cloud Max
  </Checkout>
);

export function Hosting() {
  const subscribed = () => cloud.billing.subscribed;
  const loading = () => cloud.billing.loading || cloud.billing.subscriptionLoading;
  return (
    <div class="pc-stack">
      <div class="pc-identity">
        <h2>Cloud · {personal() ? "Personal workspace" : "Organization workspace"}</h2>
        <Show when={subscribed() === true}>
          <Badge>Ready</Badge>
        </Show>
      </div>
      <Show when={cloud.billing.error}>
        {(error) => (
          <div class="pc-billing-error" role="alert">
            <span>{error()}</span>
            <button type="button" class="pc-error-dismiss" aria-label="Dismiss" onClick={clearBillingNotice}>
              <Icon name="close" size={12} />
            </button>
          </div>
        )}
      </Show>
      <Group title="Hosting" help="Hosting pays for the machines. Agents and GitHub use your own accounts, connected through Zuse or already signed in on your snapshot.">
        <Row
          title="Zuse Cloud"
          description={
            subscribed() === true
              ? `${plan()?.planName ? `${plan()!.planName} plan` : "Subscribed"}. Zuse runs and bills the machines.`
              : "No setup. Zuse runs and bills the machines. Choose a plan to start."
          }
          action={
            <Show
              when={subscribed() !== null}
              fallback={
                <>
                  <Badge tone="outline">{loading() ? "Loading…" : "Could not load billing"}</Badge>
                  <Show when={!loading()}>
                    <Action onClick={() => void loadBilling()}>Retry</Action>
                  </Show>
                </>
              }
            >
              <Show when={subscribed() === true}>
                <Badge>In use</Badge>
                <Action busy={cloud.billing.portalBusy} onClick={() => void openBilling()}>
                  Manage subscription
                </Action>
              </Show>
            </Show>
          }
        />
      </Group>
      <Show when={subscribed() === false}>
        <Plans />
      </Show>
    </div>
  );
}

/**
 * Cloud, Cloud Max and Enterprise side by side, with the $1 trial above them when the server offers it.
 * Before subscribing every action opens checkout; afterwards the current plan is marked and the others switch to it.
 */
function Plans() {
  const subscribed = () => cloud.billing.subscribed === true;
  const tier = () => (subscribed() ? (tierOf(plan()?.planId) ?? "cloud") : null);
  const trialing = () => !!plan()?.trial;
  const offer = (id: "cloud" | "cloud-max") => catalog().plans.find((p) => p.planId === id) ?? FALLBACK_CATALOG.plans.find((p) => p.planId === id)!;
  const showTrial = () => catalog().trialAvailable && !subscribed();
  const cloudAction = () => {
    if (!subscribed()) return <Checkout offer={CLOUD_OFFER} primary={!showTrial()} disabled={!offer("cloud").available}>Choose Cloud</Checkout>;
    if (trialing()) return <StartCloud />;
    if (tier() === "cloud-max") return <Checkout offer={CLOUD_OFFER}>Switch to Cloud</Checkout>;
    return null;
  };
  const maxAction = () => {
    if (!subscribed()) return <Checkout offer={CLOUD_MAX_OFFER} disabled={!offer("cloud-max").available}>Choose Cloud Max</Checkout>;
    if (tier() === "cloud-max") return null;
    return <UpgradeMax />;
  };
  return (
    <Group title={subscribed() ? "Change plan" : "Plans"} help={MACHINE_MINUTE_HELP}>
      <Show when={showTrial()}>
        <div class="pc-trial-offer">
          <div class="pc-trial-copy">
            <span class="pc-plan-kicker">Trial</span>
            <span class="pc-trial-text">{trialCopy(catalog())}</span>
            <span class="pc-row-description">Once per card and account. No pay as you go during the trial.</span>
          </div>
          <Checkout offer={CLOUD_TRIAL_OFFER} primary>
            Start trial · {dollars(catalog().trialPriceMicros)}
          </Checkout>
        </div>
      </Show>
      <div class="pc-plans">
        <PlanCard offer={offer("cloud")} current={tier() === "cloud" && !trialing()} action={cloudAction()} />
        <PlanCard offer={offer("cloud-max")} current={tier() === "cloud-max"} action={maxAction()} featured />
        <div class="pc-plan-card">
          <div class="pc-plan-card-head">
            <span class="pc-plan-kicker">Enterprise</span>
          </div>
          <div class="pc-plan-price">
            <strong>Custom</strong>
          </div>
          <ul class="pc-plan-points">
            <li>Machine minutes sized to your team</li>
            <li>Larger computers on request</li>
            <li>Invoicing and priority support</li>
          </ul>
          <div class="pc-plan-card-action">
            <Action onClick={() => openContact(catalog().enterpriseContactUrl)}>Contact us</Action>
          </div>
        </div>
      </div>
    </Group>
  );
}

function PlanCard(props: { offer: PlanOffer; current: boolean; action: JSX.Element; featured?: boolean }) {
  return (
    <div class="pc-plan-card" data-current={props.current ? "true" : undefined} data-featured={props.featured ? "true" : undefined}>
      <div class="pc-plan-card-head">
        <span class="pc-plan-kicker">{props.offer.name}</span>
        <Show when={props.current}>
          <Badge>Current plan</Badge>
        </Show>
      </div>
      <div class="pc-plan-price">
        <strong>{dollars(props.offer.priceMicros)}</strong>
        <span>/month</span>
      </div>
      <ul class="pc-plan-points">
        <li>{offerAllowance(props.offer)}</li>
        <li>{offerMachines(props.offer)}</li>
        <li>{props.offer.payAsYouGo ? "Then pay as you go at the same rate, up to a cap you set" : "Machine minutes reset each month"}</li>
      </ul>
      <div class="pc-plan-card-action">{props.action}</div>
    </div>
  );
}

export function Usage() {
  const summary = () => cloud.billing.summary;
  return (
    <div class="pc-stack">
      <Show when={cloud.billing.subscribed === true}>
        <Show
          when={summary()}
          fallback={
            <Group title="Usage and billing" help="Usage details are temporarily unavailable.">
              <Row
                title={cloud.billing.loading ? "Loading billing…" : "Could not load billing"}
                description="Refresh cloud settings to try again. Existing workspaces are unaffected."
                action={
                  <Action disabled={cloud.billing.loading} onClick={() => void loadBilling()}>
                    Retry
                  </Action>
                }
              />
            </Group>
          }
        >
          {(s) => (
            <Show when={s().plan} fallback={<DollarUsage summary={s()} />}>
              {(p) => (
                <>
                  <PlanOverview summary={s()} plan={p()} />
                  <UsageBreakdown summary={s()} plan={p()} />
                  <Plans />
                </>
              )}
            </Show>
          )}
        </Show>
      </Show>
      <Credits />
      <Show when={cloud.billing.subscribed === true && personal()}>
        <Activity />
      </Show>
    </div>
  );
}

/** The current plan: allowance, a large meter of machine minutes, renewal or trial, holds and the pay-as-you-go cap. */
function PlanOverview(props: { summary: BillingSummary; plan: PlanUsage }) {
  const p = () => props.plan;
  const hold = () => holdNotice(p());
  const paused = () => p().usageHold || props.summary.status === "billing-hold";
  const fraction = () => usedFraction(p());
  const tone = () => (paused() ? "var(--danger)" : fraction() >= 0.9 ? "var(--warning)" : "var(--accent)");
  const holdAction = (action: HoldAction) =>
    action === "start-cloud" ? (
      <StartCloud />
    ) : action === "upgrade-cloud-max" ? (
      <UpgradeMax />
    ) : (
      <Action primary onClick={raiseCap}>
        Raise cap
      </Action>
    );
  return (
    <Group
      title="Plan"
      help={MACHINE_MINUTE_HELP}
      action={<Badge tone={paused() ? "warn" : p().trial ? "outline" : "ok"}>{paused() ? "Paused" : p().trial ? "Trial" : "Active"}</Badge>}
    >
      <Show when={hold()}>
        {(notice) => (
          <div class="pc-image-alert" role="alert">
            <Icon name="danger-triangle" size={16} class="pc-image-alert-icon" />
            <div class="pc-row-copy">
              <div class="pc-row-title">{notice().message}</div>
            </div>
            <div class="pc-row-action">
              <For each={notice().actions}>{(action) => holdAction(action)}</For>
            </div>
          </div>
        )}
      </Show>
      <div class="pc-plan-hero">
        <div class="pc-plan-hero-head">
          <div class="pc-plan-hero-copy">
            <span class="pc-plan-hero-title">{planTitle(p())}</span>
            <span class="pc-row-description">{allowanceLine(p())}</span>
          </div>
          <Show when={p().trial} fallback={<span class="pc-chip pc-plan-renew">{renewLine(p())}</span>}>
            <Show when={!hold()}>
              <StartCloud />
            </Show>
          </Show>
        </div>
        <Show when={p().trial}>
          <div class="pc-plan-trial">
            <span class="pc-row-title">{trialLine(p())}</span>
            <span class="pc-row-description">Then Cloud at {dollars(catalog().plans.find((o) => o.planId === "cloud")?.priceMicros ?? 20_000_000)}/month. No pay as you go during the trial.</span>
          </div>
        </Show>
        <div class="pc-plan-meter">
          <div class="pc-plan-meter-figures" title={meterHint(p())}>
            <strong>{count(p().remainingMachineMinutes)}</strong>
            <span>machine minutes left</span>
          </div>
          <DitherMeter
            class="pc-plan-meter-bar"
            fraction={fraction()}
            tone={tone()}
            height={20}
            label="Machine minutes used this month"
            valueText={meterLine(p())}
            title={meterHint(p())}
          />
          <div class="pc-plan-meter-caption">
            <span title={meterHint(p())}>{meterLine(p())}</span>
            <span class="pc-muted">{meterHint(p())}</span>
          </div>
          <Show when={extraLine(p())}>{(line) => <div class="pc-plan-extra">{line()}</div>}</Show>
        </div>
      </div>
      <Show when={p().payAsYouGo}>
        <CapRow title="Monthly pay-as-you-go cap" description="After the included machine minutes, use is billed at the same rate. Cloud computers pause when it reaches this pre-tax limit." />
      </Show>
      <Show when={cloud.billing.notice}>{(notice) => <Note>{notice()}</Note>}</Show>
    </Group>
  );
}

/** Machine minutes this period by day, computer size and chat, from the loaded usage entries. */
function UsageBreakdown(props: { summary: BillingSummary; plan: PlanUsage }) {
  const items = () => cloud.billing.usage;
  const days = createMemo(() => byDay(items(), props.summary.periodStart, Math.min(Date.now(), props.summary.periodEnd)));
  const rate = () => props.plan.defaultMachineRate;
  const columns = createMemo(() =>
    days().map((day) => ({
      label: day.label,
      value: day.machineMinutes,
      title: `${day.label} · ${machineMinutesLabel(day.machineMinutes)} · ${usageHint(day.machineMinutes, rate())}`,
    })),
  );
  const titleOf = (workspaceId: string) => {
    const row = cloud.workspaces.rows.find((w) => w.workspaceId === workspaceId);
    return row?.title || row?.branch || null;
  };
  const sizes = createMemo(() => bySize(items()));
  const chats = createMemo(() => byChat(items(), titleOf));
  const used = () => days().some((day) => day.machineMinutes > 0);
  return (
    <Group title="Usage this month" help={MACHINE_MINUTE_HELP}>
      <Show
        when={used()}
        fallback={<Row title="No cloud computer use yet this month" description="Machine minutes appear here by day, computer and chat once a cloud computer has been awake." />}
      >
        <div class="pc-usage-chart">
          <DitherColumns
            label="Machine minutes by day"
            columns={columns()}
            idle={`${machineMinutesLabel(props.plan.usedMachineMinutes)} this month · ${usageHint(props.plan.usedMachineMinutes, rate())}`}
          />
        </div>
        <div class="pc-breakdown">
          <Breakdown title="By computer" buckets={sizes()} />
          <Breakdown title="By chat" buckets={chats()} />
        </div>
      </Show>
      <Show when={cloud.billing.cursor}>
        <div class="pc-row-tail pc-row-tail-split">
          <span class="pc-row-description">Showing the latest {count(items().length)} runs.</span>
          <Action disabled={cloud.billing.loading} onClick={() => void loadBilling(true)}>
            Load more
          </Action>
        </div>
      </Show>
    </Group>
  );
}

function Breakdown(props: { title: string; buckets: Bucket[] }) {
  const max = () => Math.max(1, ...props.buckets.map((b) => b.machineMinutes));
  return (
    <section class="pc-breakdown-list" aria-label={props.title}>
      <h4>{props.title}</h4>
      <Show when={props.buckets.length} fallback={<span class="pc-row-description">Nothing yet.</span>}>
        <For each={props.buckets.slice(0, 6)}>
          {(bucket) => (
            <DitherShare
              label={bucket.label}
              value={machineMinutesLabel(bucket.machineMinutes)}
              title={usageHint(bucket.machineMinutes, bucket.rate)}
              fraction={bucket.machineMinutes / max()}
            />
          )}
        </For>
        <Show when={props.buckets.length > 6}>
          <span class="pc-row-description">and {props.buckets.length - 6} more</span>
        </Show>
      </Show>
    </section>
  );
}

/** Periods billed before machine-minute plans keep the dollar view. */
function DollarUsage(props: { summary: BillingSummary }) {
  const s = () => props.summary;
  const percent = () => {
    if (s().overageCapMicros <= 0) return s().overageProviderCostMicros > 0 ? 100 : 0;
    return Math.max(0, Math.min(100, Math.trunc((s().overageChargeMicros * 100) / s().overageCapMicros)));
  };
  const included = () => (s().includedProviderCostMicros > 0 ? s().includedUsedMicros / s().includedProviderCostMicros : 0);
  return (
    <Group
      title="Usage and billing"
      help="Usage in this billing period. Builds and running workspaces pause when the overage cap is reached."
      action={<Badge tone={s().status === "billing-hold" ? "warn" : "ok"}>{s().status === "billing-hold" ? "Paused" : "Active"}</Badge>}
    >
      <Show when={percent() >= 50}>
        <Row
          title={percent() >= 100 ? "Billing hold" : "Usage warning"}
          description={
            percent() >= 100
              ? "The overage cap is exhausted. New billable operations are blocked and running compute is being paused."
              : `You have used ${percent()}% of this period's overage cap.`
          }
          action={<Badge tone="warn">{percent()}%</Badge>}
        />
      </Show>
      <Row
        title={`${usd(s().includedUsedMicros)} of ${usd(s().includedProviderCostMicros)} included used`}
        description={`${usd(s().includedRemainingMicros)} remaining · ${usd(s().overageChargeMicros)} overage · ${usd(s().currentInvoiceEstimateMicros)} invoice estimate before tax`}
      >
        <DitherMeter fraction={included()} height={8} label="Included usage this period" valueText={`${usd(s().includedUsedMicros)} of ${usd(s().includedProviderCostMicros)} used`} />
      </Row>
      <CapRow title="Monthly overage cap" description="Builds and running workspaces pause when this pre-tax limit is reached." />
      <Show when={cloud.billing.notice}>{(notice) => <Note>{notice()}</Note>}</Show>
      <RecentUsage />
    </Group>
  );
}

function CapRow(props: { title: string; description: string }) {
  return (
    <Row
      title={props.title}
      description={props.description}
      action={
        <form
          class="pc-cap"
          onSubmit={(event) => {
            event.preventDefault();
            void saveCap();
          }}
        >
          <span class="pc-cap-field">
            <span aria-hidden="true">$</span>
            <input
              id={CAP_INPUT}
              inputmode="decimal"
              aria-label="Monthly overage cap in dollars"
              value={cloud.billing.capInput}
              onInput={(event) => setCapInput(event.currentTarget.value)}
            />
          </span>
          <Action type="submit" primary busy={cloud.billing.capBusy} disabled={cloud.billing.loading}>
            Save
          </Action>
        </form>
      }
    />
  );
}

const usageLabel = (item: { usageKind?: string | null; resourceKind: string; resourceId: string }) => `${item.usageKind ?? item.resourceKind} ${item.resourceId}`;

/** Dollar periods: the three latest usage entries, then the rest on request. Entries never name the machines' infrastructure. */
function RecentUsage() {
  const [historyOpen, setHistoryOpen] = createSignal(false);
  const provisional = (item: BillingUsageItem) => (item.status === "provisional" ? " (provisional)" : "");
  const recent = (): JSX.Element =>
    cloud.billing.usage.length === 0
      ? "No completed cloud computer runs in this billing period."
      : cloud.billing.usage
          .slice(0, 3)
          .map((item) => `${usageLabel(item)}: ${usd(item.providerCostMicros)}${provisional(item)}`)
          .join(" · ");
  return (
    <>
      <Row title="Recent usage" description={recent()} />
      <Show when={cloud.billing.usage.length > 3 || cloud.billing.cursor}>
        <div class="pc-row-tail">
          <Action aria-expanded={historyOpen() ? "true" : "false"} onClick={() => setHistoryOpen(!historyOpen())}>
            {historyOpen() ? "Hide history" : "View history"}
          </Action>
        </div>
      </Show>
      <Show when={historyOpen()}>
        <For each={cloud.billing.usage.slice(3)}>{(item) => <Row title={usageLabel(item)} description={`${usd(item.providerCostMicros)} · ${item.status}`} />}</For>
        <Show when={cloud.billing.cursor}>
          <div class="pc-row-tail">
            <Action disabled={cloud.billing.loading} onClick={() => void loadBilling(true)}>
              Load more
            </Action>
          </div>
        </Show>
      </Show>
    </>
  );
}

function Credits() {
  const enabled = () => !!cloud.billing.credits?.available && !cloud.billing.creditsBusy;
  const move = (event: KeyboardEvent, index: number) => {
    const next =
      event.key === "ArrowLeft" || event.key === "ArrowUp"
        ? (index + 3) % 4
        : event.key === "ArrowRight" || event.key === "ArrowDown"
          ? (index + 1) % 4
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? 3
              : -1;
    if (next < 0 || !enabled()) return;
    event.preventDefault();
    selectCredit(CREDIT_AMOUNTS[next]!);
    ((event.currentTarget as HTMLElement).parentElement?.children[next] as HTMLElement | undefined)?.focus();
  };
  return (
    <Group title="Billing">
      <div class="pc-credits">
        <div class="pc-credits-head">
          <div class="pc-credits-title">
            <span class="pc-row-title">Zuse credits</span>
            <span class="pc-chip">{cloud.billing.credits ? `${cents(cloud.billing.credits.creditCents)} available` : "Balance unavailable"}</span>
          </div>
          <Action busy={cloud.billing.creditsBusy} onClick={() => void loadCredits(false)}>
            Refresh balance
          </Action>
        </div>
        <Show when={(cloud.billing.credits?.debitCents ?? 0) > 0}>
          <p class="pc-muted">{cents(cloud.billing.credits!.debitCents)} owed on future invoices</p>
        </Show>
        <div class="pc-credits-buy">
          <div class="pc-credits-amounts">
            <span class="pc-muted" id="pc-credit-label">
              Credit amount
            </span>
            <div role="radiogroup" aria-labelledby="pc-credit-label" class="pc-radios">
              <For keyed={false} each={CREDIT_AMOUNTS}>
                {(amount, index) => (
                  <button
                    type="button"
                    role="radio"
                    class="pc-radio"
                    aria-checked={cloud.billing.creditAmount === amount() ? "true" : "false"}
                    tabindex={enabled() && cloud.billing.creditAmount === amount() ? 0 : -1}
                    disabled={!enabled()}
                    onClick={() => selectCredit(amount())}
                    onKeyDown={(event) => move(event, index)}
                  >
                    {cents(amount())}
                  </button>
                )}
              </For>
            </div>
          </div>
          <Action primary disabled={!enabled()} busy={cloud.billing.creditsBusy} onClick={() => void loadCredits(true)}>
            Buy credits
          </Action>
        </div>
        <p class="pc-muted pc-relaxed">
          Applies automatically to future Zuse invoices, including subscriptions. Unused credit rolls forward. Your machine minutes and spending cap stay the same.
        </p>
        <Show when={cloud.billing.credits && !cloud.billing.credits.available}>
          <p class="pc-muted">Credit purchases are not enabled for this workspace yet.</p>
        </Show>
        <Show when={cloud.billing.creditError}>{(error) => <Note error>{error()}</Note>}</Show>
      </div>
      <Row
        title="Invoices and payment method"
        description="Invoices, receipts, payment details and cancellation are in the billing portal."
        action={
          <Action busy={cloud.billing.portalBusy} onClick={() => void openBilling()}>
            Open billing portal
          </Action>
        }
      />
    </Group>
  );
}

function Activity() {
  return (
    <Group title="Workspace activity" help="Current and recent cloud workspaces for this account." action={<Badge tone="outline">{cloud.workspaces.rows.length}</Badge>}>
      <Show when={cloud.workspaces.rows.length > 0} fallback={<Row title="No cloud workspaces yet" description="Start a cloud chat and its workspace will appear here." />}>
        <For each={cloud.workspaces.rows}>
          {(row) => (
            <Row
              title={row.branch || "Workspace"}
              description={row.statusCode}
              action={<Badge tone={row.state === "deleted" ? "outline" : row.state === "ready" ? "ok" : "warn"}>{row.state || "unknown"}</Badge>}
            />
          )}
        </For>
      </Show>
    </Group>
  );
}
