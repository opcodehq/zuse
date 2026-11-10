# Embedded billing operations

Zuse embeds Brokeboy at `1e35dc7925f7b4df49c05283f687f51a77886e8a`. Stripe remains the payment processor. Enable the same account list and environment in the cloud API and `billing-worker`. The default remains the existing backend until an operator enrolls an account.

The implementation includes subscription checkout and portal operations, signed webhook intake, customer/subscription adoption, host entitlement and compute-period projection, a prepaid hosted-model wallet, durable model reservations/settlement/release/increases, maintenance and account-scoped operation recovery. Financial work retains the host's billing ownership fence.

Compute observations, ledger, spending-cap edits, storage settlement and original meter outboxes remain in Zuse. Admission and settlement use Brokeboy’s shared integer cost policy inside the existing host transactions, so mutable cap edits remain atomic without introducing a second period authority or a new startup round trip. This preserves the current allowance/markup/cap policy and historical delivery keys. Brokeboy does not independently export those compute events. Polar subscriptions can be retained through the native adapter, but its metering/reconciliation lane remains the existing host implementation; prepaid checkout currently requires Stripe.

## Install schema and publish pricing

Use the existing private-schema database workflow. Review `crates/cloud/migrations/0032_brokeboy.sql`, then run against the intended environment:

```sh
python3 infra/cloud/database.py --environment staging apply-brokeboy
python3 infra/cloud/database.py --environment staging check
```

This optional migration verifies the base ledger and environment, uses the same schema lock as bootstrap, and records its checksum transactionally. Startup never applies DDL. Brokeboy opens an independent SQLx 0.9 pool (four connections); the host remains on SQLx 0.8. Budget API and worker pools separately. The connection URL must select the same private schema for both versions, for example with URL-encoded `options=-csearch_path=zuse_staging`.

Configure secrets through the existing deployment environment, without copying values into the repository:

```sh
export ZUSE_BROKEBOY_ACCOUNTS='user-id'
export ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS='user-id'
export BROKEBOY_ENVIRONMENT=test
export BILLING_DEFAULT_PROVIDER=stripe
export API_ISSUER=https://api-rust-stag.zuse.sh
```

Keep existing `ZUSE_CLOUD_DATABASE_URL`, `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, the approved `STRIPE_PRICE_CLOUD_WORKSPACE_STANDARD_V1`, `STRIPE_CLOUD_OVERAGE_PRICE_ID`, `STRIPE_CLOUD_OVERAGE_METER_ID`, export ownership/configuration and signing key. Stripe test keys require `test`; live keys require `live`. `MACHINE_LIVE_CHECKOUT_ENABLED=true` and `RELAY_MINT_PRIVATE_JWK` enable signed checkout, including in the sandbox. With checkout disabled, balance reads remain available but purchases are unavailable.

Copy `infra/cloud/brokeboy.catalog.example.json` into deployment configuration and replace both placeholder price IDs with the existing approved prices. The cloud plan must retain the current USD 40/month terms; the price and overage price must match the host's configuration. In explicit `STRIPE_CLOUD_BILLING_MODE=subscription-only` mode, omit the meter mapping and overage item. Do not publish a second independent meter for the same compute charges.

```sh
cargo run -p zuse-cloud-api -- billing-catalog \
  --catalog /path/to/reviewed-catalog.json --expected-revision 0
