-- Project caches are optional, account/project scoped, and never task snapshots.
-- No capability is inferred/backfilled for previously published runtimes.
CREATE TABLE cloud_project_cache_capabilities (
 provider text NOT NULL CHECK(provider IN ('boxd','boat','e2b','daytona')),
 release_id text NOT NULL,
 runtime_sha256 text NOT NULL CHECK(runtime_sha256 ~ '^[a-f0-9]{64}$'),
 architecture text NOT NULL CHECK(architecture IN ('x86_64','aarch64')),
 receipt jsonb NOT NULL CHECK(jsonb_typeof(receipt)='object'),
 state text NOT NULL CHECK(state IN ('certified','retired')),
 certified_at bigint NOT NULL,
 PRIMARY KEY(provider,release_id),
 FOREIGN KEY(provider,release_id) REFERENCES cloud_managed_bases(provider,release_id)
);

CREATE TABLE zuse_rust_project_preparation_requests (
 request_id text PRIMARY KEY,
 owner_id text NOT NULL,
 actor_id text NOT NULL,
 actor jsonb,
 project_id text NOT NULL,
 environment_revision bigint NOT NULL CHECK(environment_revision>0),
 recipe_digest text NOT NULL CHECK(recipe_digest ~ '^[a-f0-9]{64}$'),
 repository_identity text NOT NULL,
 requested_ref text NOT NULL,
 state text NOT NULL CHECK(state IN ('queued','resolving','preparing','ready','blocked','failed','superseded')),
 last_error_code text,
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 next_attempt_at bigint NOT NULL,
 lease_id text,
 lease_expires_at bigint,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 UNIQUE(owner_id,project_id,environment_revision,requested_ref),
 FOREIGN KEY(owner_id,project_id,environment_revision) REFERENCES cloud_environment_versions(owner_id,project_id,revision)
);
CREATE INDEX zuse_rust_project_preparation_due ON zuse_rust_project_preparation_requests(next_attempt_at,request_id) WHERE state IN ('queued','resolving','preparing','blocked');

CREATE TABLE zuse_rust_project_cache_entries (
 cache_key text PRIMARY KEY CHECK(cache_key ~ '^[a-f0-9]{64}$'),
 owner_id text NOT NULL,
 project_id text NOT NULL,
 provider text NOT NULL CHECK(provider IN ('boxd','boat','e2b','daytona')),
 architecture text NOT NULL CHECK(architecture IN ('x86_64','aarch64')),
 base_release_id text NOT NULL,
 base_artifact_id text NOT NULL,
 base_runtime_sha256 text NOT NULL CHECK(base_runtime_sha256 ~ '^[a-f0-9]{64}$'),
 identity jsonb NOT NULL CHECK(jsonb_typeof(identity)='object'),
 recipe_digest text NOT NULL CHECK(recipe_digest ~ '^[a-f0-9]{64}$'),
 dependency_digest text NOT NULL CHECK(dependency_digest ~ '^[a-f0-9]{64}$'),
 source_commit text NOT NULL CHECK(source_commit ~ '^[a-f0-9]{40}([a-f0-9]{24})?$'),
 state text NOT NULL CHECK(state IN ('preparing','ready','retired')),
 artifact_id text,
 artifact_version text,
 artifact_binding jsonb,
 receipt jsonb,
 preparation_operation_id text,
 created_at bigint NOT NULL,
 verified_at bigint,
 revoked_at bigint,
 FOREIGN KEY(provider,base_release_id) REFERENCES cloud_managed_bases(provider,release_id),
 CHECK(state<>'ready' OR (artifact_id IS NOT NULL AND receipt IS NOT NULL AND verified_at IS NOT NULL AND preparation_operation_id IS NOT NULL))
);
CREATE INDEX zuse_rust_project_cache_lookup ON zuse_rust_project_cache_entries(owner_id,project_id,provider,base_release_id,source_commit,recipe_digest,verified_at DESC) WHERE state='ready' AND revoked_at IS NULL;

-- Operation/resource receipts deliberately survive project removal. Cleanup must
-- reconcile the original provider operation even after authorization is revoked.
CREATE TABLE zuse_rust_project_cache_operations (
 operation_id text PRIMARY KEY,
 request_id text NOT NULL,
 cache_key text NOT NULL REFERENCES zuse_rust_project_cache_entries(cache_key),
 owner_id text NOT NULL,
 actor_id text NOT NULL,
 actor jsonb,
 project_id text NOT NULL,
 provider text NOT NULL CHECK(provider IN ('boxd','boat','e2b','daytona')),
 state text NOT NULL CHECK(state IN ('queued','running','uncertain','ready','failed','revoked','cleaning','deleted')),
 phase text NOT NULL CHECK(phase IN ('resolve','allocate','prepare','quiesce','capture','retire-builder','verify','retire-verifier','publish','cleanup','complete')),
 revision bigint NOT NULL DEFAULT 1 CHECK(revision>0),
 plan jsonb NOT NULL CHECK(jsonb_typeof(plan)='object'),
 started_at bigint,
 resource_receipts jsonb NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(resource_receipts)='object'),
 lease_id text,
 lease_expires_at bigint,
 next_attempt_at bigint NOT NULL,
 last_error_code text,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 UNIQUE(request_id,cache_key)
);
CREATE INDEX zuse_rust_project_cache_operations_due ON zuse_rust_project_cache_operations(next_attempt_at,operation_id) WHERE state IN ('queued','running','uncertain','cleaning');
CREATE TABLE zuse_rust_project_preparation_keys (
 request_id text NOT NULL REFERENCES zuse_rust_project_preparation_requests(request_id),
 cache_key text NOT NULL REFERENCES zuse_rust_project_cache_entries(cache_key),
 PRIMARY KEY(request_id,cache_key)
);
CREATE UNIQUE INDEX zuse_rust_project_cache_one_active_identity ON zuse_rust_project_cache_operations(cache_key) WHERE state IN ('queued','running','uncertain','cleaning');

