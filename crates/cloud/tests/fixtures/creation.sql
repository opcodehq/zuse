-- Existing creation/billing schema subset; no migration is applied to production by tests.
ALTER TABLE api_cloud_workspaces ADD COLUMN idempotency_key text;
UPDATE api_cloud_workspaces SET idempotency_key=workspace_id;
ALTER TABLE api_cloud_workspaces ALTER COLUMN idempotency_key SET NOT NULL;
CREATE UNIQUE INDEX ON api_cloud_workspaces(account_id,idempotency_key);
CREATE UNIQUE INDEX ON api_cloud_workspaces(project_id,branch) WHERE state<>'deleted';
CREATE TABLE api_cloud_projects(project_id text PRIMARY KEY,account_id text NOT NULL,state text NOT NULL,included boolean NOT NULL,secret_bindings jsonb NOT NULL DEFAULT '[]');
CREATE TABLE api_cloud_project_builds(build_id text PRIMARY KEY,account_id text NOT NULL,project_id text,provider text NOT NULL,state text NOT NULL,snapshot_id text,settings jsonb NOT NULL,updated_at bigint NOT NULL);
CREATE TABLE api_cloud_snapshots(account_id text NOT NULL,provider text NOT NULL,snapshot_id text NOT NULL,state text NOT NULL);
CREATE TABLE api_cloud_billing_periods(period_id text PRIMARY KEY,account_id text NOT NULL,billing_provider text NOT NULL DEFAULT 'polar',provider_subscription_id text,status text NOT NULL,currency text NOT NULL DEFAULT 'USD',period_start bigint NOT NULL,period_end bigint NOT NULL,included_provider_cost_micros bigint NOT NULL,markup_basis_points bigint NOT NULL,overage_cap_micros bigint NOT NULL);
CREATE TABLE api_cloud_billing_ledger(entry_id text PRIMARY KEY,period_id text NOT NULL,kind text NOT NULL,amount_micros bigint NOT NULL);
CREATE TABLE api_cloud_billing_reservations(period_id text NOT NULL,resource_id text NOT NULL,provider_cost_micros bigint NOT NULL,expires_at bigint NOT NULL);
INSERT INTO api_cloud_projects VALUES('create-project','user-bob','ready',true,'[]'),('private-project','user-alice','ready',true,'[]');
INSERT INTO api_cloud_project_builds VALUES('rust-build','user-bob','create-project','boxd','ready','approved-rust-snapshot','{"runtimeRpcProtocol":"zuse-rpc-v1","rustRuntimeStorageFormat":"zuse-rust-engine-v1","rustSessionCommandVersion":1,"rustRuntimeCheckoutProjectId":"create-project"}',100);
