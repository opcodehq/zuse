// Hosting, usage and billing (crates/viewport/src/cloud_billing_view.rs).
import { createSignal, For, Index, Show } from "solid-js";
import { owner } from "../account";
import { Icon } from "@/ui/Icon";
import { Action, Badge, Group, Note, Row } from "../ui";
import { cents, CREDIT_AMOUNTS, usd } from "./money";
import { cloud, clearBillingNotice, loadBilling, loadCredits, openBilling, saveCap, selectCredit, setCapInput } from "./state";

const personal = () => !owner()?.organizationId;

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
          description={subscribed() === true ? "Subscribed. Zuse runs and bills the machines." : "No setup. Zuse runs and bills the machines."}
          action={
            <>
              <Show when={subscribed() === true}>
                <Badge>In use</Badge>
              </Show>
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
                <Action primary={subscribed() === false} busy={cloud.billing.portalBusy} onClick={() => void openBilling(false)}>
                  {subscribed() ? "Manage subscription" : "Subscribe · $40/month"}
                </Action>
              </Show>
            </>
          }
        />
      </Group>
    </div>
  );
}

export function Usage() {
  const [historyOpen, setHistoryOpen] = createSignal(false);
  const summary = () => cloud.billing.summary;
  const percent = () => {
    const s = summary();
    if (!s) return 0;
    if (s.overageCapMicros <= 0) return s.overageProviderCostMicros > 0 ? 100 : 0;
    return Math.max(0, Math.min(100, Math.trunc((s.overageChargeMicros * 100) / s.overageCapMicros)));
  };
  const usageLabel = (item: { usageKind?: string | null; resourceKind: string; resourceId: string }) => `${item.usageKind ?? item.resourceKind} ${item.resourceId}`;
  const recent = () =>
    cloud.billing.usage.length === 0
      ? "No completed sandbox runs in this billing period."
      : cloud.billing.usage
          .slice(0, 3)
          .map((item) => `${usageLabel(item)}: ${usd(item.providerCostMicros)}${item.status === "provisional" ? " (provisional)" : ""}`)
          .join(" · ");
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
            <Group
              title="Usage and billing"
              help="$40/month includes $35 of sandbox compute and image storage. Additional usage has a 5% markup."
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
              />
              <Row
                title="Monthly overage cap"
                description="Builds and running workspaces pause when this pre-tax limit is reached."
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
                        inputMode="decimal"
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
              <Show when={cloud.billing.notice}>{(notice) => <Note>{notice()}</Note>}</Show>
              <Row title="Recent usage" description={recent()} />
              <Show when={cloud.billing.usage.length > 3 || cloud.billing.cursor}>
                <div class="pc-row-tail">
                  <Action aria-expanded={historyOpen()} onClick={() => setHistoryOpen(!historyOpen())}>
                    {historyOpen() ? "Hide history" : "View history"}
                  </Action>
                </div>
              </Show>
              <Show when={historyOpen()}>
                <For each={cloud.billing.usage.slice(3)}>
                  {(item) => <Row title={usageLabel(item)} description={`${usd(item.providerCostMicros)} · ${item.provider} · ${item.status}`} />}
                </For>
                <Show when={cloud.billing.cursor}>
                  <div class="pc-row-tail">
                    <Action disabled={cloud.billing.loading} onClick={() => void loadBilling(true)}>
                      Load more
                    </Action>
                  </div>
                </Show>
              </Show>
            </Group>
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
              <Index each={CREDIT_AMOUNTS}>
                {(amount, index) => (
                  <button
                    type="button"
                    role="radio"
                    class="pc-radio"
                    aria-checked={cloud.billing.creditAmount === amount()}
                    tabIndex={enabled() && cloud.billing.creditAmount === amount() ? 0 : -1}
                    disabled={!enabled()}
                    onClick={() => selectCredit(amount())}
                    onKeyDown={(event) => move(event, index)}
                  >
                    {cents(amount())}
                  </button>
                )}
              </Index>
            </div>
          </div>
          <Action primary disabled={!enabled()} busy={cloud.billing.creditsBusy} onClick={() => void loadCredits(true)}>
            Buy credits
          </Action>
        </div>
        <p class="pc-muted pc-relaxed">
          Applies automatically to future Zuse invoices, including subscriptions. Unused credit rolls forward. Your usage allowance and spending cap stay the same.
        </p>
        <Show when={cloud.billing.credits && !cloud.billing.credits.available}>
          <p class="pc-muted">Credit purchases are not enabled for this workspace yet.</p>
        </Show>
        <Show when={cloud.billing.creditError}>{(error) => <Note error>{error()}</Note>}</Show>
      </div>
      <Row
        title="Manage subscription"
        description="Manage or cancel your Zuse subscription, payment details, and invoices in the billing portal."
        action={
          <Action busy={cloud.billing.portalBusy} onClick={() => void openBilling(true)}>
            Manage subscription
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
