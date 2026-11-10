# Brokeboy integration assessment

Reviewed 2026-11-09 against Brokeboy commit [`1e35dc7925f7b4df49c05283f687f51a77886e8a`](https://github.com/opcodehq/brokeboy/commit/1e35dc7925f7b4df49c05283f687f51a77886e8a) and Zuse `484d2c97`. This assessment preceded implementation. The embedded integration is now implemented in `crates/cloud/src/billing/native`; deployment, account enrollment and merchant qualification remain explicit operator work. See [operations](brokeboy-operations.md) for the implemented boundary and commands.

## Recommendation

Embed Brokeboy's native Rust engine inside the cloud API and use it for subscriptions, credit wallets, reservations and durable payment operations. Keep Zuse responsible for authenticating accounts, attributing usage to computers/chats, obtaining provider costs and displaying usage. Keep payment processing behind Brokeboy's Stripe adapter initially; retain the current Polar lane until merchant qualification passes. No JavaScript sidecar is needed: the Rust SDK executes its own engine. This replaces our direct payment orchestration, while Stripe still collects payments. Brokeboy is an MIT library using our database and processor accounts, not a hosted processor. Sources: [README](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/README.md), [native architecture](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/rust-port.md).

Brokeboy already audited this application's billing implementation and added several primitives specifically needed by it: cost periods, prepaid transfers, customer adoption, partial refunds/disputes, Polar routing and abandoned-stream settlement. Its [embedding guide](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/embedding.md) is the integration starting point. That work did not modify or qualify the actual Zuse integration.

## Ownership and product mapping

| Area | Suggested owner and behavior |
| --- | --- |
| Plans and payment operations | Brokeboy catalog, subscriptions, checkout, portal and durable operations; preserve processor/customer identity. |
| Included compute and overage caps | Evaluate Brokeboy `periods` using our existing integer USD micros, included provider cost and markup policy; retain Zuse's observation and settlement evidence. |
| Hosted-model credits | Separate model-credit feature and funding policy initially. Reserve an estimate before a call, increase before spending beyond it, settle actuals, release on cancellation. |
| BYO local/cloud agent subscriptions | Continue showing observed token/cost information separately. Do not debit Zuse credits for usage billed by the user's provider. |
| Compute/model wallet sharing | Make pooling an explicit product decision later; separate features prevent accidentally consuming one allowance with another. |
| Usage breakdown | Zuse keeps computer, chat, model, execution and provider dimensions. Translate receipts into existing desktop/API view models. |
| Machine and snapshot lifecycle | Zuse keeps lifecycle fences, ownership, BYO-connection exclusions, price evidence, storage leases and fractional-cost checkpoints. |

These are proposed Zuse policies, not existing plan settings. The library supplies [feature/pool and reservation APIs](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/crates/brokeboy/src/api.rs), [period actions](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/crates/brokeboy/src/engine/periods.rs) and an [example streaming lifecycle](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/crates/brokeboy/examples/streaming.rs). Reservations with `onExpiry: "settle_estimate"` require native maintenance; normal reservations release on expiry. Persist source-derived idempotency keys and operation IDs in host job state.

## Integration seams in this repository

Adding another `PortalProvider` alone does not replace this billing system. Brokeboy is the engine above processor adapters, so `stripe` and `polar` remain processor identities rather than being relabeled `brokeboy`.

| Zuse code | Required integration work |
| --- | --- |
| `crates/cloud/src/billing/catalog.rs` | Add an embedded-engine handle and explicit account rollout/ownership routing; retain writer fencing. Map summary, cap and usage reads to existing wire contracts. |
| `crates/cloud/src/billing/payments.rs` | Replace checkout/portal operation orchestration for migrated accounts with native SDK calls. Inspect durable operation status/result/error before returning a URL or marking completion. |
| `crates/cloud/src/billing/payments/checkout.rs` | Preserve approved offers, organization restrictions, signed completion tickets and redirect policy. Existing Stripe customer recovery and metadata expectations need adoption, not duplicate customer creation. |
| `crates/cloud/src/billing/payments/events.rs` | Route raw signed notifications to exactly one financial writer per account/purchase; reconcile authoritative subscription state into application access policy. |
| `crates/cloud/src/billing/payments/credits.rs` | Replace Stripe invoice-balance reads and adjustments only after choosing the wallet funding policy below. Preserve old purchase/refund/dispute provenance. The existing credit-event path explicitly requires Stripe. |
| `crates/cloud/src/billing.rs` | Current capacity checks run inside application SQLx transactions. Brokeboy's advisory `periods.check` is insufficient for admission; use `periods.reserve` with durable host admission state and compensation/recovery. |
| `crates/cloud/src/billing/settlement.rs`, `storage.rs` | Retain complete report retrieval, period splitting, resource/configuration fences and fractional checkpoints. Forward accepted monetary facts through stable keys; avoid settling twice. |
| `crates/cloud/src/billing/exports.rs`, `delivery.rs` | Preserve existing export identities/receipts and cumulative-cent correction checkpoints. Migrate export ownership deliberately; do not send an already acknowledged host outbox item under a new operation key. |
| `crates/cloud/src/routes.rs`, `crates/protocol/src/cloud_billing.rs` | Keep application account authorization and existing desktop responses; adapt native results rather than exposing arbitrary library customer/admin inputs. |

Hardcoded assumptions extend beyond the provider trait: settlement accepts only Stripe/Polar; export configuration registers those two processors; Stripe customer mappings are queried in checkout, event, portal and export paths. These need deliberate account-routing changes during engine adoption. They are not evidence that a third payment processor is needed.

One specific behavior gap needs resolution before period migration: Zuse's `BillingControl::set_cap` edits the active period cap idempotently, refuses caps below already incurred charges and carries the cap into renewals. At the reviewed revision Brokeboy's period dispatcher offers create/get/check/status/reserve/settle/release, with immutable period terms and no cap-edit action. Retain the host policy initially or add an audited, compatible upstream operation and tests; do not silently drop the existing spending-cap UI or mutate Brokeboy tables directly. Source: [period implementation](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/crates/brokeboy/src/engine/periods.rs).

## Database and dependency integration

Use a reviewed commit, not an invented release tag:

```toml
brokeboy = { git = "https://github.com/opcodehq/brokeboy", rev = "1e35dc7925f7b4df49c05283f687f51a77886e8a", default-features = false, features = ["postgres", "stripe"] }
```

This is a proposed dependency, not a build-verified change. Add `polar` only when integrating that adapter; `http` is optional because Zuse already owns Axum routes. Brokeboy requires Rust 1.94 and uses SQLx 0.9/Reqwest 0.13; this host uses SQLx 0.8/Reqwest 0.12. Open an independent Brokeboy pool with `Billing::open(DatabaseConnection, BillingOptions)` and explicit schema migrations. A host SQLx 0.8 pool cannot be passed to SQLx 0.9. Cargo may retain both versions; actual host dependency resolution/build still needs verification. Sources: [manifest](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/Cargo.toml), [SQL connection entry point](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/crates/brokeboy/src/sql.rs), [embedding guidance](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/embedding.md).

The separate pool means no shared atomic host/billing transaction. Commit a host outbox request, invoke billing with its stable source key, then acknowledge its durable receipt. Recover an interrupted admission or payment from that persisted state. Do not keep a host transaction open while another pool waits for overlapping locks. Run bounded native maintenance from the existing worker/scheduler. Initialization does not apply migrations or create processor resources. Source: [embedding protocol](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/embedding.md).

## Wallet policy that must stay explicit

Today our prepaid balance is Stripe customer **invoice credit**. Brokeboy prepaid checkout funds a **local wallet**. That wallet does not automatically pay Stripe subscription or overage invoices. Likewise `periods.settle` calculates a cost receipt; it does not silently debit a wallet or issue an invoice. Charging both paths would double bill. Source: [prepaid and period semantics](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/embedding.md).

For the first implementation, retain existing compute invoice funding and use a separate local wallet for new Zuse-hosted model usage. If later consolidating wallets, explicitly design invoice funding/transfers first. Existing Stripe credit must be transferred, never copied: fence writers/collection, inspect and drain uncertain work, adopt IDs, dry-run `migration.importStripeBalance`, debit the source through its recoverable operation, verify zero source balance and one local grant, then enable the new writer. Preserve the legacy path for historical refunds/disputes unless their provenance is imported. Source: [transfer and recovery procedure](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/embedding.md).

## Proposed rollout and acceptance criteria

1. **Compile and isolate.** Add the pinned native dependency behind a feature flag; migrate a dedicated staging schema; open the independent pool. Verify shutdown leaves the host pool alive. No production cutover is implied.
2. **Integrate sandbox commerce.** Create account-scoped customers/catalog; adapt checkout, portal, signed webhooks and operation recovery through existing authorization. Test cross-account access, response loss, duplicate/reordered notifications, delayed payment and renewal.
3. **Introduce model-credit accounting.** Connect trusted hosted-model receipts to reserve/increase/settle/release with model/chat/request dimensions, cancellation and abandoned-estimate recovery. Keep BYO usage nonbillable. Confirm one debit after crash/replay.
4. **Shadow compute policy.** Replay accepted resource facts into a separate noncharging comparison environment and compare period/cap/markup/rounding/correction outcomes. Resolve active-period cap edits and admission consistency before selecting Brokeboy as the writer. Retain resource attribution and provider reconciliation.
5. **Cut over per account.** Rehearse adoption, opening balances, draining reservations and export ownership; fence old writers. Demonstrate restore/replay without duplicate charges. Keep pre-cutover receipts accessible. Remove obsolete direct integrations only after the replacement covers their behavior.

## Readiness and evidence limits

The project remains alpha and its crates are `publish = false`. Native SDK, Stripe adapter, periods and wallet operations are implemented. The dated 2026-11-08 qualification records passing PostgreSQL 16/MySQL 8 source contracts and real Stripe sandbox browser checkout, registered public webhook delivery, 3DS, ACH success, partial/failed refunds, disputes, opening-balance transfer, metered invoice and top-up/renewal exercises. It would be inaccurate to describe all of those as fixture-only. Sources: [native qualification](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/rust-port.md#embedding-qualification-2026-11-08), [verification policy](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/verification.md).

Those are upstream recorded results, not tests executed against Zuse during this assessment. The actual host build/cutover and original host integration suites remain unverified. Polar has fixture qualification but no real merchant flow, deduplication or meter-summary qualification. Broader financial-commit response-loss/expired-idempotency matrices, additional schedule/tax/discount cases and updated distribution/performance evidence remain open. Brokeboy therefore fits a staged integration, not an immediate wholesale live replacement. Sources: [remaining work](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/rust-port.md#remaining-integration-and-release-work), [host boundary](https://github.com/opcodehq/brokeboy/blob/1e35dc7925f7b4df49c05283f687f51a77886e8a/internal-docs/embedding.md#what-the-host-application-must-keep).
