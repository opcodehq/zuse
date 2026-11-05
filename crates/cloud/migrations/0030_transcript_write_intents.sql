-- Optional transcript PUT handoff: cleanup authority outlives an in-flight
-- request, so object I/O does not need to hold the workspace row lock.
ALTER TABLE zuse_rust_object_write_intents
 DROP CONSTRAINT zuse_rust_object_write_intents_purpose_check;
ALTER TABLE zuse_rust_object_write_intents
 ADD CONSTRAINT zuse_rust_object_write_intents_purpose_check
 CHECK(purpose IN ('tool','asset-chunk','asset-manifest','transcript'));
DO $install$
DECLARE schema_name text:=current_schema(); app_role text;
BEGIN
 EXECUTE format('CREATE FUNCTION %I.zuse_rust_transcript_write_intents_v1() RETURNS boolean LANGUAGE sql IMMUTABLE SECURITY INVOKER SET search_path=pg_catalog,pg_temp AS ''SELECT true''',schema_name);
 EXECUTE format('REVOKE ALL ON FUNCTION %I.zuse_rust_transcript_write_intents_v1() FROM PUBLIC',schema_name);
 app_role:=CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT EXECUTE ON FUNCTION %I.zuse_rust_transcript_write_intents_v1() TO %I',schema_name,app_role);
 END IF;
END $install$;
