-- Brokeboy 1e35dc79 PostgreSQL DDL. Applied explicitly, never on API startup.
CREATE TABLE IF NOT EXISTS brokeboy_locks (scope VARCHAR(191) PRIMARY KEY, revision INTEGER NOT NULL DEFAULT 0, token VARCHAR(64));
CREATE TABLE IF NOT EXISTS brokeboy_resources (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_resources_state ON brokeboy_resources (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_customers (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_customers_state ON brokeboy_customers (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_entities (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_entities_state ON brokeboy_entities (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_catalog (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_catalog_state ON brokeboy_catalog (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_subscriptions (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_subscriptions_state ON brokeboy_subscriptions (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_grants (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_grants_state ON brokeboy_grants (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_movements (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_movements_state ON brokeboy_movements (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_reservations (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_reservations_state ON brokeboy_reservations (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_usage (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_usage_state ON brokeboy_usage (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_operations (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_operations_state ON brokeboy_operations (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_inbox (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_inbox_state ON brokeboy_inbox (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_work (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, due BIGINT NOT NULL DEFAULT 0, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_work_state ON brokeboy_work (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_events (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_events_state ON brokeboy_events (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_audit (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_audit_state ON brokeboy_audit (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_counters (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_counters_state ON brokeboy_counters (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_licenses (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_licenses_state ON brokeboy_licenses (scope,state);
CREATE TABLE IF NOT EXISTS brokeboy_workflows (scope VARCHAR(191) NOT NULL, id VARCHAR(191) NOT NULL, state VARCHAR(32) NOT NULL, data TEXT NOT NULL, PRIMARY KEY (scope,id));
CREATE INDEX IF NOT EXISTS brokeboy_workflows_state ON brokeboy_workflows (scope,state);
CREATE INDEX IF NOT EXISTS brokeboy_work_due ON brokeboy_work (scope,state,due,id);

-- Enrollment is durable: changing an environment variable cannot hand an account back
-- to the legacy financial writer. Explicit adoption preserves external identities.
CREATE TABLE zuse_brokeboy_accounts (
    account_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL CHECK (provider IN ('stripe','polar')),
    state TEXT NOT NULL CHECK (state IN ('initializing','active')),
    stripe_customer_id TEXT UNIQUE,
    created_at BIGINT NOT NULL
);
CREATE TABLE zuse_brokeboy_model_requests (
    account_id TEXT NOT NULL REFERENCES zuse_brokeboy_accounts(account_id),
    request_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    model TEXT NOT NULL,
    estimate_micros BIGINT NOT NULL CHECK (estimate_micros > 0),
    held_micros BIGINT NOT NULL CHECK (held_micros > 0),
    increase_target_micros BIGINT,
    actual_micros BIGINT CHECK (actual_micros >= 0),
    reservation_id TEXT,
    state TEXT NOT NULL CHECK (state IN ('reserving','held','increasing','settling','settled','releasing','released','estimated','reconciliation','failed')),
    created_at BIGINT NOT NULL,
    expires_at BIGINT NOT NULL,
    PRIMARY KEY(account_id,request_id)
);
CREATE INDEX zuse_brokeboy_model_pending ON zuse_brokeboy_model_requests(state,created_at);

-- Stable payloads preserve signed return URLs and provider keys across lost replies.
CREATE TABLE zuse_brokeboy_checkout_requests (
 account_id TEXT NOT NULL REFERENCES zuse_brokeboy_accounts(account_id),
 idempotency_key TEXT NOT NULL,
 method TEXT NOT NULL,
 payload JSONB NOT NULL,
 created_at BIGINT NOT NULL,
 PRIMARY KEY(account_id,idempotency_key)
);
