#!/usr/bin/env python3
"""Prepare an explicit staging-only optional0028 rollout. Default has no DB/host effects.

The reviewed plan is bound to local source, an exact base and an exclusive case.
This tool never enables a service, issues credentials, certifies a base, allocates
compute or deletes provider resources. Preparation/cleanup uses the actual worker.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
MIGRATION = "0028_project_environment_caches.sql"
TABLES = ("cloud_project_cache_capabilities", "zuse_rust_project_preparation_requests",
          "zuse_rust_project_cache_entries", "zuse_rust_project_cache_operations",
          "zuse_rust_project_preparation_keys", "zuse_rust_project_source_bundles",
          "zuse_rust_project_cache_resources")

def module(name):
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("cache_rollout_" + name, HERE / (name + ".py"))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result

SECURE = module("project-cache-certification")
DB = module("database")
require = SECURE.require
sha = SECURE.digest


def bounds(value):
    SECURE.exact(value, {"version", "caseId", "ownerId", "projectId", "protectedWorkspaceId",
        "protectedMachineId", "protectedGeneration", "protectedRetainedSha256", "apiSha256",
        "maxActiveMachines", "maxOwnedMachines", "maxDurationSeconds", "cleanupReserveSeconds"}, "rollout_bounds")
    require(type(value["version"]) is int and value["version"] == 1, "rollout_bounds")
    for key in ("caseId", "ownerId", "projectId", "protectedWorkspaceId", "protectedMachineId"):
        require(SECURE.identifier(value[key]), "rollout_identity")
    require(value["projectId"] != value["protectedWorkspaceId"], "rollout_identity")
    require(type(value["protectedGeneration"]) is int and value["protectedGeneration"] == 1,
            "rollout_protection")
    for key in ("protectedRetainedSha256", "apiSha256"):
        require(SECURE.is_hash(value[key]), "rollout_protection")
    require(type(value["maxActiveMachines"]) is int and value["maxActiveMachines"] == 2
        and type(value["maxOwnedMachines"]) is int and value["maxOwnedMachines"] == 1,
        "rollout_resource_cap")
    require(type(value["maxDurationSeconds"]) is int and 300 <= value["maxDurationSeconds"] <= 1800
        and type(value["cleanupReserveSeconds"]) is int
        and 120 <= value["cleanupReserveSeconds"] <= value["maxDurationSeconds"] // 2,
        "rollout_deadline")
    return value


def source_pins():
    paths = [HERE / "project-cache-rollout.py", HERE / "database.py",
             HERE / "project-cache-certification.py", ROOT / "crates/cloud/migrations" / MIGRATION]
    return {str(path.relative_to(ROOT)): sha(path.read_bytes()) for path in paths}


def plan(release_path, bounds_path, pilot_path=None, base_pin_path=None, allow_expired=False):
    release = SECURE.load(release_path)
    module("managed-base-release").validate_release(release)
    require(release["provider"] == "boxd" and release.get("initialRuntimeBootstrap"), "rollout_base")
    spec = bounds(SECURE.load(bounds_path))
    pilot_binding = None
    if pilot_path or base_pin_path:
        require(pilot_path and base_pin_path, "rollout_complete_pilot_binding_required")
        pilot = SECURE.load(pilot_path)
        pin = SECURE.load(base_pin_path)
        fields = ("version","pilotId","ownerId","actorId","projectId","repositoryIdentity","candidateSha256","baseReleaseId","baseArtifactId","baseArtifactVersion","runtimeSha256","architecture","expiresAt","protectedWorkspaceId","protectedMachineId","maxActiveMachines","maxOwnedMachines","inputs","mechanismCapability")
        SECURE.exact(pilot, fields, "rollout_pilot_fields")
        require(pilot["version"] == 1 and type(pilot["version"]) is int
            and pilot["ownerId"] == pilot["actorId"] == spec["ownerId"]
            and not pilot["ownerId"].startswith("organization:")
            and pilot["projectId"] == spec["projectId"]
            and pilot["protectedWorkspaceId"] == spec["protectedWorkspaceId"]
            and pilot["protectedMachineId"] == spec["protectedMachineId"]
            and pilot["maxActiveMachines"] == spec["maxActiveMachines"] == 2
            and pilot["maxOwnedMachines"] == spec["maxOwnedMachines"] == 1,
            "rollout_pilot_scope")
        require(pin.get("ownerId") == pilot["ownerId"] and pin.get("actorId",pilot["actorId"]) == pilot["actorId"]
            and pin.get("provider") == release["provider"] == "boxd"
            and pin.get("releaseId") == pilot["baseReleaseId"] == release["releaseId"]
            and pin.get("artifactId") == pilot["baseArtifactId"] == release["artifactId"]
            and pin.get("candidateSha256") == pilot["candidateSha256"] == sha(SECURE.canonical(release))
            and pin.get("manifestSha256") == sha(SECURE.canonical(release["manifest"]))
            and pilot["runtimeSha256"] == release["runtimeSha256"] == release["initialRuntimeBootstrap"]["runtimeSha256"]
            and pilot["architecture"] == release["architecture"]
            and SECURE.is_version(pilot["baseArtifactVersion"]), "rollout_pilot_base")
        now = int(time.time()*1000)
        require(type(pilot["expiresAt"]) is int and pilot["expiresAt"] > 0
            and (allow_expired or now < pilot["expiresAt"] <= now+1_800_000)
            and pilot["expiresAt"] <= pin["expiresAt"], "rollout_pilot_expiry")
        require(isinstance(pilot["inputs"],list) and 1 <= len(pilot["inputs"]) <= 2, "rollout_pilot_inputs")
        typed_inputs = []
        for item in pilot["inputs"]:
            SECURE.exact(item, ("recipeDigest","recipeSource","sourceCommit","dependencyDigest"), "rollout_pilot_input")
            require(SECURE.is_hash(item["recipeDigest"]) and SECURE.is_hash(item["dependencyDigest"])
                and item["recipeSource"] in ("project","repository")
                and isinstance(item["sourceCommit"],str) and re.fullmatch('[a-f0-9]{40}|[a-f0-9]{64}',item["sourceCommit"]), "rollout_pilot_input")
            typed_inputs.append({key:item[key] for key in ("recipeDigest","recipeSource","sourceCommit","dependencyDigest")})
        cap = pilot["mechanismCapability"]
        SECURE.exact(cap,SECURE.CAP_KEYS,"rollout_pilot_capability")
        require(cap == dict(version=1,runtimeSha256=pilot["runtimeSha256"],environmentCacheVersion=1,
            secretFreeSourceBundleVersion=1,retainedTaskStorageAbsentRequired=True,
            credentialFreePreparationRequired=True,preparationCaptureVersion=1)
            and all(type(cap[key]) is int for key in ("version","environmentCacheVersion","secretFreeSourceBundleVersion","preparationCaptureVersion"))
            and all(cap[key] is True for key in ("retainedTaskStorageAbsentRequired","credentialFreePreparationRequired")), "rollout_pilot_capability")
        typed = {key:pilot[key] for key in fields}
        typed["inputs"] = typed_inputs
        typed["mechanismCapability"] = {key:cap[key] for key in ("version","runtimeSha256","environmentCacheVersion","secretFreeSourceBundleVersion","retainedTaskStorageAbsentRequired","credentialFreePreparationRequired","preparationCaptureVersion")}
        fingerprint = sha(json.dumps(typed,separators=(',',':'),ensure_ascii=False).encode())
        pilot_binding = {"configuration":pilot,"fingerprint":fingerprint,
            "configurationSha256":sha(SECURE.read(pilot_path)),"basePinSha256":sha(SECURE.read(base_pin_path))}
    value = {"version": 1, "kind": "staging-project-cache-rollout-v1", "environment": "staging",
        "schema": "zuse_staging", "provider": "boxd", "releaseId": release["releaseId"],
        "baseArtifactId": release["artifactId"], "runtimeSha256": release["initialRuntimeBootstrap"]["runtimeSha256"],
        "releaseSha256": sha(SECURE.read(release_path)), "bounds": spec,
        "sourceSha256": source_pins(), "migration": {"name": MIGRATION,
            "sha256": sha((DB.MIGRATIONS / MIGRATION).read_bytes())},
        "baseMigrations": DB.plan("staging")["migrations"],
        "workerEnvironmentOverlay": {"ZUSE_CLOUD_PROJECT_PREPARATION_ENABLED": "1",
            "ZUSE_CLOUD_PROJECT_PREPARATION_BACKGROUND": "0"},
        "workerCommand": ["zuse-cloud-api", "project-preparation-worker", "--once"],
        "requiredLivePreflight": ["exact API/worker binary and clean runtime pins",
            "same protected original, generation, initial-message never-leased and retained ciphertext hash",
            "exclusive provider lane; every configured provider inventory; protected original plus at most one case-owned VM; unrelated resource baselines unchanged",
            "no due preparation outside exact canary project/request before every awaited tick",
            "builder independently absent before verifier; no new task until preparation VMs absent",
            "full base certification and separate cache certificate remain operator-only"],
        "defaultHasEffects": False, "certifiesBaseOrCache": False, "changesFlagsOrServices": False}
    value["baseAdmission"] = "staging-pilot" if pilot_binding else "verified"
    if pilot_binding:
        value["pilot"] = pilot_binding
        value["workerEnvironmentOverlay"]["ZUSE_CLOUD_STAGING_PROJECT_CACHE_PILOT"] = json.dumps(pilot_binding["configuration"],separators=(',',':'))
    return value


def migration_sql(value, release):
    require(value["sourceSha256"] == source_pins(), "rollout_source_changed")
    literal = DB.literal
    expected = {item["name"]: item["sha256"] for item in value["baseMigrations"]}
    exact_rows = " OR ".join("(name=" + literal(name) + " AND sha256=" + literal(checksum) + ")"
                             for name, checksum in expected.items())
    migration = (DB.MIGRATIONS / MIGRATION).read_text()
    require(sha(migration.encode()) == value["migration"]["sha256"], "rollout_source_changed")
    release_text = SECURE.canonical(release).decode()
    tag = "$rollout_" + sha(SECURE.canonical(value)) + "$"
    if value.get("baseAdmission") == "staging-pilot":
        base_condition = "state='preparing' AND runtime_manifest IS NULL AND candidate_manifest=" + literal(release_text) + "::jsonb"
    else:
        base_condition = "state='verified' AND runtime_manifest=" + literal(release_text) + "::jsonb"
    guard = f"""DO {tag} BEGIN
