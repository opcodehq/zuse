-- Provider submission and its original receipt survive a worker crash or lease loss.
ALTER TABLE zuse_rust_lifecycle_operations ADD COLUMN provider_effects jsonb NOT NULL DEFAULT '{}'
    CHECK(jsonb_typeof(provider_effects)='object');
