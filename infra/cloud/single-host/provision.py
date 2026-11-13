#!/usr/bin/env python3
"""Provision private external DB schemas/roles and fresh deployment keys on the host."""
import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
from urllib.parse import quote, urlencode, urlsplit

HERE = Path(__file__).resolve().parent

def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result

DB = module("cloud_database", HERE.parent / "database.py")
DEPLOY = module("cloud_deployment", HERE / "deploy.py")


def update(path, replacements):
    values = DEPLOY.read_env(path)
    lines = path.read_text().splitlines()
    for key, value in replacements.items():
        if "\n" in value or "\r" in value:
            raise ValueError("Provisioned values must be single-line")
        if key in values:
            lines = [f"{key}={value}" if line.startswith(key + "=") else line for line in lines]
        else:
            lines.append(f"{key}={value}")
    fd, temporary = tempfile.mkstemp(dir=path.parent, prefix=".provision-")
    try:
        with os.fdopen(fd, "w") as target:
            target.write("\n".join(lines) + "\n")
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def signing_key():
    private = subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-outform", "DER"],
                             check=True, capture_output=True).stdout
    public = subprocess.run(["openssl", "pkey", "-inform", "DER", "-pubout", "-outform", "DER"],
                            input=private, check=True, capture_output=True).stdout
    if len(private) != 48 or len(public) != 44 or not private.startswith(bytes.fromhex("302e020100300506032b657004220420")):
        raise ValueError("Unexpected Ed25519 key encoding")
    encode = lambda raw: base64.urlsafe_b64encode(raw).decode().rstrip("=")
    return json.dumps({"kty": "OKP", "crv": "Ed25519", "x": encode(public[-32:]),
                       "d": encode(private[-32:])}, separators=(",", ":"))


def role_sql(environment, database_name, password, create):
    # Names derive only from the closed environment set, never customer input.
    schema, role = DB.SCHEMAS[environment], f"zuse_cloud_{environment}"
    identifier = lambda value: '"' + value.replace('"', '""') + '"'
    statements = ["BEGIN;", "SET LOCAL statement_timeout='30s';"]
    if create:
        statements.append(f"CREATE ROLE {identifier(role)} LOGIN PASSWORD {DB.literal(password)} NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS CONNECTION LIMIT 24;")
    statements += [f"GRANT CONNECT ON DATABASE {identifier(database_name)} TO {identifier(role)};",
                   f"GRANT USAGE ON SCHEMA {identifier(schema)} TO {identifier(role)};",
                   f"REVOKE CREATE ON SCHEMA {identifier(schema)} FROM {identifier(role)};",
                   f"ALTER ROLE {identifier(role)} IN DATABASE {identifier(database_name)} SET search_path TO {identifier(schema)},pg_catalog;",
                   f"DO $grants$ DECLARE item record; BEGIN FOR item IN SELECT tablename FROM pg_tables WHERE schemaname={DB.literal(schema)} AND (tablename LIKE 'api\\_%' ESCAPE '\\' OR tablename LIKE 'zuse\\_rust\\_%' ESCAPE '\\' OR tablename LIKE 'cloud\\_%' ESCAPE '\\' OR tablename LIKE 'brokeboy\\_%' ESCAPE '\\' OR tablename LIKE 'zuse\\_brokeboy\\_%' ESCAPE '\\') LOOP EXECUTE format('GRANT SELECT,INSERT,UPDATE,DELETE ON TABLE %I.%I TO %I',{DB.literal(schema)},item.tablename,{DB.literal(role)}); END LOOP; END $grants$;",
                   f"REVOKE INSERT,UPDATE,DELETE ON {identifier(schema)}.cloud_managed_bases FROM {identifier(role)};",
                   f"DO $release_grant$ BEGIN IF to_regclass({DB.literal(schema+'.zuse_rust_runtime_releases')}) IS NOT NULL THEN EXECUTE 'REVOKE INSERT,UPDATE,DELETE ON {identifier(schema)}.zuse_rust_runtime_releases FROM {identifier(role)}'; END IF; END $release_grant$;",
                   f"DO $cache_grant$ BEGIN IF to_regclass({DB.literal(schema+'.cloud_project_cache_capabilities')}) IS NOT NULL THEN EXECUTE 'REVOKE INSERT,UPDATE,DELETE ON {identifier(schema)}.cloud_project_cache_capabilities FROM {identifier(role)}'; END IF; END $cache_grant$;",
                   f"DO $lock_grant$ BEGIN IF to_regprocedure({DB.literal(schema+'.zuse_rust_managed_base_admission_lock(text,text)')}) IS NOT NULL THEN EXECUTE 'GRANT EXECUTE ON FUNCTION {identifier(schema)}.zuse_rust_managed_base_admission_lock(text,text) TO {identifier(role)}'; END IF; END $lock_grant$;",
                   f"DO $cache_lock_grant$ BEGIN IF to_regprocedure({DB.literal(schema+'.zuse_rust_project_cache_capability_lock(text,text)')}) IS NOT NULL THEN EXECUTE 'GRANT EXECUTE ON FUNCTION {identifier(schema)}.zuse_rust_project_cache_capability_lock(text,text) TO {identifier(role)}'; END IF; END $cache_lock_grant$;",
                   f"DO $batch_grant$ BEGIN IF to_regprocedure({DB.literal(schema+'.zuse_rust_admission_read_batch(jsonb)')}) IS NOT NULL THEN EXECUTE 'GRANT EXECUTE ON FUNCTION {identifier(schema)}.zuse_rust_admission_read_batch(jsonb) TO {identifier(role)}'; END IF; END $batch_grant$;",
                   f"GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA {identifier(schema)} TO {identifier(role)};", "COMMIT;"]
    return "\n".join(statements)


