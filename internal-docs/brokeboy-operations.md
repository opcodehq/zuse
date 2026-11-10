# Embedded billing operations

Zuse embeds Brokeboy at `1e35dc7925f7b4df49c05283f687f51a77886e8a`. Stripe remains the payment processor. Enable the same account list and environment in the cloud API and `billing-worker`. The default remains the existing backend until an operator enrolls an account.

The implementation includes subscription checkout and portal operations, signed webhook intake, customer/subscription adoption, host entitlement and compute-period projection, one shared compute/model credit wallet, durable model reservations/settlement/release/increases, maintenance and account-scoped operation recovery. Financial work retains the host's billing ownership fence.

The $40/month plan grants **$40 in shared USD credits each paid month**. Compute, image storage and Zuse-funded models debit the same `zuse-usd` wallet; top-ups fund that wallet too. Compute retains the existing 5% rate markup. BYO provider usage is excluded. Monthly subscription grants expire at the end of their period; purchased credits retain their purchase policy.

Zuse retains canonical provider observations, resource attribution, storage checkpoints and the ownership fence. Confirmed retail compute costs enqueue durable wallet charges in the same host transaction. The worker consumes those charges with stable native idempotency keys; shared periods never create Stripe overage exports. Legacy accounts keep the original invoice/cap lane. Subscription invoices still collect $40 through Stripe; the credit grant funds usage and does not pay the subscription invoice itself.

Admission checks the projected shared-credit hold locally. Native model reservations and the maintenance worker refresh that hold. Exhaustion blocks new paid operations and requests a safe retained pause for supported online computers through the existing lifecycle worker. Compute is charged from confirmed observations, so delayed provider reporting can create outstanding liabilities; this is not a real-time prepaid guarantee. Pending compute reduces available credit and takes priority over new model reservations. Closed-period observations are held for reconciliation instead of silently consuming the next month's grant.

## Install schema and publish pricing

Use the existing private-schema database workflow. Review `crates/cloud/migrations/0032_brokeboy.sql` and `0033_shared_credits.sql`, then run against the intended environment:

```sh
python3 infra/cloud/database.py --environment staging apply-brokeboy
python3 infra/cloud/database.py --environment staging check
```

The optional migrations verify the base ledger and environment, uses the same schema lock as bootstrap, and records its checksum transactionally. Startup never applies DDL. Brokeboy opens an independent SQLx 0.9 pool (four connections); the host remains on SQLx 0.8. Budget API and worker pools separately. The connection URL must select the same private schema for both versions, for example with URL-encoded `options=-csearch_path=zuse_staging`.

Configure secrets through the existing deployment environment, without copying values into the repository:

```sh
export ZUSE_BROKEBOY_ACCOUNTS='user-id'
export ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS='user-id'
export BROKEBOY_ENVIRONMENT=test
export BILLING_DEFAULT_PROVIDER=stripe
export API_ISSUER=https://api-rust-stag.zuse.sh
```

Keep existing `ZUSE_CLOUD_DATABASE_URL`, `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, the approved `STRIPE_PRICE_CLOUD_WORKSPACE_STANDARD_V1` and signing key. Keep meter/export configuration for legacy accounts only. Stripe test keys require `test`; live keys require `live`. `MACHINE_LIVE_CHECKOUT_ENABLED=true` and `RELAY_MINT_PRIVATE_JWK` enable signed checkout, including in the sandbox. With checkout disabled, balance reads remain available but purchases are unavailable.

Copy `infra/cloud/brokeboy.catalog.example.json` into deployment configuration and replace the placeholder subscription price ID with the approved USD 40/month price. The plan must include exactly 40 USD of `zuse-usd` credits resetting monthly. Validation rejects native metered prices and overage items, preventing duplicate compute collection.

```sh
cargo run -p zuse-cloud-api -- billing-catalog \
  --catalog /path/to/reviewed-catalog.json --expected-revision 0
