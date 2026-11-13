#!/usr/bin/env python3
"""Transactional bootstrap for a private standalone cloud schema; no legacy data import."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import ssl
import subprocess
import urllib.parse

MIGRATIONS = Path(__file__).resolve().parents[2] / "crates/cloud/migrations"
SCHEMAS = {"staging": "zuse_staging", "production": "zuse_production"}
FILES = ("0000_application.sql", "0001_control_plane.sql", "0002_gateway_leases.sql",
         "0003_command_mailbox.sql", "0004_allocations.sql", "0005_lifecycle.sql",
         "0006_recovery.sql", "0007_images.sql", "0007_restore.sql",
         "0008_delete.sql", "0008_launch_assets.sql", "0009_computers.sql",
         "0010_managed_bases.sql", "0011_transcript_heads.sql", "0012_provider_receipts.sql",
         "0013_recovery_receipts.sql", "0014_restore_receipts.sql", "0015_github.sql",
         "0016_task_history.sql", "0017_account_events.sql", "0018_github_joining_jobs.sql", "0019_workload_secrets.sql",
         "0020_staging_managed_base_candidates.sql", "0021_runtime_repairs.sql", "0023_managed_base_admission_lock.sql", "0024_credential_delivery_journal.sql", "0025_admission_read_batch.sql", "0026_task_command_admission.sql", "0027_computer_recreations.sql", "0031_machine_images.sql", "0033_prized_provider.sql", "0034_runtime_releases.sql", "0035_launch_asset_release.sql")


# Accepted only when separately installed by its explicit operator workflow.
# Ordinary bootstrap never executes or backfills an optional migration.
OPTIONAL_FILES = ("0028_project_environment_caches.sql", "0029_runtime_statement_batch.sql", "0030_transcript_write_intents.sql", "0032_brokeboy.sql", "0033_shared_credits.sql")


def optional_migrations(root=MIGRATIONS):
    result = []
    for name in OPTIONAL_FILES:
        path = root / name
        if path.is_file():
            raw = path.read_bytes()
            result.append({"name": name, "sha256": hashlib.sha256(raw).hexdigest(), "sql": raw.decode()})
    return result


def manifest_matches(environment, actual, root=MIGRATIONS):
    if not isinstance(actual, list):
        return False
    expected = {item["name"]: item["sha256"] for item in plan(environment, root)["migrations"]}
    optional = {item["name"]: item["sha256"] for item in optional_migrations(root)}
    seen = set()
    for item in actual:
        if not isinstance(item, dict) or set(item) != {"name", "sha256"}:
            return False
        name = item["name"]
        if name in seen or name not in (expected | optional) or item["sha256"] != (expected | optional)[name]:
            return False
        seen.add(name)
    return set(expected) <= seen


def literal(value):
    return "'" + value.replace("'", "''") + "'"


def migrations(root=MIGRATIONS):
    result = []
    for name in FILES:
        raw = (root / name).read_bytes()
        sql = raw.decode()
        if re.search(r"(?m)^\s*\\|\bCONCURRENTLY\b", sql):
            raise ValueError("zuse_schema_nontransactional_migration")
        result.append({"name": name, "sha256": hashlib.sha256(raw).hexdigest(), "sql": sql})
    return result


def plan(environment, root=MIGRATIONS):
    return {"environment": environment, "schema": SCHEMAS[environment],
            "migrations": [{key: value for key, value in item.items() if key != "sql"}
                           for item in migrations(root)], "importsCustomerData": False}


def render(environment, root=MIGRATIONS):
    schema = SCHEMAS[environment]
    parts = ["BEGIN;", "SET LOCAL lock_timeout='15s';", "SET LOCAL statement_timeout='120s';",
             f"SELECT pg_advisory_xact_lock(hashtextextended(current_database() || ':{schema}', 0));",
             f'CREATE SCHEMA IF NOT EXISTS "{schema}";',
             f'SET LOCAL search_path TO "{schema}", pg_catalog;',
             """DO $guard$ BEGIN
