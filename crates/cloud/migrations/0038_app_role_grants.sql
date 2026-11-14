-- 0035 and 0037 created tables without granting the restricted application
-- role access, so the attachment release sweep and user image builds failed
-- with permission errors under that role. Grant them like 0034 and 0036 do.
DO $grants$
DECLARE schema_name text := current_schema(); app_role text;
BEGIN
 app_role := CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.zuse_rust_launch_asset_holds TO %I',schema_name,app_role);
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.cloud_machine_images TO %I',schema_name,app_role);
 END IF;
END $grants$;
