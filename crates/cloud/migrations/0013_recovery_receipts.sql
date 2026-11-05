-- Recovery mutations retain their original provider identity through worker failure.
ALTER TABLE zuse_rust_recovery_operations ADD COLUMN provider_effects jsonb NOT NULL DEFAULT '{}'
    CHECK(jsonb_typeof(provider_effects)='object');
