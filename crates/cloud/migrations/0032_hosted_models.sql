-- One row per proxied hosted-model completion: the per-request spend history.
-- Opened with a wallet hold before the upstream call, closed with reported usage,
-- then settled against the hold. A row is never settled twice.
CREATE TABLE IF NOT EXISTS zuse_rust_model_usage (
 request_id text PRIMARY KEY,
 account_id text NOT NULL,
 model_id text NOT NULL,
 reservation_id text NOT NULL,
 input_tokens bigint NOT NULL DEFAULT 0 CHECK(input_tokens>=0),
 output_tokens bigint NOT NULL DEFAULT 0 CHECK(output_tokens>=0),
 estimate_micros bigint NOT NULL CHECK(estimate_micros>=0),
 hold_micros bigint NOT NULL CHECK(hold_micros>0),
 cost_micros bigint CHECK(cost_micros>=0),
 estimated boolean NOT NULL DEFAULT false,
 started_at bigint NOT NULL,
 finished_at bigint,
 settled_at bigint,
 CHECK((finished_at IS NULL)=(cost_micros IS NULL))
);
CREATE INDEX IF NOT EXISTS zuse_rust_model_usage_open ON zuse_rust_model_usage(started_at) WHERE finished_at IS NULL;
CREATE INDEX IF NOT EXISTS zuse_rust_model_usage_unsettled ON zuse_rust_model_usage(finished_at) WHERE finished_at IS NOT NULL AND settled_at IS NULL;
CREATE INDEX IF NOT EXISTS zuse_rust_model_usage_account ON zuse_rust_model_usage(account_id);
