# Production cloud access and billing settings

Production admits every signed-in account that pays for cloud. Staging keeps its
per-account allowlists. One setting selects the behavior:

| `ZUSE_CLOUD_ACCESS_POLICY` | Who may use cloud writes |
|---|---|
| `allowlist` (default, staging) | Only accounts in each feature's `*_ACCOUNTS` list. An empty list turns the feature off. |
| `entitled` (production) | Every signed-in account. The paid `cloud-workspace` entitlement and the billing period's allowance and overage cap decide whether compute may be allocated. |

Any other value stops the API, workers and `deploy.py`. Put the setting in the
environment's shared `runtime.env`, so the API and every worker read the same policy.
Source: `crates/cloud/src/access_policy.rs`.

## What `entitled` opens

Each list below accepts every account under `entitled`. Under `allowlist` it is
unchanged.

| List | Feature |
|---|---|
| `ZUSE_CLOUD_PROJECT_WRITE_ACCOUNTS` | Repository selection, environments, machine recipes and image rebuilds |
| `ZUSE_CLOUD_CREATE_ACCOUNTS` | Computer creation, launch assets, project preparation and idle sleep |
| `ZUSE_CLOUD_GITHUB_WRITE_ACCOUNTS` | GitHub App connection and repository access |
| `ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS` | Agent credential connections and the credential authority worker |
| `ZUSE_CLOUD_FAST_START_ACCOUNTS` | Fast start (allowlist mode keeps it staging-only) |
| `ZUSE_CLOUD_IMAGE_WRITE_ACCOUNTS` | Legacy repository image builder, if it is configured |
| `ZUSE_CLOUD_BILLING_WRITE_ACCOUNTS` | Checkout, portal, overage cap, subscription webhooks, settlement and meter export |

The first write by a new account records it as Rust-owned (`zuse_rust_control_owners`
rows for `project-catalog`, `image-catalog`, `billing` and `billing-export`). An
account explicitly marked `legacy` keeps its fence.

These stay unchanged under `entitled`:

- **Organizations.** Organization workspaces still need
  `ORGANIZATION_WORKSPACES_ENABLED=true` and the organization ID in
  `ZUSE_CLOUD_ORGANIZATION_IDS`. WorkOS membership and role checks always apply.
- **Embedded shared-credit billing.** It stays limited to the accounts in
  `ZUSE_BROKEBOY_ACCOUNTS`.
- **Staging-only paths.** These refuse to start under `entitled`: the staging
  base pin (`ZUSE_CLOUD_STAGING_BASE_PIN`), and with it the staging compute
  allowance and project cache pilots, plus candidate authority releases. Staging
  operator commands (model discovery and credential renewal) still need exactly
  one staging owner.

## What is checked before compute is allocated

Each of these checks the paid entitlement first, then billing capacity. An unpaid
account gets `cloud_entitlement_required` and nothing is written:

- computer creation
- allocation
- wake and resume
- recreation and update
- repair, restore and recovery
- project cache builds
- machine image builds: the "Rebuild now" request, and the build worker before
  and during each build

Saving a machine recipe stays allowed. Its queued build waits, with the error code
shown, until the account is entitled.

## Production environment checklist

`runtime.env` (shared):

```
ZUSE_DEPLOYMENT_ENVIRONMENT=production   # set by Compose
ZUSE_CLOUD_ACCESS_POLICY=entitled
ZUSE_CLOUD_RUNTIME_WRITES=true
ZUSE_CLOUD_DATABASE_URL=...              # production schema and role
CLOUD_DATA_ENCRYPTION_KEY=...
RELAY_MINT_PRIVATE_JWK=...
API_ISSUER=https://<production API origin>
WORKOS_CLIENT_ID=... / WORKOS_API_KEY=...
ZUSE_CLOUD_ALLOWED_ORIGINS=...
ZUSE_CLOUD_PROVIDER_TEMPLATE_VERSIONS=... # verified managed bases
# Leave every *_ACCOUNTS list empty; never set ZUSE_CLOUD_STAGING_BASE_PIN.
```

`api.env` (API only, also read by the billing worker):

```
GITHUB_APP_ID / GITHUB_APP_SLUG / GITHUB_APP_CLIENT_ID / ZUSE_CLOUD_GITHUB_ORIGIN
GITHUB_APP_PRIVATE_KEY_FILE / GITHUB_APP_CLIENT_SECRET_FILE / GITHUB_WEBHOOK_SECRET_FILE
STRIPE_SECRET_KEY=sk_live_...
STRIPE_ENVIRONMENT=production
STRIPE_WEBHOOK_SECRET=whsec_...
STRIPE_PRICE_CLOUD_WORKSPACE_STANDARD_V1=price_...  # monthly plan price
STRIPE_CLOUD_BILLING_MODE=metered
STRIPE_CLOUD_OVERAGE_METER_ID=mtr_...
STRIPE_CLOUD_OVERAGE_PRICE_ID=price_...             # metered price on that meter
STRIPE_PORTAL_CONFIGURATION_ID=bpc_...
STRIPE_AUTOMATIC_TAX_ENABLED=true|false
STRIPE_PREPAID_CREDIT_PRICE_ID=price_...            # optional, prepaid credits
BILLING_DEFAULT_PROVIDER=stripe
CLOUD_BILLING_EXPORT_ENABLED=true                   # export overage to the meter
CLOUD_USAGE_EXPORT_ENABLED=true|false               # cost telemetry (non-billable)
BOXD_BILLING_ENABLED=true
BOXD_ADAPTER_ENABLED=true
BOXD_BILLING_CUTOVER_AT=<RFC 3339, whole second>    # first settled window
CLOUD_SNAPSHOT_BILLING_CUTOVER_AT=<RFC 3339>        # snapshot storage
```

`deploy.py` reacts to `entitled`:

- It requires the complete GitHub App configuration and adds `github-api.yaml`.
- It enables the `authority` profile (credential authority worker).
- It enables the `billing` profile, which runs `billing-worker` for settlement and
  meter export.
- It refuses a staging base pin.

## Stripe objects to create (live mode)

1. A product with a $40 per month recurring price for
   `cloud-workspace-standard-v1`. Set it as `STRIPE_PRICE_CLOUD_WORKSPACE_STANDARD_V1`.
   A period includes $35 of provider cost, then charges cost + 5%, with a default
   overage cap of $25.
2. A billing meter with event name `zuse_cloud_overage_cent`, summing values and
   keyed by customer (`STRIPE_CLOUD_OVERAGE_METER_ID`). Add a $0.01 per unit metered
   price on it (`STRIPE_CLOUD_OVERAGE_PRICE_ID`). Checkout adds it as the second line
   item.
3. A customer portal configuration (`STRIPE_PORTAL_CONFIGURATION_ID`).
4. A webhook endpoint at `<API origin>/v1/billing/webhook/stripe` with these events:
   - `checkout.session.completed`, `checkout.session.async_payment_succeeded`
   - `customer.subscription.created`, `.updated`, `.deleted`, `.paused`, `.resumed`
   - `invoice.paid`, `invoice.payment_failed`
   - `charge.refunded`, `charge.dispute.funds_withdrawn`,
     `charge.dispute.funds_reinstated`, `refund.updated`

   Use its signing secret as `STRIPE_WEBHOOK_SECRET`. Each renewal arrives as
   `customer.subscription.updated` and opens the next billing period.
5. Optional: a one-time prepaid credit price (`STRIPE_PREPAID_CREDIT_PRICE_ID`).
