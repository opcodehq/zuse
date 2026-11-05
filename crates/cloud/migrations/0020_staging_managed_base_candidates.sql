-- Explicit operator candidates remain outside the verified release catalogue.
ALTER TABLE cloud_managed_bases ADD COLUMN candidate_manifest jsonb
    CHECK(candidate_manifest IS NULL OR jsonb_typeof(candidate_manifest)='object');
