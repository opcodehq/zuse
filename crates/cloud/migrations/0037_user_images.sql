-- User images: one built image line per account (personal or organization)
-- and provider, made from the account's setup script on top of a verified
-- managed base. No repository, commit or credential is part of an image.
--
-- 0031 created a placeholder `cloud_machine_images` that nothing wrote. It is
-- replaced by a build record with a durable phase journal. The replacement
-- refuses to run over a populated placeholder.
DO $replace$
BEGIN
 IF to_regclass('cloud_machine_images') IS NOT NULL
    AND NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='cloud_machine_images' AND column_name='image_id') THEN
  IF EXISTS(SELECT 1 FROM cloud_machine_images) THEN
   RAISE EXCEPTION 'cloud_machine_images placeholder is not empty';
  END IF;
  DROP TABLE cloud_machine_images;
 END IF;
END $replace$;

CREATE TABLE IF NOT EXISTS cloud_machine_images (
    image_id text PRIMARY KEY CHECK (image_id ~ '^uimg_[0-9a-f]{32}$'),
    owner_id text NOT NULL,
    provider text NOT NULL,
    -- Why this build exists: recipe save, new managed base, manual rebuild,
    -- scheduled refresh or first use after the base changed.
    reason text NOT NULL CHECK (reason IN ('recipe','base','manual','refresh','use')),
    -- queued → building → ready | cleaning → failed. A queued build that a
    -- newer request replaced, or that turned out identical to a ready image,
    -- is superseded. Ready images retire (provider snapshot deleted) only once
    -- no computer references them.
    state text NOT NULL CHECK (state IN ('queued','building','cleaning','failed','superseded','ready','retiring','retired')),
    phase text,
    recipe_revision bigint NOT NULL CHECK (recipe_revision > 0),
    -- Digest of the image-relevant recipe fields only (setup script, timeout).
    recipe_digest text NOT NULL CHECK (recipe_digest ~ '^[0-9a-f]{64}$'),
    -- Filled when the worker freezes the build plan against a verified base.
    architecture text,
    base_release_id text,
    base_artifact_id text,
    base_artifact_version text,
    base_runtime_sha256 text CHECK (base_runtime_sha256 IS NULL OR base_runtime_sha256 ~ '^[0-9a-f]{64}$'),
    identity_key text CHECK (identity_key IS NULL OR identity_key ~ '^[0-9a-f]{64}$'),
    plan jsonb,
    -- Opaque provider IDs and readiness booleans only; never provider output.
    resources jsonb NOT NULL DEFAULT '{}'::jsonb,
    artifact_id text,
    artifact_version text,
    error_code text,
    -- Bounded, redacted tail of the setup script's output for the owner.
    log_excerpt text CHECK (log_excerpt IS NULL OR length(log_excerpt) <= 8192),
    lease_id text,
    lease_expires_at bigint,
    next_attempt_at bigint NOT NULL,
    started_at bigint,
    built_at bigint,
    retired_at bigint,
    revision bigint NOT NULL DEFAULT 0,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    CHECK ((state IN ('ready','retiring')) = (artifact_id IS NOT NULL AND built_at IS NOT NULL) OR state IN ('retired','cleaning','failed','building')),
    CHECK (state IN ('queued','superseded') OR identity_key IS NOT NULL)
);
-- One build waits per account and provider; a newer request replaces it.
CREATE UNIQUE INDEX IF NOT EXISTS cloud_machine_images_one_queued
    ON cloud_machine_images(owner_id, provider) WHERE state = 'queued';
-- One build or failure cleanup holds provider resources per account.
CREATE UNIQUE INDEX IF NOT EXISTS cloud_machine_images_one_active
    ON cloud_machine_images(owner_id) WHERE state IN ('building','cleaning','retiring');
CREATE INDEX IF NOT EXISTS cloud_machine_images_owner_ready
    ON cloud_machine_images(owner_id, provider, built_at DESC) WHERE state = 'ready';
CREATE INDEX IF NOT EXISTS cloud_machine_images_work
    ON cloud_machine_images(state, next_attempt_at) WHERE state IN ('queued','building','cleaning','retiring');
