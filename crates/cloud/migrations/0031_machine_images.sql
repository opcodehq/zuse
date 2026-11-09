-- One machine image per account, built from the account's machine recipe on
-- top of a managed base. Recipes hold no secrets and images hold no
-- repositories; a task clones its own repository at start.
CREATE TABLE IF NOT EXISTS cloud_machine_versions (
    owner_id text NOT NULL,
    revision bigint NOT NULL CHECK (revision>0),
    actor_id text NOT NULL,
    actor jsonb,
    recipe jsonb NOT NULL,
    digest text NOT NULL CHECK (digest ~ '^[0-9a-f]{64}$'),
    created_at bigint NOT NULL,
    PRIMARY KEY (owner_id, revision)
);

-- At most one image per (account, provider, base release, recipe digest).
-- A newer image never replaces a ready one until it is ready itself.
CREATE TABLE IF NOT EXISTS cloud_machine_images (
    owner_id text NOT NULL,
    provider text NOT NULL,
    base_release_id text NOT NULL,
    recipe_digest text NOT NULL CHECK (recipe_digest ~ '^[0-9a-f]{64}$'),
    revision bigint NOT NULL CHECK (revision>0),
    state text NOT NULL CHECK (state IN ('queued','building','ready','failed','retired')),
    artifact_id text,
    artifact_binding jsonb,
    error_code text,
    lease_id text,
    lease_expires_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    PRIMARY KEY (owner_id, provider, base_release_id, recipe_digest),
    CHECK ((state = 'ready') = (artifact_id IS NOT NULL) OR state = 'retired')
);
CREATE INDEX IF NOT EXISTS cloud_machine_images_owner_ready
    ON cloud_machine_images(owner_id, provider, state, updated_at DESC);
CREATE INDEX IF NOT EXISTS cloud_machine_images_queue
    ON cloud_machine_images(state, updated_at) WHERE state IN ('queued','building');
