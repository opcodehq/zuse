-- One native credit pool pays for compute, storage and Zuse-funded inference.
-- Historical periods and Stripe export receipts retain their original funding lane.
ALTER TABLE api_cloud_billing_periods ADD COLUMN shared_credit_account boolean NOT NULL DEFAULT false;
ALTER TABLE api_cloud_billing_periods ADD COLUMN shared_credit_hold boolean NOT NULL DEFAULT false;
CREATE TABLE zuse_brokeboy_compute_charges (
 account_id text NOT NULL REFERENCES zuse_brokeboy_accounts(account_id),
 entry_id text NOT NULL,
 period_id text NOT NULL REFERENCES api_cloud_billing_periods(period_id),
 amount_micros bigint NOT NULL CHECK(amount_micros >= 0),
 state text NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','paid','reconciliation')),
 created_at bigint NOT NULL,
 PRIMARY KEY(account_id,entry_id)
);
CREATE INDEX zuse_brokeboy_compute_pending ON zuse_brokeboy_compute_charges(account_id,state,created_at,entry_id);
