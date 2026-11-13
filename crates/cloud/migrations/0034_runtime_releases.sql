-- Signed guest runtime releases a computer can move to without a new machine
-- image (internal-docs/cloud-runtime-updates.md). Operators register releases
-- with `zuse-cloud-api register-runtime-release`; application roles only read.
CREATE TABLE IF NOT EXISTS zuse_rust_runtime_releases (
    sha256 text PRIMARY KEY CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    version text NOT NULL CHECK (char_length(version) BETWEEN 1 AND 64),
    architecture text NOT NULL CHECK (architecture IN ('x86_64','aarch64')),
    storage_formats jsonb NOT NULL CHECK (jsonb_typeof(storage_formats)='array'),
    -- The guest's command admission receipt, as `runtime-capabilities` prints it.
    capabilities jsonb NOT NULL CHECK (jsonb_typeof(capabilities)='object'),
    -- Capability names advertised at bootstrap; demand updates read these.
    bootstrap_capabilities jsonb CHECK (bootstrap_capabilities IS NULL OR jsonb_typeof(bootstrap_capabilities)='array'),
    signature text NOT NULL CHECK (char_length(signature) BETWEEN 1 AND 128),
    artifact_key text NOT NULL CHECK (artifact_key LIKE 'runtime/%'),
    compressed_size_bytes bigint NOT NULL CHECK (compressed_size_bytes > 0),
    state text NOT NULL CHECK (state IN ('candidate','staging','stable','retired')),
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS zuse_rust_runtime_releases_channel
    ON zuse_rust_runtime_releases(architecture, state);

-- The update the server last offered each computer, written by the API.
CREATE TABLE IF NOT EXISTS zuse_rust_runtime_updates (
    workspace_id text PRIMARY KEY,
    update_id text NOT NULL,
    target_sha256 text NOT NULL CHECK (target_sha256 ~ '^[0-9a-f]{64}$'),
    urgency text NOT NULL CHECK (urgency IN ('background','now')),
    state text NOT NULL CHECK (state IN ('pending','downloading','committed','rolled-back','failed')),
    error_code text,
    offered_at bigint,
    attested_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

-- Failed attempts per computer and release. After two the release is no
-- longer offered to that computer.
CREATE TABLE IF NOT EXISTS zuse_rust_runtime_update_failures (
    workspace_id text NOT NULL,
    sha256 text NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    failures integer NOT NULL CHECK (failures > 0),
    error_code text,
    updated_at bigint NOT NULL,
    PRIMARY KEY (workspace_id, sha256)
);

DO $grants$
DECLARE schema_name text := current_schema(); app_role text;
BEGIN
 app_role := CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT SELECT ON TABLE %I.zuse_rust_runtime_releases TO %I',schema_name,app_role);
  EXECUTE format('REVOKE INSERT,UPDATE,DELETE,TRUNCATE ON TABLE %I.zuse_rust_runtime_releases FROM %I',schema_name,app_role);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.zuse_rust_runtime_updates TO %I',schema_name,app_role);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.zuse_rust_runtime_update_failures TO %I',schema_name,app_role);
 END IF;
END $grants$;