def provision(args):
    edge = DEPLOY.read_env(args.config_root / "edge.env")
    password = args.admin_password_file.read_text().strip()
    if not password:
        raise ValueError("Missing private database password")
    authority = f"{quote(args.database_user, safe='')}:{quote(password, safe='')}@{args.database_host}:5432/{quote(args.database_name, safe='')}"
    admin = "postgresql://" + authority + "?" + urlencode({"sslmode": "verify-full", "sslrootcert": str(args.database_ca_file)})
    result = []
    for environment in DEPLOY.ENVIRONMENTS:
        schema, role = DB.SCHEMAS[environment], f"zuse_cloud_{environment}"
        directory = args.config_root / environment
        runtime_file = directory / "runtime.env"
        runtime = DEPLOY.read_env(runtime_file)
        # An existing application URL belongs to the operator: never replace it silently.
        if runtime.get("ZUSE_CLOUD_DATABASE_URL"):
            if not DEPLOY.isolated_schema_url(runtime["ZUSE_CLOUD_DATABASE_URL"], environment):
                raise ValueError("Existing database configuration needs explicit migration")
            result.append({"environment": environment, "status": "already-configured"})
            continue
        applied = DB.execute(admin, DB.render(environment))[-1]
        secret_file = directory / "database-password"
        if not secret_file.exists():
            DEPLOY.private_write(secret_file, secrets.token_urlsafe(36))
        application_password = secret_file.read_text().strip()
        exists = DB.execute(admin, f"SELECT json_build_object('exists',EXISTS(SELECT 1 FROM pg_roles WHERE rolname={DB.literal(role)}));")[-1]["exists"]
        local_query = {"sslmode": "verify-full", "sslrootcert": str(args.database_ca_file)}
        application_base = f"postgresql://{role}:{quote(application_password, safe='')}@{args.database_host}:5432/{quote(args.database_name, safe='')}"
        if exists:
            # A retry can recover our own prior role, but cannot rotate/take over another login.
            DB.execute(application_base + "?" + urlencode(local_query), "SELECT json_build_object('connected',true);")
        DB.execute(admin, role_sql(environment, args.database_name, application_password, not exists))
        local = application_base + "?" + urlencode(local_query)
        verification = DB.execute(local, f"SELECT json_build_object('role',current_user,'schema',current_schema(),'otherSchemaAccessible',has_schema_privilege(current_user,{DB.literal(DB.SCHEMAS['production' if environment == 'staging' else 'staging'])},'USAGE'));") if environment == "production" else DB.execute(local, "SELECT json_build_object('role',current_user,'schema',current_schema());")
        if verification[-1]["schema"] != schema or verification[-1].get("otherSchemaAccessible"):
            raise ValueError("Database role isolation verification failed")
        container_query = {"sslmode": "verify-full", "sslrootcert": "/etc/zuse/cloud/database-ca.crt",
                           "options": f"-csearch_path={schema},pg_catalog"}
        changes = {"ZUSE_CLOUD_DATABASE_URL": application_base + "?" + urlencode(container_query),
                   "API_ISSUER": "https://" + edge["PRODUCTION_DOMAIN" if environment == "production" else "STAGING_DOMAIN"],
                   "WORKOS_CLIENT_ID": args.workos_client_id,
                   "ZUSE_CLOUD_ALLOWED_ORIGINS": args.allowed_origins}
        if not runtime.get("CLOUD_DATA_ENCRYPTION_KEY"):
            changes["CLOUD_DATA_ENCRYPTION_KEY"] = base64.urlsafe_b64encode(secrets.token_bytes(32)).decode().rstrip("=")
        if not runtime.get("RELAY_MINT_PRIVATE_JWK"):
            changes["RELAY_MINT_PRIVATE_JWK"] = signing_key()
        update(runtime_file, changes)
        update(directory / "deploy.env", {"ZUSE_DATABASE_CA_FILE": str(args.database_ca_file)})
        result.append({"environment": environment, "status": "provisioned", "schema": schema, "migrationCount": applied["migrationCount"]})
    # Verify BOTH cross-schema boundaries after both schemas exist.
    for environment in DEPLOY.ENVIRONMENTS:
        runtime = DEPLOY.read_env(args.config_root / environment / "runtime.env")
        raw = runtime["ZUSE_CLOUD_DATABASE_URL"].replace(quote("/etc/zuse/cloud/database-ca.crt", safe=""), quote(str(args.database_ca_file), safe=""))
        other = DB.SCHEMAS["production" if environment == "staging" else "staging"]
        checked = DB.execute(raw, f"SELECT json_build_object('otherAccessible',has_schema_privilege(current_user,{DB.literal(other)},'USAGE'),'ledgerWritable',has_table_privilege(current_user,'zuse_cloud_schema_migrations','UPDATE'));")[-1]
        if checked["otherAccessible"] or checked["ledgerWritable"]:
            raise ValueError("Application role permissions are too broad")
    print(json.dumps({"environments": result, "crossEnvironmentIsolation": True, "adminPasswordExported": False}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config-root", type=Path, default=Path("/etc/zuse/cloud"))
    parser.add_argument("--admin-password-file", type=Path, required=True)
    parser.add_argument("--database-host", required=True)
    parser.add_argument("--database-name", default="postgres")
    parser.add_argument("--database-user", default="postgres")
    parser.add_argument("--database-ca-file", type=Path, required=True)
    parser.add_argument("--workos-client-id", default="client_01KWGQ818571ARFATQ3G9AR2Y2")
    parser.add_argument("--allowed-origins", default="https://zuse.sh,https://app.zuse.sh")
    args = parser.parse_args()
    DEPLOY.domain(args.database_host)
    if not args.workos_client_id.startswith("client_"):
        raise ValueError("Invalid WorkOS public client ID")
    provision(args)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.SubprocessError):
        # DB errors and captured openssl output must never disclose credentials.
        raise SystemExit("Private provisioning failed; existing credentials were not printed") from None
