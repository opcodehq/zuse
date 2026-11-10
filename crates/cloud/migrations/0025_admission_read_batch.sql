-- One network read; each VOLATILE function statement obtains a fresh snapshot
-- after the account advisory lock has waited. SECURITY INVOKER retains the
-- provisioner's ordinary table restrictions. Rust remains the policy authority.
DO $install$
DECLARE schema_name text:=current_schema(); app_role text;
BEGIN
 EXECUTE format($definition$
 CREATE FUNCTION %I.zuse_rust_admission_read_batch(p jsonb)
 RETURNS jsonb LANGUAGE plpgsql VOLATILE SECURITY INVOKER
 SET search_path=%I,pg_catalog,pg_temp AS $function$
 DECLARE own text:=p->>'owner'; actor text:=p->>'actor'; at_ms bigint:=(p->>'now')::bigint;
 result jsonb; prior jsonb; project jsonb; selected text; environment jsonb; staging jsonb;
 period jsonb; totals jsonb; bases jsonb:='[]'; base_record jsonb; release text;
 workspace text; branch_name text; secret_records jsonb; asset_records jsonb;
 BEGIN
  IF current_setting('transaction_isolation') <> 'read committed' THEN RAISE EXCEPTION 'admission_batch_requires_read_committed' USING ERRCODE='22023';END IF;
  PERFORM pg_advisory_xact_lock(hashtextextended('cloud-create:'||own,0));
  SELECT jsonb_build_object('deleted',EXISTS(SELECT 1 FROM zuse_rust_deletions WHERE account_id=own AND idempotency_digest=p->>'deleteDigest' AND(expires_at IS NULL OR expires_at>at_ms)),
   'prior',(SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE account_id=own AND idempotency_key=p->>'idempotencyKey')) INTO result;
  IF result->>'deleted'='true' OR result->'prior'<>'null'::jsonb THEN RETURN result;END IF;
  PERFORM pg_advisory_xact_lock_shared(hashtextextended('snapshot:'||own,0));
  SELECT to_jsonb(q) INTO project FROM api_cloud_projects q WHERE q.project_id=p->>'projectId' AND q.account_id=own AND included FOR SHARE;
  SELECT owner_id INTO selected FROM cloud_agent_connections WHERE owner_id IN(own,actor) AND connection_id=p->>'connectionId' AND agent=p->>'agent' AND state='connected';
  IF NOT coalesce((p->>'stagingAllowance')::boolean,false) THEN
   SELECT to_jsonb(b) INTO period FROM api_cloud_billing_periods b WHERE account_id=own AND period_start<=at_ms AND period_end>at_ms ORDER BY period_start DESC LIMIT 1 FOR SHARE;
   SELECT jsonb_build_array((SELECT coalesce(sum(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=period->>'period_id' AND kind='provider-cost'),
    (SELECT coalesce(sum(amount_micros),0)::bigint FROM api_cloud_billing_ledger WHERE period_id=period->>'period_id' AND kind='overage-charge'),
    (SELECT coalesce(sum(provider_cost_micros),0)::bigint FROM api_cloud_billing_reservations WHERE period_id=period->>'period_id' AND expires_at>at_ms)) INTO totals;
  END IF;
  IF p->>'candidateRelease' IS NOT NULL THEN
   SELECT to_jsonb(b) INTO base_record FROM zuse_rust_managed_base_admission_lock(p->>'provider',p->>'candidateRelease') b;
   IF base_record IS NOT NULL THEN bases:=jsonb_build_array(base_record);END IF;
  ELSE
   FOR release IN SELECT release_id FROM cloud_managed_bases WHERE provider=p->>'provider' AND state='verified' AND runtime_manifest IS NOT NULL ORDER BY created_at DESC,release_id DESC LOOP
    SELECT to_jsonb(b) INTO base_record FROM zuse_rust_managed_base_admission_lock(p->>'provider',release) b;
    IF base_record IS NOT NULL THEN bases:=bases||jsonb_build_array(base_record);END IF;
   END LOOP;
  END IF;
  IF p->>'environmentRevision' IS DISTINCT FROM '0' THEN
   SELECT jsonb_build_object('revision',revision,'recipe',recipe) INTO environment FROM cloud_environment_versions WHERE owner_id=own AND project_id=p->>'projectId' AND(p->>'environmentRevision' IS NULL OR revision=(p->>'environmentRevision')::bigint) ORDER BY revision DESC LIMIT 1;
  END IF;
  SELECT to_jsonb(s) INTO staging FROM zuse_rust_launch_staging s WHERE account_id=own AND idempotency_digest=p->>'launchDigest' FOR UPDATE;
  workspace:=coalesce(staging->>'workspace_id',p->>'workspaceId');
  branch_name:=coalesce(p->>'branch','cloud-'||substring(workspace FROM 11 FOR 12));
  SELECT coalesce(jsonb_agg(to_jsonb(q)),'[]'::jsonb) INTO secret_records FROM (
   SELECT secret_id,revision,ciphertext FROM zuse_rust_workload_secrets WHERE owner_id=own AND project_id=p->>'projectId' AND revoked_at IS NULL
    AND secret_id IN(SELECT jsonb_array_elements_text(coalesce(p->'secretBindings','[]'::jsonb)||CASE WHEN jsonb_typeof(project->'secret_bindings')='array' THEN project->'secret_bindings' ELSE '[]'::jsonb END||CASE WHEN jsonb_typeof(environment->'recipe'->'secretBindings')='array' THEN environment->'recipe'->'secretBindings' ELSE '[]'::jsonb END)) FOR SHARE
  )q;
  SELECT coalesce(jsonb_agg(to_jsonb(q)),'[]'::jsonb) INTO asset_records FROM (
   SELECT asset_id,manifest FROM zuse_rust_launch_assets WHERE workspace_id=workspace AND state='ready'
    AND asset_id IN(SELECT jsonb_array_elements_text(coalesce(p->'assetIds','[]'::jsonb))) FOR SHARE
  )q;
  RETURN result||jsonb_build_object('project',project,'connectionOwner',selected,'bases',bases,'environment',environment,'staging',staging,'workspaceId',workspace,'branch',branch_name,'billingPeriod',period,'billingTotals',totals,'secretRows',secret_records,'assetRows',asset_records,
   'entitled',EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=own AND kind='cloud-workspace' AND(status IN('active','grace') OR(status='ended' AND paid_through>at_ms)) AND(paid_through IS NULL OR paid_through>at_ms)),
   'candidateExists',EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE account_id=own AND state<>'deleted' AND request_config?'managedCandidateSha256'),
   'githubUserId',(SELECT github_user_id FROM zuse_rust_github_users WHERE actor_id=actor AND state='connected' AND NOT refresh_uncertain),
   'branchInUse',EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE project_id=p->>'projectId' AND branch=branch_name AND state<>'deleted'),
   'unavailableSnapshots',coalesce((SELECT jsonb_agg(snapshot_id) FROM api_cloud_snapshots WHERE account_id=own AND provider IN('box','boxd') AND state IN('deleting','deleted')),'[]'::jsonb));
 END $function$;
 $definition$,schema_name,schema_name);
 EXECUTE format('REVOKE ALL ON FUNCTION %I.zuse_rust_admission_read_batch(jsonb) FROM PUBLIC',schema_name);
 app_role:=CASE schema_name WHEN 'zuse_staging' THEN 'zuse_cloud_staging' WHEN 'zuse_production' THEN 'zuse_cloud_production' END;
 IF app_role IS NOT NULL AND EXISTS(SELECT 1 FROM pg_roles WHERE rolname=app_role) THEN
  EXECUTE format('GRANT EXECUTE ON FUNCTION %I.zuse_rust_admission_read_batch(jsonb) TO %I',schema_name,app_role);
 END IF;
END $install$;
