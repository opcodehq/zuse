-- Application roles can pin a platform release without gaining registry UPDATE.
-- Return the row read under the lock, so an activation/revocation which wins the
-- lock race cannot leave a stale outer-query manifest eligible for admission.
DO $install$
DECLARE schema_name text := current_schema(); app_role text;
BEGIN
 EXECUTE format($definition$
 CREATE FUNCTION %I.zuse_rust_managed_base_admission_lock(text,text)
 RETURNS SETOF %I.cloud_managed_bases
 LANGUAGE sql VOLATILE SECURITY DEFINER
 SET search_path=pg_catalog,pg_temp
 AS $function$
 SELECT b.* FROM %I.cloud_managed_bases b
 WHERE b.provider=$1 AND b.release_id=$2 FOR SHARE
 $function$
 $definition$,schema_name,schema_name,schema_name);
 EXECUTE format('REVOKE ALL ON FUNCTION %I.zuse_rust_managed_base_admission_lock(text,text) FROM PUBLIC',schema_name);
 app_role := CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT EXECUTE ON FUNCTION %I.zuse_rust_managed_base_admission_lock(text,text) TO %I',schema_name,app_role);
 END IF;
END $install$;
