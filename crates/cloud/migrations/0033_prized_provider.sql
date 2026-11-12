-- Prized joins the managed computer providers. Constraint names match the
-- ones PostgreSQL generated for the original inline CHECKs. Project cache
-- tables come from an optional migration, so absent tables are skipped.
DO $prized$
BEGIN
  IF to_regclass('cloud_managed_bases') IS NOT NULL THEN
    ALTER TABLE cloud_managed_bases DROP CONSTRAINT IF EXISTS cloud_managed_bases_provider_check,
      ADD CONSTRAINT cloud_managed_bases_provider_check CHECK (provider IN ('boat','boxd','e2b','daytona','prized'));
  END IF;
  IF to_regclass('cloud_computers') IS NOT NULL THEN
    ALTER TABLE cloud_computers DROP CONSTRAINT IF EXISTS cloud_computers_provider_check,
      ADD CONSTRAINT cloud_computers_provider_check CHECK (provider IN ('boat','boxd','e2b','daytona','prized'));
  END IF;
  IF to_regclass('zuse_rust_computer_recreations') IS NOT NULL THEN
    ALTER TABLE zuse_rust_computer_recreations DROP CONSTRAINT IF EXISTS zuse_rust_computer_recreations_provider_id_check,
      ADD CONSTRAINT zuse_rust_computer_recreations_provider_id_check CHECK (provider_id IN ('boxd','boat','e2b','daytona','prized'));
  END IF;
  IF to_regclass('cloud_project_cache_capabilities') IS NOT NULL THEN
    ALTER TABLE cloud_project_cache_capabilities DROP CONSTRAINT IF EXISTS cloud_project_cache_capabilities_provider_check,
      ADD CONSTRAINT cloud_project_cache_capabilities_provider_check CHECK (provider IN ('boxd','boat','e2b','daytona','prized'));
  END IF;
  IF to_regclass('zuse_rust_project_cache_entries') IS NOT NULL THEN
    ALTER TABLE zuse_rust_project_cache_entries DROP CONSTRAINT IF EXISTS zuse_rust_project_cache_entries_provider_check,
      ADD CONSTRAINT zuse_rust_project_cache_entries_provider_check CHECK (provider IN ('boxd','boat','e2b','daytona','prized'));
  END IF;
  IF to_regclass('zuse_rust_project_cache_operations') IS NOT NULL THEN
    ALTER TABLE zuse_rust_project_cache_operations DROP CONSTRAINT IF EXISTS zuse_rust_project_cache_operations_provider_check,
      ADD CONSTRAINT zuse_rust_project_cache_operations_provider_check CHECK (provider IN ('boxd','boat','e2b','daytona','prized'));
  END IF;
END
$prized$;