```

Later publication uses the next reviewed expected revision. It creates immutable plan versions; startup never republishes prices. Shared wallet amounts are exact six-place USD decimals. The example catalog explicitly grants the $40 monthly allowance.

## Enroll and serve

The account must already have explicit `billing` ownership set to `rust` by the existing ownership workflow. New empty accounts can be enrolled directly:

```sh
cargo run -p zuse-cloud-api -- billing-enroll --owner user-id
```

For an existing account, inspect its current subscriptions and financial operations, fence/drain older API workers, then use:

```sh
cargo run -p zuse-cloud-api -- billing-enroll --owner user-id --adopt-existing
```

Adoption retains Stripe customer and subscription IDs. It does not recreate subscriptions, charge the customer, or transfer invoice credit. An interrupted enrollment remains `initializing`, preventing checkout from falling back to the old writer. Repeat enrollment to resume. Active enrollment is idempotent. Adoption rejects a current period with recorded usage: reconcile its consumed allowance, pending reservations and exports before cutover. This is not an automatic mid-period balance migration. Existing native subscriptions or balances using the earlier model-only feature also need a reviewed cutover; publishing a new catalog does not rewrite immutable subscription versions. Accounts with active subscriptions across multiple processors require reconciliation before enrollment.

Run the API and the existing `billing-worker` with matching configuration. The worker durably processes notifications, retries recoverable operations, projects canonical subscriptions, and recovers model billing requests. It runs bounded native maintenance under each account's existing ownership fence. Allow up to one worker polling interval for payment confirmation to appear in Zuse. Configure the Stripe endpoint as `/v1/billing/webhook/stripe`; the original raw signed bytes are required. The same route selects the correct financial handler, while historical invoice-credit purchases keep their original adjustment handling.

The enrollment marker is durable. Removing the account allowlist is not a rollback: startup rejects legacy financial writing for an enrolled account. To disable activity, stop the writer or hand off its existing billing ownership fence. Reverting the binary requires a reviewed account handoff that drains pending work and preserves receipts; an older binary does not understand the new marker.

For Polar, configure its token, signing secret and `POLAR_PRODUCT_CLOUD_WORKSPACE_STANDARD_V1`. `POLAR_LEGACY_LITERAL_SECRET=true` is required for older endpoints signing with literal secret bytes. Native Polar merchant flows still need qualification against the actual account.

## Wallet and model calls

`GET /v1/billing/prepaid` returns `funding: "shared-credits"` for enrolled accounts. Workspace billing presents the combined balance and compute/model usage attribution. Retained legacy Stripe invoice credit stays at Stripe and applies to invoices; it is never copied into the shared wallet. There is no automatic wallet-to-invoice transfer.

Existing checkout endpoints accept optional `idempotencyKey`; the desktop supplies one. Reuse it after a lost reply, and use a new key for a deliberate new purchase. Saved checkout payloads preserve signed return URLs across retries. A created checkout is pending payment until canonical paid state is observed; unsigned redirects never grant credit.

Zuse-funded inference integrations use the trusted server methods on `Store`:

1. `reserve_hosted_model(owner, ModelReservation, now)` before calling a paid provider. The request identifies the chat, model and an estimated USD cost in micros.
2. `increase_hosted_model(owner, request_id, total_micros)` before spending beyond the funded hold.
3. `settle_hosted_model(owner, request_id, actual_micros)` after receiving authoritative usage, including partial work on cancellation.
4. `release_hosted_model` only if no billable work occurred.

The one-hour reservation expiry settles an abandoned request at its funded estimate. Late or uncovered actual costs become explicit reconciliation work rather than a second debit. Stable host request records recover a committed native reservation/debit if the host ACK is lost. Insufficient initial funding refuses admission. Never call these methods for local or BYO provider subscriptions, and never derive actual cost from client-provided totals. This change supplies billing integration points; it does not add a new model inference transport or provider credentials.

`GET /v1/billing/models/usage?limit=50&cursor=...` exposes paginated account-scoped model/chat/request attribution to authorized billing readers. Usage attribution stays separate by resource while funding is shared. Legacy spending-cap controls are hidden for shared-credit accounts.

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

### Zuse merchant exercise, 2026-11-10

Before the shared-credit policy change, the opt-in `stripe_merchant_checkout_wallet_refund_and_compute` test passed against a real Stripe test account and a disposable PostgreSQL schema. Chrome completed both native-created hosted Checkouts using a Stripe test card. Stripe reported both payments paid. The $40/month subscription produced the host compute period, the $10 prepaid payment granted exactly 1,000 cents, a real successful $5 partial refund left 500 cents, and a replayed 25-cent model settlement left 475 cents. Replaying the refund notification did not claw back twice.

That historical run used the earlier model-only wallet policy; it does not qualify the new $40 monthly shared grant. The harness now expects $50 after subscription plus $10 top-up, $45 after the $5 refund and $44.75 after model settlement. Shared-policy accounting is covered by the isolated PostgreSQL integration test.

The exercise retrieved real Stripe events and signed them locally before invoking Zuse's webhook entry point. This verifies canonical provider reconciliation and host accounting, **not registered public webhook delivery**. It does not qualify renewal, 3DS, ACH, disputes, tax, metered overage collection or live-mode payments. The first harness attempt included unrelated customer events despite having no legacy fallback adapter; the corrected harness selects the relevant billing notifications. No production billing change was required by this exercise.

Both runs' synthetic customers were deleted, and their dedicated prices/products were archived and independently checked afterward. Raw IDs, browser checkout URLs and result evidence remain in the ignored `.context/stripe-live-evidence/` directory; no credential is committed.

To repeat, use a protected test-key file and a fresh evidence directory, run the ignored test, complete the two URLs written to `checkouts.json`, then create the `browser-completed` marker only after Stripe confirms both sessions paid:

```sh
ZUSE_STRIPE_LIVE_QUALIFICATION=test-only \
ZUSE_STRIPE_TEST_KEY_FILE=/absolute/path/to/test-key \
ZUSE_STRIPE_EVIDENCE_DIR=/absolute/path/to/fresh-evidence-directory \
ZUSE_CLOUD_TEST_DATABASE_URL=postgresql://user@localhost/disposable_database \
  cargo test -p zuse-cloud --lib stripe_merchant_checkout_wallet_refund_and_compute \
  -- --ignored --nocapture
```
