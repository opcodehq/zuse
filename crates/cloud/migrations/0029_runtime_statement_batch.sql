-- Optional control-plane transport optimization. The application supplies only
-- static statements and binds all values separately. This grants no privileges:
-- each statement runs as the restricted invoking application role.
DO $install$
DECLARE schema_name text:=current_schema(); app_role text;
BEGIN
 EXECUTE format($definition$
 CREATE FUNCTION %I.zuse_rust_runtime_statement_batch(statements jsonb, arguments jsonb)
 RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY INVOKER
 SET search_path=%I,pg_catalog,pg_temp AS $function$
 DECLARE statement jsonb; value jsonb; result jsonb:='{}'::jsonb; label text;
 BEGIN
  IF current_setting('transaction_isolation')<>'read committed' THEN
   RAISE EXCEPTION 'runtime_batch_requires_read_committed' USING ERRCODE='22023';
  END IF;
  IF jsonb_typeof(statements) IS DISTINCT FROM 'array'
   OR jsonb_array_length(statements) NOT BETWEEN 1 AND 32
   OR octet_length(statements::text)>262144 THEN
   RAISE EXCEPTION 'runtime_batch_invalid_statements' USING ERRCODE='22023';
  END IF;
  FOR statement IN SELECT jsonb_array_elements(statements) LOOP
   label:=statement->>'name';
   IF label IS NULL OR label !~ '^[a-zA-Z][a-zA-Z0-9_]{0,63}$'
    OR result ? label OR jsonb_typeof(statement->'sql') IS DISTINCT FROM 'string' THEN
    RAISE EXCEPTION 'runtime_batch_invalid_statement' USING ERRCODE='22023';
   END IF;
   -- VOLATILE preserves a fresh snapshot for every dependent statement,
   -- including reads following an advisory/row-lock wait. STRICT requires one
   -- JSON value per statement rather than silently dropping additional rows.
   EXECUTE statement->>'sql' INTO STRICT value USING arguments,result;
   result:=result||jsonb_build_object(label,value);
  END LOOP;
  RETURN result;
 END $function$
 $definition$,schema_name,schema_name);
 EXECUTE format('REVOKE ALL ON FUNCTION %I.zuse_rust_runtime_statement_batch(jsonb,jsonb) FROM PUBLIC',schema_name);
 app_role:=CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT EXECUTE ON FUNCTION %I.zuse_rust_runtime_statement_batch(jsonb,jsonb) TO %I',schema_name,app_role);
 END IF;
END $install$;
