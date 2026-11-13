-- Signed agent CLI harness bundles a computer can adopt without a new machine
-- image (infra/cloud/HARNESS_BUNDLES.md). Operators register bundles with
-- `zuse-cloud-api register-harness-bundle`; application roles only read.
CREATE TABLE IF NOT EXISTS zuse_rust_harness_bundles (
    sha256 text PRIMARY KEY CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    agent text NOT NULL CHECK (agent IN ('claude','codex','grok')),
    version text NOT NULL CHECK (char_length(version) BETWEEN 1 AND 64),
    architecture text NOT NULL CHECK (architecture IN ('x86_64','aarch64')),
    executable text NOT NULL CHECK (char_length(executable) BETWEEN 1 AND 256),
    adapter text NOT NULL CHECK (char_length(adapter) BETWEEN 1 AND 64),
    state_format text NOT NULL CHECK (char_length(state_format) BETWEEN 1 AND 64),
    reads_state_formats jsonb NOT NULL CHECK (jsonb_typeof(reads_state_formats)='array'),
    capabilities jsonb NOT NULL CHECK (jsonb_typeof(capabilities)='array'),
    signature text NOT NULL CHECK (char_length(signature) BETWEEN 1 AND 128),
    artifact_key text NOT NULL CHECK (artifact_key LIKE 'runtime/harness/%'),
    compressed_size_bytes bigint NOT NULL CHECK (compressed_size_bytes > 0),
    state text NOT NULL CHECK (state IN ('candidate','staging','canary','stable','retired')),
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS zuse_rust_harness_bundles_channel
    ON zuse_rust_harness_bundles(agent, architecture, state);

-- The bundle the server last offered each computer per agent, written by the API.
CREATE TABLE IF NOT EXISTS zuse_rust_harness_updates (
    workspace_id text NOT NULL,
    agent text NOT NULL,
    update_id text NOT NULL,
    target_sha256 text NOT NULL CHECK (target_sha256 ~ '^[0-9a-f]{64}$'),
    state text NOT NULL CHECK (state IN ('offered','adopted','failed')),
    error_code text,
    offered_at bigint,
    attested_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    PRIMARY KEY (workspace_id, agent)
);

-- What each computer binds new harness processes to, as it last reported.
-- `sha256` is NULL for the CLI that came with the machine image.
CREATE TABLE IF NOT EXISTS zuse_rust_harness_bindings (
    workspace_id text NOT NULL,
    agent text NOT NULL,
    sha256 text CHECK (sha256 IS NULL OR sha256 ~ '^[0-9a-f]{64}$'),
    version text NOT NULL CHECK (char_length(version) BETWEEN 1 AND 64),
    state_format text NOT NULL CHECK (char_length(state_format) BETWEEN 1 AND 64),
    previous_sha256 text CHECK (previous_sha256 IS NULL OR previous_sha256 ~ '^[0-9a-f]{64}$'),
    adopted_at bigint,
    reported_at bigint NOT NULL,
    PRIMARY KEY (workspace_id, agent)
);

-- Failed attempts per computer and bundle. After two the bundle is no longer
-- offered to that computer.
CREATE TABLE IF NOT EXISTS zuse_rust_harness_update_failures (
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
  EXECUTE format('GRANT SELECT ON TABLE %I.zuse_rust_harness_bundles TO %I',schema_name,app_role);
  EXECUTE format('REVOKE INSERT,UPDATE,DELETE,TRUNCATE ON TABLE %I.zuse_rust_harness_bundles FROM %I',schema_name,app_role);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.zuse_rust_harness_updates TO %I',schema_name,app_role);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.zuse_rust_harness_bindings TO %I',schema_name,app_role);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.zuse_rust_harness_update_failures TO %I',schema_name,app_role);
 END IF;
END $grants$;