IF to_regclass('zuse_cloud_schema_identity') IS NULL AND EXISTS (
 SELECT 1 FROM pg_tables WHERE schemaname=current_schema()
) THEN RAISE EXCEPTION 'zuse_schema_existing_tables_require_import_review'; END IF;
END $guard$;""",
             "CREATE TABLE IF NOT EXISTS zuse_cloud_schema_identity (singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton), environment text NOT NULL);",
             "CREATE TABLE IF NOT EXISTS zuse_cloud_schema_migrations (name text PRIMARY KEY, sha256 text NOT NULL, applied_at timestamptz NOT NULL DEFAULT clock_timestamp());",
             f"INSERT INTO zuse_cloud_schema_identity VALUES(true,{literal(environment)}) ON CONFLICT DO NOTHING;",
             f"DO $guard$ BEGIN IF NOT EXISTS (SELECT 1 FROM zuse_cloud_schema_identity WHERE environment={literal(environment)}) THEN RAISE EXCEPTION 'zuse_schema_environment_mismatch'; END IF; END $guard$;",
             f'REVOKE ALL ON SCHEMA "{schema}" FROM PUBLIC;']
    expected = migrations(root)
    optional = optional_migrations(root)
    names = ",".join(literal(item["name"]) for item in expected + optional)
    parts.append(f"DO $guard$ BEGIN IF EXISTS (SELECT 1 FROM zuse_cloud_schema_migrations WHERE name NOT IN ({names})) THEN RAISE EXCEPTION 'zuse_schema_unknown_migration'; END IF; END $guard$;")
    for item in optional:
        parts.append(f"DO $guard$ BEGIN IF EXISTS (SELECT 1 FROM zuse_cloud_schema_migrations WHERE name={literal(item['name'])} AND sha256<>{literal(item['sha256'])}) THEN RAISE EXCEPTION 'zuse_schema_checksum_mismatch'; END IF; END $guard$;")
    for item in expected:
        name, checksum = literal(item["name"]), literal(item["sha256"])
        parts += [f"DO $guard$ BEGIN IF EXISTS (SELECT 1 FROM zuse_cloud_schema_migrations WHERE name={name} AND sha256<>{checksum}) THEN RAISE EXCEPTION 'zuse_schema_checksum_mismatch'; END IF; END $guard$;",
                  f"SELECT {literal(item['sql'])} WHERE NOT EXISTS (SELECT 1 FROM zuse_cloud_schema_migrations WHERE name={name})\n\\gexec",
                  f"INSERT INTO zuse_cloud_schema_migrations(name,sha256) VALUES({name},{checksum}) ON CONFLICT DO NOTHING;"]
    parts += [f"SELECT json_build_object('environment',{literal(environment)},'schema',{literal(schema)},'migrationCount',(SELECT count(*) FROM zuse_cloud_schema_migrations),'applicationTableCount',(SELECT count(*) FROM pg_tables WHERE schemaname={literal(schema)} AND tablename LIKE 'api_%'));", "COMMIT;"]
    return "\n".join(parts) + "\n"


def connection_environment(raw, inherited=None):
    parsed = urllib.parse.urlsplit(raw)
    if parsed.scheme not in ("postgres", "postgresql") or not parsed.hostname or not parsed.username or not parsed.path.strip("/"):
        raise ValueError("zuse_schema_invalid_database_url")
    query = urllib.parse.parse_qs(parsed.query, strict_parsing=True)
    local = parsed.hostname in ("127.0.0.1", "localhost", "::1")
    mode = query.get("sslmode", ["verify-full"])[-1]
    if not local and mode != "verify-full":
        raise ValueError("zuse_schema_verified_tls_required")
    env = {key: value for key, value in (os.environ if inherited is None else inherited).items()
           if not key.startswith("PG") and key not in ("DATABASE_URL", "DATABASE_URL_PROD", "ZUSE_CLOUD_DATABASE_URL")}
    env.update(PGHOST=parsed.hostname, PGPORT=str(parsed.port or 5432),
               PGDATABASE=urllib.parse.unquote(parsed.path[1:]), PGUSER=urllib.parse.unquote(parsed.username),
               PGPASSWORD=urllib.parse.unquote(parsed.password or ""), PGSSLMODE=mode, PGCONNECT_TIMEOUT="10",
               PGAPPNAME="zuse-cloud-schema")
    if query.get("sslrootcert"):
        root = query["sslrootcert"][-1]
        if root == "system":
            # libpq 15 treats "system" as a literal file. Preserve verified TLS
            # using the platform trust bundle instead of weakening sslmode.
            root = ssl.get_default_verify_paths().cafile
            if not root or not Path(root).is_file():
                raise ValueError("zuse_schema_system_ca_unavailable")
        env["PGSSLROOTCERT"] = root
    return env


def execute(raw, sql):
    result = subprocess.run(["psql", "-X", "--no-password", "--no-align", "--tuples-only", "--quiet", "-v", "ON_ERROR_STOP=1"],
                            input=sql, env=connection_environment(raw), capture_output=True, text=True, timeout=180)
    if result.returncode:
        known = re.search(r"zuse_schema_[a-z_]+", result.stderr)
        raise ValueError(known.group() if known else "zuse_schema_database_operation_failed")
    return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]


def render_brokeboy(environment):
    """Install the optional engine only after verifying the managed schema ledger."""
    name = "0032_brokeboy.sql"
    schema = SCHEMAS[environment]
    base = plan(environment)["migrations"]
    permitted = base + optional_migrations()
    matches = " OR ".join(f"(name={literal(item['name'])} AND sha256={literal(item['sha256'])})" for item in permitted)
    required = " OR ".join(f"(name={literal(item['name'])} AND sha256={literal(item['sha256'])})" for item in base)
    selected=[item for item in permitted if item["name"] in (name,"0033_shared_credits.sql")]
    install="\n".join(f"SELECT {literal(item['sql'])} WHERE NOT EXISTS(SELECT 1 FROM zuse_cloud_schema_migrations WHERE name={literal(item['name'])})\n\\gexec\nINSERT INTO zuse_cloud_schema_migrations(name,sha256) VALUES({literal(item['name'])},{literal(item['sha256'])}) ON CONFLICT DO NOTHING;" for item in selected)
    return f'''BEGIN;
SET LOCAL search_path TO "{schema}", pg_catalog;
SET LOCAL lock_timeout='15s';
SET LOCAL statement_timeout='120s';
SELECT pg_advisory_xact_lock(hashtextextended(current_database() || {literal(':' + schema)},0));
DO $$ BEGIN
IF NOT EXISTS(SELECT 1 FROM zuse_cloud_schema_identity WHERE singleton AND environment={literal(environment)})
 OR (SELECT count(*) FROM zuse_cloud_schema_migrations WHERE {required})<>{len(base)}
 OR EXISTS(SELECT 1 FROM zuse_cloud_schema_migrations WHERE NOT ({matches}))
THEN RAISE EXCEPTION 'zuse_schema_manifest_mismatch'; END IF;
END $$;
{install}
DO $grants$ DECLARE item record; BEGIN
IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname={literal('zuse_cloud_' + environment)}) THEN
 FOR item IN SELECT tablename FROM pg_tables WHERE schemaname={literal(schema)} AND (tablename LIKE 'brokeboy\\_%' ESCAPE '\\' OR tablename LIKE 'zuse\\_brokeboy\\_%' ESCAPE '\\') LOOP
  EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.%I TO %I',{literal(schema)},item.tablename,{literal('zuse_cloud_' + environment)});
 END LOOP;
END IF;
END $grants$;
SELECT json_build_object('installed',true,'migration',{literal(name)});
COMMIT;'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--environment", choices=SCHEMAS, required=True)
    parser.add_argument("action", choices=("plan", "apply", "check", "apply-brokeboy"))
    args = parser.parse_args()
    if args.action == "plan":
        print(json.dumps(plan(args.environment), indent=2))
        return
    raw = os.environ.get("ZUSE_CLOUD_DATABASE_URL", "")
    if not raw:
        raise ValueError("zuse_schema_explicit_database_url_required")
    if args.action == "apply":
        result = execute(raw, render(args.environment))
    elif args.action == "apply-brokeboy":
        result = execute(raw, render_brokeboy(args.environment))
    else:
        schema = SCHEMAS[args.environment]
        sql = f'''BEGIN READ ONLY;
SET LOCAL search_path TO "{schema}", pg_catalog;
SELECT json_build_object('environment',environment,'schema',current_schema(),'migrations',(SELECT json_agg(json_build_object('name',name,'sha256',sha256) ORDER BY name) FROM zuse_cloud_schema_migrations)) FROM zuse_cloud_schema_identity;
COMMIT;'''
        result = execute(raw, sql)
        if not result or result[0]["environment"] != args.environment or not manifest_matches(args.environment, result[0]["migrations"]):
            raise ValueError("zuse_schema_manifest_mismatch")
    print(json.dumps(result[-1] if result else {"ok": True}))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        # SQL stderr and database URLs can contain credentials. Emit only known codes.
        code = str(error) if isinstance(error, ValueError) and re.fullmatch(r"zuse_schema_[a-z_]+", str(error)) else "zuse_schema_operation_failed"
        raise SystemExit(code) from None
