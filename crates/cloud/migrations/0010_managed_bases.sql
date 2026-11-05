-- A managed base belongs to the platform, never to a customer's image catalog.
ALTER TABLE cloud_managed_bases ADD COLUMN runtime_manifest jsonb;
ALTER TABLE api_cloud_workspaces ALTER COLUMN build_id DROP NOT NULL;
ALTER TABLE api_cloud_workspaces ADD COLUMN managed_release_id text;
ALTER TABLE api_cloud_workspaces ADD CONSTRAINT cloud_workspace_managed_base_fk
    FOREIGN KEY(provider,managed_release_id) REFERENCES cloud_managed_bases(provider,release_id);
ALTER TABLE api_cloud_workspaces ADD CONSTRAINT cloud_workspace_source_required
    CHECK(build_id IS NOT NULL OR managed_release_id IS NOT NULL);