IF current_schema()<>'zuse_staging' OR NOT EXISTS(SELECT 1 FROM zuse_cloud_schema_identity WHERE singleton AND environment='staging') THEN RAISE EXCEPTION 'project_cache_rollout_staging_required'; END IF;
IF current_user IN ('zuse_cloud_staging','zuse_cloud_production') THEN RAISE EXCEPTION 'project_cache_rollout_operator_required'; END IF;
IF (SELECT count(*) FROM zuse_cloud_schema_migrations WHERE {exact_rows})<>{len(expected)} OR EXISTS(SELECT 1 FROM zuse_cloud_schema_migrations WHERE NOT ({exact_rows}) AND NOT(name={literal(MIGRATION)} AND sha256={literal(value['migration']['sha256'])})) THEN RAISE EXCEPTION 'project_cache_rollout_base_ledger_changed'; END IF;
PERFORM 1 FROM cloud_managed_bases WHERE provider='boxd' AND release_id={literal(value['releaseId'])} AND artifact_id={literal(value['baseArtifactId'])} AND {base_condition} FOR SHARE;
IF NOT FOUND THEN RAISE EXCEPTION 'project_cache_rollout_verified_base_required'; END IF;
IF NOT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id={literal(value['bounds']['protectedWorkspaceId'])} AND provider_sandbox_id={literal(value['bounds']['protectedMachineId'])} AND request_config->>'runtimeGeneration'='1' AND state NOT IN('deleting','deleted')) THEN RAISE EXCEPTION 'project_cache_rollout_original_changed'; END IF;
END {tag};"""
    statements = ["BEGIN; SET LOCAL search_path=zuse_staging,pg_catalog;",
        "SET LOCAL lock_timeout='5s'; SET LOCAL statement_timeout='60s';",
        "SELECT pg_advisory_xact_lock(hashtextextended(current_database()||':zuse_staging',0));", guard,
        f"SELECT {literal(migration)} WHERE NOT EXISTS(SELECT 1 FROM zuse_cloud_schema_migrations WHERE name={literal(MIGRATION)})\n\\gexec",
        f"INSERT INTO zuse_cloud_schema_migrations(name,sha256) VALUES({literal(MIGRATION)},{literal(value['migration']['sha256'])}) ON CONFLICT DO NOTHING;"]
    # Exactly these optional tables; no broad privilege refresh, role/password or
    # production changes. Capability publication stays operator-only.
    for table in TABLES:
        privileges = "SELECT" if table == "cloud_project_cache_capabilities" else "SELECT,INSERT,UPDATE,DELETE"
        statements.append(f"GRANT {privileges} ON zuse_staging.{table} TO zuse_cloud_staging;")
    statements += ["REVOKE INSERT,UPDATE,DELETE ON zuse_staging.cloud_project_cache_capabilities FROM zuse_cloud_staging;",
        "GRANT EXECUTE ON FUNCTION zuse_staging.zuse_rust_project_cache_capability_lock(text,text) TO zuse_cloud_staging;",
        "SELECT json_build_object('status','optional-schema-installed','schema',current_schema(),'migration','" + MIGRATION + "');", "COMMIT;"]
    return "\n".join(statements)


def apply(plan_path, approved, release_path, bounds_path, url_file, receipt, pilot_path=None, base_pin_path=None):
    raw = SECURE.read(plan_path)
    require(sha(raw) == approved, "rollout_approved_plan_required")
    value = SECURE.decode(raw)
    require(value == plan(release_path, bounds_path, pilot_path, base_pin_path), "rollout_plan_changed")
    receipt = Path(receipt)
    intent = Path(str(receipt) + ".intent.json")
    require(not receipt.exists() and not intent.exists(), "rollout_inspect_original")
    statement = migration_sql(value, SECURE.load(release_path))
    SECURE.write_new(intent, {"version": 1, "planSha256": approved, "statementSha256": sha(statement.encode())})
    env = DB.connection_environment(SECURE.read(url_file, 16384).decode().strip(), inherited={})
    result = subprocess.run(["psql", "-X", "--no-password", "-Atq", "-v", "ON_ERROR_STOP=1"],
        input=statement, env=env, text=True, capture_output=True, timeout=90)
    require(result.returncode == 0, "rollout_database_rejected_inspect_original")
    rows = [SECURE.decode(line.encode()) for line in result.stdout.splitlines() if line.startswith("{")]
    require(rows and rows[-1].get("status") == "optional-schema-installed", "rollout_receipt_invalid")
    SECURE.write_new(receipt, {"version": 1, "status": "optional-schema-installed", "planSha256": approved,
        "migration": value["migration"], "providerEffects": False, "hostOrFlagEffects": False})


def inspect(plan_path, approved, release_path, bounds_path, url_file, receipt, pilot_path=None, base_pin_path=None):
    raw = SECURE.read(plan_path)
    require(sha(raw) == approved, "rollout_approved_plan_required")
    value = SECURE.decode(raw)
    require(value == plan(release_path, bounds_path, pilot_path, base_pin_path, allow_expired=True), "rollout_plan_changed")
    original = SECURE.load(str(receipt) + ".intent.json")
    require(original == {"version": 1, "planSha256": approved,
        "statementSha256": sha(migration_sql(value, SECURE.load(release_path)).encode())}, "rollout_original_intent_changed")
    sql = "BEGIN READ ONLY; SET LOCAL search_path=zuse_staging,pg_catalog; SET LOCAL statement_timeout='10s';"
    sql += "SELECT json_build_object('schema',current_schema(),'environment',(SELECT environment FROM zuse_cloud_schema_identity WHERE singleton),'installed',EXISTS(SELECT 1 FROM zuse_cloud_schema_migrations WHERE name=" + DB.literal(MIGRATION) + " AND sha256=" + DB.literal(value['migration']['sha256']) + ")); COMMIT;"
    rows = DB.execute(SECURE.read(url_file, 16384).decode().strip(), sql)
    require(rows and rows[-1].get("schema") == "zuse_staging" and rows[-1].get("environment") == "staging", "rollout_inspection_invalid")
    return {"status": "original-schema-commit-observed" if rows[-1]["installed"] else "original-schema-not-observed",
        "providerEffects": False, "writes": False, "neverRetryApply": True}


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--bounds", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--pilot", type=Path)
    parser.add_argument("--base-pin", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply-schema", action="store_true")
    mode.add_argument("--inspect-schema", action="store_true")
    parser.add_argument("--approved-plan-sha256")
    parser.add_argument("--database-url-file", type=Path)
    parser.add_argument("--receipt", type=Path)
    args = parser.parse_args()
    if args.apply_schema or args.inspect_schema:
        require(args.approved_plan_sha256 and args.database_url_file and args.receipt, "rollout_explicit_apply_required")
        if args.inspect_schema:
            print(json.dumps(inspect(args.plan, args.approved_plan_sha256, args.release, args.bounds, args.database_url_file, args.receipt, args.pilot, args.base_pin)))
        else:
            apply(args.plan, args.approved_plan_sha256, args.release, args.bounds, args.database_url_file, args.receipt, args.pilot, args.base_pin)
            print(json.dumps({"status": "optional-schema-installed", "providerEffects": False}))
    else:
        require(not args.approved_plan_sha256 and not args.database_url_file and not args.receipt,
                "rollout_plan_never_reads_credentials")
        value = plan(args.release, args.bounds, args.pilot, args.base_pin)
        SECURE.write_new(args.plan, value)
        print(json.dumps({"status": "plan-only", "planSha256": sha(SECURE.read(args.plan)), "effects": False}))


if __name__ == "__main__":
    try:
        main()
    except (SECURE.Rejected, ValueError, OSError, subprocess.SubprocessError):
        raise SystemExit("project_cache_rollout_rejected_or_inspect_original") from None
