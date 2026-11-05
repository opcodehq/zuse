-- A replacement's original wake must survive worker loss independently of its phase.
ALTER TABLE zuse_rust_restores ADD COLUMN provider_effects jsonb NOT NULL DEFAULT '{}'
    CHECK(jsonb_typeof(provider_effects)='object');