-- Publishing/retiring cache certification is operator-only. Admission can lock
-- one exact certificate without obtaining registry UPDATE authority.
DO $install$
DECLARE schema_name text:=current_schema(); app_role text;
BEGIN
 EXECUTE format($definition$
 CREATE FUNCTION %I.zuse_rust_project_cache_capability_lock(text,text)
 RETURNS SETOF %I.cloud_project_cache_capabilities
 LANGUAGE sql VOLATILE SECURITY DEFINER
 SET search_path=pg_catalog,pg_temp
 AS $function$
 SELECT c.* FROM %I.cloud_project_cache_capabilities c
 WHERE c.provider=$1 AND c.release_id=$2 FOR SHARE
 $function$
 $definition$,schema_name,schema_name,schema_name);
 EXECUTE format('REVOKE ALL ON FUNCTION %I.zuse_rust_project_cache_capability_lock(text,text) FROM PUBLIC',schema_name);
 app_role:=CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT EXECUTE ON FUNCTION %I.zuse_rust_project_cache_capability_lock(text,text) TO %I',schema_name,app_role);
 END IF;
END $install$;

-- Realtime invalidations contain only identity and safe phase metadata. Lease
-- renewal/backoff does not manufacture user-visible progress events.
CREATE FUNCTION zuse_rust_project_preparation_event() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
 IF to_regclass('zuse_rust_account_event_outbox') IS NOT NULL AND
   (TG_OP='INSERT' OR NEW.state IS DISTINCT FROM OLD.state OR NEW.last_error_code IS DISTINCT FROM OLD.last_error_code) THEN
  INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
  VALUES(NEW.owner_id,'environment',NEW.project_id,jsonb_build_object('revision',NEW.environment_revision,'preparationState',NEW.state),NEW.updated_at);
 END IF;
 RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_project_preparation_events AFTER INSERT OR UPDATE ON zuse_rust_project_preparation_requests FOR EACH ROW EXECUTE FUNCTION zuse_rust_project_preparation_event();
-- Background preparation is bounded to one original resource chain per owner;
-- uncertainty/cleanup retains its slot until independent resource retirement.
CREATE UNIQUE INDEX zuse_rust_project_cache_one_active_owner ON zuse_rust_project_cache_operations(owner_id) WHERE state IN ('running','uncertain','cleaning');

-- Public source only. Intents survive project removal and retain original R2
-- object keys for reconciliation/cleanup after uncertain immutable PUTs.
CREATE TABLE zuse_rust_project_source_bundles (
 owner_id text NOT NULL,project_id text NOT NULL,
 source_commit text NOT NULL CHECK(source_commit ~ '^[a-f0-9]{40}([a-f0-9]{24})?$'),
 bundle_sha256 text NOT NULL CHECK(bundle_sha256 ~ '^[a-f0-9]{64}$'),
 bytes bigint NOT NULL CHECK(bytes>0 AND bytes<=33554432),
 chunks jsonb NOT NULL CHECK(jsonb_typeof(chunks)='array' AND jsonb_array_length(chunks)<=32),
 state text NOT NULL CHECK(state IN ('writing','ready','cleaning')),
 created_at bigint NOT NULL,updated_at bigint NOT NULL,
 PRIMARY KEY(owner_id,project_id,bundle_sha256)
);

-- Resource observations are operational accounting, not a synthetic invoice.
-- Unknown/lost provider replies remain unpriced and retain the original intent.
CREATE TABLE zuse_rust_project_cache_resources (
 operation_id text NOT NULL,
 owner_id text NOT NULL,
 project_id text NOT NULL,
 provider text NOT NULL,
 resource_kind text NOT NULL CHECK(resource_kind IN ('builder','verifier','artifact')),
 resource_id text NOT NULL,
 first_observed_at bigint NOT NULL,
 last_observed_at bigint NOT NULL,
 absent_observed_at bigint,
 cost_status text NOT NULL DEFAULT 'unpriced' CHECK(cost_status='unpriced'),
 PRIMARY KEY(operation_id,resource_kind),
 CHECK(last_observed_at>=first_observed_at),
 CHECK(absent_observed_at IS NULL OR absent_observed_at>=first_observed_at)
);
ALTER TABLE zuse_rust_project_source_bundles ADD COLUMN cleanup_due_at bigint;
ALTER TABLE zuse_rust_project_source_bundles ADD COLUMN cleanup_lease_id text;
ALTER TABLE zuse_rust_project_source_bundles ADD COLUMN cleanup_lease_expires_at bigint;
ALTER TABLE zuse_rust_project_source_bundles ADD COLUMN cleanup_attempts integer NOT NULL DEFAULT 0;
CREATE INDEX zuse_rust_project_source_cleanup_due ON zuse_rust_project_source_bundles(cleanup_due_at) WHERE state='cleaning';
