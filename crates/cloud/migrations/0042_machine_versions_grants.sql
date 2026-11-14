-- 0031 created cloud_machine_versions without granting the restricted
-- application role; production's worker failed with permission denied (42501)
-- reading machine recipes. Grant it like 0034/0038 do.
DO $grants$
DECLARE schema_name text := current_schema(); app_role text;
BEGIN
 app_role := CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.cloud_machine_versions TO %I',schema_name,app_role);
 END IF;
END $grants$;