```

Later publication uses the next reviewed expected revision. It creates immutable plan versions; startup never republishes prices. Model wallet amounts are exact six-place USD decimals. No included model allowance is invented by the example catalog.

## Enroll and serve

The account must already have explicit `billing` ownership set to `rust` by the existing ownership workflow. New empty accounts can be enrolled directly:

```sh
cargo run -p zuse-cloud-api -- billing-enroll --owner user-id
```

For an existing account, inspect its current subscriptions and financial operations, fence/drain older API workers, then use:

```sh
cargo run -p zuse-cloud-api -- billing-enroll --owner user-id --adopt-existing
```

Adoption retains Stripe customer and subscription IDs. It does not recreate subscriptions, charge the customer, or transfer invoice credit. An interrupted enrollment remains `initializing`, preventing checkout from falling back to the old writer. Repeat enrollment to resume. Active enrollment is idempotent. Accounts with active subscriptions across multiple processors require reconciliation before enrollment.

Run the API and the existing `billing-worker` with matching configuration. The worker durably processes notifications, retries recoverable operations, projects canonical subscriptions, and recovers model billing requests. It runs bounded native maintenance under each account's existing ownership fence. Allow up to one worker polling interval for payment confirmation to appear in Zuse. Configure the Stripe endpoint as `/v1/billing/webhook/stripe`; the original raw signed bytes are required. The same route selects the correct financial handler, while historical invoice-credit purchases keep their original adjustment handling.

The enrollment marker is durable. Removing the account allowlist is not a rollback: startup rejects legacy financial writing for an enrolled account. To disable activity, stop the writer or hand off its existing billing ownership fence. Reverting the binary requires a reviewed account handoff that drains pending work and preserves receipts; an older binary does not understand the new marker.

For Polar, configure its token, signing secret and `POLAR_PRODUCT_CLOUD_WORKSPACE_STANDARD_V1`. `POLAR_LEGACY_LITERAL_SECRET=true` is required for older endpoints signing with literal secret bytes. Native Polar merchant flows still need qualification against the actual account.

## Wallet and model calls

`GET /v1/billing/prepaid` returns `funding: "hosted-models"` for enrolled accounts. The UI labels the model wallet separately and displays any retained invoice credit independently. Model wallet credit does not pay compute/subscription invoices. Existing invoice credit stays at Stripe and applies to those invoices as before. There is no automatic balance copying or wallet-to-invoice transfer.

Existing checkout endpoints accept optional `idempotencyKey`; the desktop supplies one. Reuse it after a lost reply, and use a new key for a deliberate new purchase. Saved checkout payloads preserve signed return URLs across retries. A created checkout is pending payment until canonical paid state is observed; unsigned redirects never grant credit.

Zuse-funded inference integrations use the trusted server methods on `Store`:

1. `reserve_hosted_model(owner, ModelReservation, now)` before calling a paid provider. The request identifies the chat, model and an estimated USD cost in micros.
2. `increase_hosted_model(owner, request_id, total_micros)` before spending beyond the funded hold.
3. `settle_hosted_model(owner, request_id, actual_micros)` after receiving authoritative usage, including partial work on cancellation.
4. `release_hosted_model` only if no billable work occurred.

The one-hour reservation expiry settles an abandoned request at its funded estimate. Late or uncovered actual costs become explicit reconciliation work rather than a second debit. Stable host request records recover a committed native reservation/debit if the host ACK is lost. Insufficient initial funding refuses admission. Never call these methods for local or BYO provider subscriptions, and never derive actual cost from client-provided totals. This change supplies billing integration points; it does not add a new model inference transport or provider credentials.

`GET /v1/billing/models/usage?limit=50&cursor=...` exposes paginated account-scoped model/chat/request attribution to authorized billing readers. Model usage remains separate from the compute summary and cap.

## Inspect and recover

```sh
cargo run -p zuse-cloud-api -- billing-operation \
  --owner user-id --operation payment:operation-key
cargo run -p zuse-cloud-api -- billing-operation \
  --owner user-id --operation payment:operation-key --retry
cargo run -p zuse-cloud-api -- billing-worker --once
```

Inspect status/result/error. `pending`, `processing`, `failed` and `reconciliation` are not payment success. Retry retains the native provider identity and external intent. Do not substitute a fresh key to bypass an ambiguous payment. Reconciliation requiring authoritative external evidence remains an operator action; the CLI does not guess a financial outcome.

## Verification

The native tests use a temporary PostgreSQL schema and a loopback Stripe HTTP fixture. They do not read deployment credentials or make live payments:

```sh
ZUSE_CLOUD_TEST_DATABASE_URL=postgresql://user@localhost/disposable_database \
  cargo test -p zuse-cloud --lib billing:: -- --include-ignored
python3 infra/cloud/tests/database.py
cargo check -p zuse-cloud-api
```

Before enabling a merchant account, repeat checkout, signed public delivery, renewal, refunds/disputes, existing-customer adoption and restore/replay against that account's Stripe sandbox. Upstream merchant qualification does not establish that this host's deployment is configured correctly. No account is enrolled or balance transferred merely by installing this code.
