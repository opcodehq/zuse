#!/usr/bin/env python3
"""Offline plan, read-only legacy counts, and isolated synthetic migration rehearsal.

The rehearsal always creates its own temporary localhost PostgreSQL cluster. It
never accepts a deployment database as its write target or imports customer data.
"""
import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import socket
import stat
import subprocess
import tempfile
import urllib.parse
import uuid

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("cloud_database", Path(__file__).with_name("database.py"))
database = importlib.util.module_from_spec(spec)
spec.loader.exec_module(database)
REFERENCE = "a0155d2dd11f07495a99d3737c9bd40a5c8b4508"
TABLES = (
    "api_stripe_customers", "api_entitlements", "api_environments", "api_environment_credentials", "api_devices", "api_agent_activity",
    "api_cloud_projects", "api_cloud_project_builds", "api_cloud_workspaces",
    "api_cloud_transcript_checkpoints", "api_cloud_workspace_api_messages",
    "api_cloud_workspace_command_receipts", "api_cloud_billing_periods",
    "api_cloud_billing_ledger", "api_cloud_auth_authorities", "api_model_connections", "api_cloud_github_users", "api_github_identities",
    "api_github_join_policies", "api_github_enrollments", "api_cloud_snapshots",
    "api_machines", "api_master_ownership", "api_workspace_settings",
    "api_cloud_github_installations", "api_cloud_workspace_api_turn_receipts",
    "api_cloud_workspace_runtime_summaries", "api_cloud_workspace_usage",
    "api_cloud_billing_usage", "api_cloud_billing_reservations",
    "api_provider_usage_events", "api_provider_lifecycle_pairs",
    "api_provider_event_finalizations", "api_provider_event_deliveries",
    "api_billing_events", "api_provider_price_schedule", "api_snapshot_price_schedule",
    "api_cloud_provider_connections", "api_cloud_billing_meter_reconciliation_attempts",
    "api_cloud_billing_meter_reconciliations", "api_platform_costs", "api_provider_statement_totals",
)
# Preserved as opaque inactive rows, never inserted into active worker tables.
QUARANTINE = ('api_api_keys', 'api_api_webhook_deliveries', 'api_api_webhooks', 'api_cloud_billing_cap_commands', 'api_cloud_billing_outbox', 'api_cloud_catalog_changes', 'api_cloud_catalog_heads', 'api_cloud_runtime_observations', 'api_cloud_snapshot_leases', 'api_cloud_usage_outbox', 'api_cloud_workspace_launch_intents', 'api_dpop_proofs', 'api_github_org_members', 'api_link_challenges', 'api_model_connection_leases', 'api_organization_domain_enrollments', 'api_organization_domains', 'api_revoked_dpop_keys', 'api_slack_installations', 'api_slack_members', 'api_slack_sessions', 'api_slack_state', 'api_stripe_meter_deliveries', 'api_stripe_subscription_migrations')

EXCLUDED = ("active runtime credentials/leases", "pending provider mutations", "native control ownership", "payment delivery outboxes")
REQUIRED = (
    "Approved consistent legacy export or private read-only connection path",
    "Original encryption/wrapped-key inventory and an authorized key migration path",
    "Legacy object-store inventory, immutable object checksums, and read access",
    "Legacy writer shutdown/role-fencing procedure with an accountable handoff owner",
    "Verified TypeScript guest/session to Rust storage conversion before retained execution",
)


def table_inventory():
    baseline = (database.MIGRATIONS / "0000_application.sql").read_text()
    actual = set(re.findall(r'CREATE TABLE\s+"(api_[a-z0-9_]+)"', baseline))
    if set(TABLES) & set(QUARANTINE) or actual != set(TABLES) | set(QUARANTINE):
        raise ValueError("zuse_rehearsal_unclassified_source_table")
    return {"retained": list(TABLES), "quarantined": list(QUARANTINE), "sourceTableCount": len(actual)}


def plan():
    inventory = table_inventory()
    return {"sourceReference": REFERENCE, "environment": "staging", "mode": "synthetic-rehearsal",
            "tables": list(TABLES), "sourceTableInventory": inventory, "excludedFromAutomaticHandoff": list(EXCLUDED),
            "preserve": ["stable account/task/repository/financial identities", "opaque ciphertext and wrapped-key metadata",
                         "history/tool receipts", "machine/snapshot/archive references", "object checksums", "sequence high-water marks"],
            "productionWritesAllowed": False, "customerCutoverAllowed": False, "nativeGuestExecutionAdmitted": False,
            "realRetainedDatasetVerified": False, "externalPrerequisites": list(REQUIRED)}


def ident(value):
    if not re.fullmatch(r"[a-z][a-z0-9_]{0,62}", value):
        raise ValueError("zuse_rehearsal_invalid_identifier")
    return '"' + value + '"'


def execute(url, sql):
    result = subprocess.run(["psql", "-X", "--no-password", "--no-align", "--tuples-only", "--quiet", "-v", "ON_ERROR_STOP=1"],
                            input=sql, env=database.connection_environment(url), capture_output=True, text=True, timeout=180)
    if result.returncode:
        known = re.search(r"zuse_rehearsal_[a-z_]+", result.stderr)
        raise ValueError(known.group() if known else "zuse_rehearsal_database_failure")
    return [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]


@contextlib.contextmanager
def owned_cluster():
    if not all(shutil.which(binary) for binary in ("initdb", "pg_ctl", "psql")):
        raise ValueError("zuse_rehearsal_postgres_tools_required")
    with tempfile.TemporaryDirectory(prefix="zuse-migration-fixture-") as temporary:
        path = Path(temporary)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        env = {k: v for k, v in os.environ.items() if k in ("PATH", "LANG", "TZ", "TMPDIR") or k.startswith("LC_")}
        subprocess.run(["initdb", "-D", str(path / "data"), "--auth=trust", "--username=zuse_rehearsal", "--no-locale"],
                       env=env, capture_output=True, check=True)
        subprocess.run(["pg_ctl", "-D", str(path / "data"), "-l", str(path / "server.log"), "-o",
                        f"-h 127.0.0.1 -p {port} -k {path}", "-w", "start"], env=env, capture_output=True, check=True)
        try:
            yield f"postgresql://zuse_rehearsal@127.0.0.1:{port}/postgres?sslmode=disable", path
        finally:
            subprocess.run(["pg_ctl", "-D", str(path / "data"), "-m", "fast", "-w", "stop"], env=env, capture_output=True, check=True)


def as_role(url, role):
    parsed = urllib.parse.urlsplit(url)
    return urllib.parse.urlunsplit(parsed._replace(netloc=f"{role}@{parsed.hostname}:{parsed.port}"))


def rows(url, schema, table, columns=None):
    selection = "to_jsonb(t)" if columns is None else "jsonb_build_object(" + ",".join(
        database.literal(c) + ",t." + ident(c) for c in columns) + ")"
    result = execute(url, f"SELECT json_build_object('rows',coalesce(json_agg(r),'[]')) FROM (SELECT {selection} r FROM {ident(schema)}.{ident(table)} t) q;")
    return result[-1]["rows"]


def fingerprint(value):
    encoded = sorted(json.dumps(row, sort_keys=True, separators=(",", ":")) for row in value)
    return hashlib.sha256("\n".join(encoded).encode()).hexdigest()


def copy_rows(url, source, target, columns):
    # Rehearsal targets are newly owned schemas. An unexpected preexisting row
    # must stop the import before any table is copied; no merge/overwrite guess.
    if not columns or any(table not in TABLES for table in columns):
        raise ValueError("zuse_rehearsal_invalid_table_inventory")
    counts = execute(url, "SELECT json_build_object('rows'," + "+".join(
        f"(SELECT count(*) FROM {ident(target)}.{ident(table)})" for table in columns) + ");")[-1]["rows"]
    if counts:
        raise ValueError("zuse_rehearsal_target_contains_rows")
    execute(url, "BEGIN;" + "\n".join(
        f"INSERT INTO {ident(target)}.{ident(table)} ({','.join(map(ident,names))}) SELECT {','.join(map(ident,names))} FROM {ident(source)}.{ident(table)};"
        for table,names in columns.items()) + "COMMIT;")


def seed_row(url, schema, table, overrides):
    metadata = execute(url, f"SELECT json_build_object('columns',json_agg(json_build_object('name',column_name,'type',data_type,'required',is_nullable='NO','default',column_default) ORDER BY ordinal_position)) FROM information_schema.columns WHERE table_schema={database.literal(schema)} AND table_name={database.literal(table)};")[-1]["columns"]
    data = dict(overrides)
    for col in metadata:
        if col["required"] and col["default"] is None and col["name"] not in data:
            kind = col["type"]
            data[col["name"]] = (1 if kind in ("bigint", "integer", "smallint") else
                                 False if kind == "boolean" else {} if kind == "jsonb" else "fixture_" + col["name"])
    names = list(data)
    sql = f"INSERT INTO {ident(schema)}.{ident(table)} ({','.join(map(ident,names))}) SELECT "
    sql += ",".join("r." + ident(c) for c in names)
    sql += f" FROM jsonb_populate_record(NULL::{ident(schema)}.{ident(table)},{database.literal(json.dumps(data))}::jsonb) r;"
    execute(url, sql)


def fixtures(object_root):
    objects = {"fixture/transcript.enc": b"synthetic-ciphertext-transcript-v1", "fixture/recovery.enc": b"synthetic-ciphertext-recovery-v1"}
    inventory = []
    for key, payload in objects.items():
        path = object_root / key
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
        path.chmod(0o600)
        inventory.append({"key": key, "sha256": hashlib.sha256(payload).hexdigest(), "bytes": len(payload)})
    key = {"keyId": "fixture-legacy-key-v1", "wrappedKey": "opaque-fixture-wrapped-key", "aadVersion": 1}
    common = {"account_id": "fixture-owner"}
    data = {
        "api_stripe_customers": dict(common, customer_id="cus_fixture_stable"),
        "api_entitlements": dict(common, entitlement_id="ent_fixture", kind="cloud-workspace", provider="polar", status="active", provider_subscription_id="sub_fixture_stable"),
        "api_environments": dict(common, environment_id="fixture-environment", provider_kind="cloud", environment_public_key="opaque-fixture-legacy-public-binding", http_base_url="https://fixture.invalid", ws_base_url="wss://fixture.invalid"),
        "api_environment_credentials": dict(common, credential_id="fixture-environment-credential", environment_id="fixture-environment", credential_hash="opaque-fixture-legacy-credential-hash"),
        "api_devices": dict(common, device_id="fixture-device", platform="desktop", dpop_jwk={"kty":"OKP","crv":"Ed25519","x":"opaque-fixture-public-key"}, dpop_thumbprint="fixture-device-thumbprint"),
        "api_agent_activity": dict(common, id=73, environment_id="fixture-environment", session_id="fixture-session", kind="completed", title="fixture opaque tool activity"),
        "api_cloud_projects": dict(common, project_id="fixture-project", repository_identity="github.com/example/fixture", repository_url="https://github.com/example/fixture.git", display_name="fixture", default_branch="main", visibility="private", git_connection_kind="github-app", configuration_digest="fixture-digest", state="connected"),
        "api_cloud_project_builds": dict(common, build_id="fixture-build", project_id="fixture-project", provider="box", snapshot_id="fixture-snapshot", template_version="typescript-legacy-v1", configuration_digest="fixture-digest", state="ready", settings={"runtimeFormat": "typescript-legacy-v1"}),
        "api_cloud_workspaces": dict(common, workspace_id="fixture-task", project_id="fixture-project", build_id="fixture-build", provider="box", provider_sandbox_id="fixture-retained-machine", chat_id="fixture-chat", initial_session_id="fixture-session", branch="fixture/dirty-branch", base_ref="main", state="paused", desired_state="paused", status_code="legacy-retained", wrapped_transcript_key="opaque-fixture-wrapped-transcript-key", request_config={"encryption": key, "runtimeFormat": "typescript-legacy-v1", "recoveryArchive": inventory[1], "toolCiphertextRef": inventory[0]["key"]}),
        "api_cloud_transcript_checkpoints": {"workspace_id": "fixture-task", "session_id": "fixture-session", "runtime_generation": 7, "stream_epoch": "fixture-epoch", "stream_version": 11, "object_key": inventory[0]["key"], "ciphertext_sha256": inventory[0]["sha256"], "ciphertext_bytes": inventory[0]["bytes"]},
        "api_cloud_workspace_api_messages": dict(common, message_id="fixture-message", workspace_id="fixture-task", seq=19, role="assistant", sealed_content="opaque-fixture-message-and-tool-ciphertext", command_id="fixture-command", turn_id="fixture-turn", status="settled"),
        "api_cloud_workspace_command_receipts": {"workspace_id": "fixture-task", "command_id": "fixture-command", "action": "tool-completed", "workspace_revision": 13},
        "api_cloud_billing_periods": dict(common, period_id="period_fixture_stable", provider_subscription_id="sub_fixture_stable", status="manual", price_catalog_version="fixture-prices"),
        "api_cloud_billing_ledger": dict(common, entry_id="entry_fixture_stable", period_id="period_fixture_stable", kind="provider-cost", amount_micros=9007199254740993, source_id="provider_event_fixture", metadata={"providerEventId": "provider_event_fixture", "settled": True}),
        "api_cloud_auth_authorities": dict(common, provider="box", provider_sandbox_id="fixture-legacy-authority-machine", storage_incarnation_id="fixture-legacy-authority-disk", auth_epoch=9, toolchain_version="typescript-legacy-v1", state="ready", status={"wrappedKeyMetadata":key}),
        "api_model_connections": dict(common, provider="chatgpt", kind="active", connection_id="fixture-legacy-model", envelope="opaque-fixture-legacy-model-credential-envelope"),
        "api_cloud_github_users": dict(common, login="fixture", name="Fixture", email="42+fixture@users.noreply.github.com", sealed_credentials="opaque-fixture-legacy-github-credential-envelope"),
        "api_github_identities": dict(common, github_user_id=42, data={"accountId": "fixture-owner", "githubUserId": 42, "verificationId": "fixture-proof"}),
        "api_github_join_policies": {"organization_id": "fixture-org", "installation_id": 9, "github_org_id": 81, "data": {"enabled": True, "revision": "fixture-policy-v1"}},
        "api_github_enrollments": dict(common, organization_id="fixture-org", installation_id=9, data={"githubUserId": 42, "blocked": False, "memberId": "fixture-membership", "revision": "fixture-enrollment-v1"}),
        "api_cloud_snapshots": dict(common, provider="box", snapshot_id="fixture-snapshot", build_id="fixture-build", state="retained", record={"storageFormat": "typescript-legacy-v1", "encryption": key, "archive": inventory[1]}),
        "api_machines": dict(common, machine_id="fixture-persistent-machine", provider="box", provider_server_id="fixture-retained-server", provider_label="fixture", state="suspended", desired_state="suspended", status_code="retained", entitlement_id="ent_fixture"),
        "api_master_ownership": dict(common, master_id="fixture-master", home_environment_id="fixture-environment", generation=17, repositories_json=json.dumps({"fixture-project": "opaque-fixture-key-binding"})),
        "api_workspace_settings": {"owner_id": "fixture-owner", "revision": 3, "values": {"cloudProvider": "box", "encryption": key}},
    }
    data.update({
        "api_cloud_github_installations": dict(common, installation_id=9, github_account_id=81, account_login="fixture-org", account_type="Organization", repository_selection="selected", allowed_repositories=[{"id":13,"nameWithOwner":"example/fixture"}]),
        "api_cloud_workspace_api_turn_receipts": dict(workspace_id="fixture-task", turn_id="fixture-turn", outcome="completed", content_digest="fixture-stable-content-digest"),
        "api_cloud_workspace_runtime_summaries": dict(workspace_id="fixture-task", runtime_generation=7, summary_revision=13, title="fixture", active_session_id="fixture-session", session_head_version=11),
        "api_cloud_workspace_usage": dict(common, event_id="usage_fixture_stable", workspace_id="fixture-task", provider="box", kind="runtime-seconds", quantity=73, provider_event_id="provider_event_fixture"),
        "api_cloud_billing_usage": dict(common, entry_id="usage_entry_stable", period_id="period_fixture_stable", resource_kind="workspace", resource_id="fixture-task", provider="box", provider_event_id="provider_event_fixture", provider_execution_id="execution_fixture_stable", status="settled", provider_cost_micros=9007199254740993),
        "api_cloud_billing_reservations": dict(common, period_id="period_fixture_stable", resource_kind="workspace", resource_id="fixture-task", provider="box", provider_cost_micros=9007199254740993),
        "api_provider_usage_events": dict(event_id="provider_event_fixture", provider="box", type="stopped", provider_resource_id="fixture-retained-machine", payload={"executionId":"execution_fixture_stable"}),
        "api_provider_lifecycle_pairs": dict(provider="box", closing_event_id="provider_event_fixture", opening_event_id="provider_opening_fixture"),
        "api_provider_event_finalizations": dict(provider="box", event_id="provider_event_fixture", provider_execution_id="execution_fixture_stable"),
        "api_provider_event_deliveries": dict(provider="box", delivery_id="delivery_fixture_stable", event_id="provider_event_fixture", source="webhook", status="processed"),
        "api_billing_events": dict(event_id="billing_event_stable", provider="stripe", payload={"customerId":"cus_fixture_stable","subscriptionId":"sub_fixture_stable"}, processed_at=1),
        "api_provider_price_schedule": dict(provider="box", version="fixture-prices"),
        "api_snapshot_price_schedule": dict(provider="box", version="fixture-snapshot-prices", monthly_micros=2500, duration_ms=2592000000),
        "api_cloud_provider_connections": dict(common, connection_id="fixture-provider-connection", provider="box", envelope="opaque-fixture-provider-envelope", active=False),
        "api_cloud_billing_meter_reconciliation_attempts": dict(period_id="period_fixture_stable", provider="stripe"),
        "api_cloud_billing_meter_reconciliations": dict(reconciliation_id="reconciliation_fixture_stable", period_id="period_fixture_stable", provider="stripe", expected_units=9007199254740993, observed_units=9007199254740993),
        "api_platform_costs": dict(cost_id="platform_cost_fixture_stable", vendor="fixture", kind="platform", amount_micros=9007199254740993, external_id="invoice_fixture_stable"),
        "api_provider_statement_totals": dict(statement_id="statement_fixture_stable", provider="box", external_id="invoice_fixture_stable", amount_micros=9007199254740993),
    })
    opening = dict(data["api_provider_usage_events"], event_id="provider_opening_fixture", type="started")
    data["api_provider_usage_events"] = [data["api_provider_usage_events"], opening]
    return data, inventory


def rehearse():
    """Use only a newly owned private cluster; deployment URLs are never read."""
    with owned_cluster() as (url, root):
        directory = execute(url, "SELECT json_build_object('directory',current_setting('data_directory')); ")[-1]["directory"]
        if Path(directory).resolve() != (root / "data").resolve():
            raise ValueError("zuse_rehearsal_not_owned_fixture_cluster")
        tag = uuid.uuid4().hex[:12]
        source, target, control = ("zuse_rehearsal_" + prefix + "_" + tag for prefix in ("source", "target", "control"))
        legacy_role, rust_role = "zuse_legacy_" + tag, "zuse_rust_" + tag
        baseline = (database.MIGRATIONS / "0000_application.sql").read_text()
        applied_migrations = database.migrations()
        for schema, migrations in ((source, [{"sql": baseline}]), (target, applied_migrations)):
            execute(url, f"CREATE SCHEMA {ident(schema)}; SET search_path TO {ident(schema)},pg_catalog;" + "\n".join(m["sql"] for m in migrations))
        data, inventory = fixtures(root / "source-objects")
        for table in TABLES:
            payloads = data[table] if isinstance(data[table],list) else [data[table]]
            for payload in payloads:
                seed_row(url, source, table, payload)
        quarantine_data = {
            "api_cloud_workspace_launch_intents": dict(workspace_id="fixture-task", account_id="fixture-owner", chat_id="fixture-chat", session_id="fixture-session", turn_id="fixture-pending-turn", command_id="fixture-pending-command", ciphertext="opaque-fixture-pending-launch", expires_at=9007199254740993),
            "api_cloud_billing_outbox": dict(outbox_id="fixture-payment-intent", period_id="period_fixture_stable", account_id="fixture-owner", provider="stripe", amount_cents=9007199254740993, idempotency_key="fixture-payment-key"),
            "api_stripe_meter_deliveries": dict(delivery_key="fixture-meter-key", payload="opaque-fixture-meter-payload", lease_until=9007199254740993),
            "api_stripe_subscription_migrations": dict(polar_subscription_id="sub_polar_fixture_stable", account_id="fixture-owner", customer_id="cus_fixture_stable", payload="opaque-fixture-subscription-payload", schedule_id="schedule_fixture_stable"),
            "api_model_connection_leases": dict(account_id="fixture-owner", provider="chatgpt", token="opaque-fixture-live-refresh-lease", expires_at_ms=9007199254740993),
        }
        for table, payload in quarantine_data.items():
            seed_row(url, source, table, payload)
        execute(url, f"SELECT setval(pg_get_serial_sequence('{source}.api_agent_activity','id'),123,true);")
        execute(url, f"""CREATE SCHEMA {ident(control)};
CREATE TABLE {ident(control)}.writer_fence(singleton boolean PRIMARY KEY CHECK(singleton),backend text NOT NULL CHECK(backend IN ('none','legacy','rust')));
INSERT INTO {ident(control)}.writer_fence VALUES(true,'none');
CREATE TABLE {ident(control)}.snapshots(side text NOT NULL,table_name text NOT NULL,row_data jsonb NOT NULL);
CREATE TABLE {ident(control)}.retained_quarantine(table_name text NOT NULL,row_data jsonb NOT NULL);
CREATE ROLE {ident(legacy_role)} LOGIN; CREATE ROLE {ident(rust_role)} LOGIN;
GRANT USAGE ON SCHEMA {ident(source)} TO {ident(legacy_role)};
GRANT USAGE ON SCHEMA {ident(target)} TO {ident(rust_role)};
GRANT SELECT,INSERT,UPDATE,DELETE ON {','.join(ident(source)+'.'+ident(t) for t in TABLES)} TO {ident(legacy_role)};
GRANT SELECT,INSERT,UPDATE,DELETE ON {','.join(ident(target)+'.'+ident(t) for t in TABLES)} TO {ident(rust_role)};
GRANT USAGE,SELECT ON SEQUENCE {ident(source)}.api_agent_activity_id_seq TO {ident(legacy_role)};
GRANT USAGE,SELECT ON SEQUENCE {ident(target)}.api_agent_activity_id_seq TO {ident(rust_role)};
CREATE FUNCTION {ident(control)}.enforce_writer() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path TO {ident(control)},pg_catalog AS $gate$
DECLARE caller text := CASE current_setting('role') WHEN 'none' THEN session_user ELSE current_setting('role') END; active text;
BEGIN
SELECT backend INTO active FROM writer_fence WHERE singleton;
IF NOT ((active='none' AND caller='zuse_rehearsal') OR
 (TG_TABLE_SCHEMA='{source}' AND active='legacy' AND caller='{legacy_role}') OR
 (TG_TABLE_SCHEMA='{target}' AND active='rust' AND caller='{rust_role}')) THEN
 RAISE EXCEPTION 'zuse_rehearsal_writer_fenced'; END IF;
IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $gate$;""")
        for schema in (source, target):
            for table in TABLES + QUARANTINE:
                execute(url, f"CREATE TRIGGER migration_writer_fence BEFORE INSERT OR UPDATE OR DELETE ON {ident(schema)}.{ident(table)} FOR EACH ROW EXECUTE FUNCTION {ident(control)}.enforce_writer();")
        execute(url, f"UPDATE {ident(control)}.writer_fence SET backend='legacy';")
        legacy_url, rust_url = as_role(url, legacy_role), as_role(url, rust_role)
        execute(legacy_url, f"UPDATE {ident(source)}.api_cloud_projects SET display_name=display_name;")
        # Drain in-flight source writes before taking away its writer lease.
        locks = ",".join(f"{ident(source)}.{ident(table)}" for table in TABLES + QUARANTINE)
        execute(url, f"BEGIN; LOCK TABLE {locks} IN SHARE ROW EXCLUSIVE MODE; UPDATE {ident(control)}.writer_fence SET backend='none'; COMMIT;")
        columns = {}
        source_digests = {}
        for table in TABLES:
            col = execute(url, f"SELECT json_build_object('columns',json_agg(column_name ORDER BY ordinal_position)) FROM information_schema.columns WHERE table_schema={database.literal(source)} AND table_name={database.literal(table)};")[-1]["columns"]
            columns[table] = col
            source_digests[table] = fingerprint(rows(url, source, table))
        copy_rows(url,source,target,columns)
        quarantine_digests = {}
        for table in QUARANTINE:
            execute(url, f"INSERT INTO {ident(control)}.retained_quarantine SELECT {database.literal(table)},to_jsonb(t) FROM {ident(source)}.{ident(table)} t;")
            source_rows = rows(url,source,table)
            retained = execute(url,f"SELECT json_build_object('rows',coalesce(json_agg(row_data),'[]')) FROM {ident(control)}.retained_quarantine WHERE table_name={database.literal(table)};")[-1]["rows"]
            if fingerprint(source_rows) != fingerprint(retained) or rows(url,target,table):
                raise ValueError("zuse_rehearsal_quarantine_changed_or_activated")
            quarantine_digests[table] = fingerprint(source_rows)
        for table in TABLES:
            if fingerprint(rows(url, target, table, columns[table])) != source_digests[table]:
                raise ValueError("zuse_rehearsal_import_identity_or_ciphertext_changed")
            for side, schema in (("source", source), ("target", target)):
                execute(url, f"INSERT INTO {ident(control)}.snapshots(side,table_name,row_data) SELECT {database.literal(side)},{database.literal(table)},to_jsonb(t) FROM {ident(schema)}.{ident(table)} t;")
        for table in QUARANTINE:
            for side,schema in (("source",source),("target",target)):
                execute(url,f"INSERT INTO {ident(control)}.snapshots SELECT {database.literal(side)},{database.literal(table)},to_jsonb(t) FROM {ident(schema)}.{ident(table)} t;")
        # Preserve consumed sequence values, including gaps not represented by rows.
        execute(url, f"SELECT setval(pg_get_serial_sequence('{target}.api_agent_activity','id'),123,true);")
        for item in inventory:
            src = root / "source-objects" / item["key"]
            dst = root / "target-objects" / item["key"]
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, dst)
            dst.chmod(0o600)
            if hashlib.sha256(dst.read_bytes()).hexdigest() != item["sha256"]:
                raise ValueError("zuse_rehearsal_object_checksum_mismatch")
        checks = []
        for side, schema in (("source", source), ("target", target)):
            for table in TABLES + QUARANTINE:
                snapshot = f"SELECT row_data FROM {ident(control)}.snapshots WHERE side={database.literal(side)} AND table_name={database.literal(table)}"
                actual = f"SELECT to_jsonb(t) FROM {ident(schema)}.{ident(table)} t"
                checks.append(f"IF EXISTS(({actual} EXCEPT {snapshot}) UNION ALL ({snapshot} EXCEPT {actual})) THEN RAISE EXCEPTION 'zuse_rehearsal_rollback_target_changed'; END IF;")
        checks.append(f"IF (SELECT last_value FROM {ident(target)}.api_agent_activity_id_seq)<>123 THEN RAISE EXCEPTION 'zuse_rehearsal_rollback_sequence_changed'; END IF;")
        all_locks = ",".join(f"{ident(schema)}.{ident(table)}" for schema in (source, target) for table in TABLES + QUARANTINE)

        def transition(expected, next_writer):
            execute(url, f"BEGIN; LOCK TABLE {all_locks} IN SHARE ROW EXCLUSIVE MODE; DO $handoff$ BEGIN IF (SELECT backend FROM {ident(control)}.writer_fence)<>{database.literal(expected)} THEN RAISE EXCEPTION 'zuse_rehearsal_writer_state_changed'; END IF;" + "".join(checks) + f" UPDATE {ident(control)}.writer_fence SET backend={database.literal(next_writer)}; END $handoff$; COMMIT;")

        def blocked(app_url, schema):
            try:
                execute(app_url, f"UPDATE {ident(schema)}.api_cloud_projects SET display_name='should-never-write';")
            except ValueError as error:
                if str(error) != "zuse_rehearsal_writer_fenced":
                    raise
                return True
            raise ValueError("zuse_rehearsal_multiple_writers_possible")

        transition("none", "rust")
        blocked(legacy_url, source)
        execute(rust_url, f"UPDATE {ident(target)}.api_cloud_projects SET display_name=display_name;")
        transition("rust", "legacy")
        blocked(rust_url, target)
        execute(legacy_url, f"UPDATE {ident(source)}.api_cloud_projects SET display_name=display_name;")
        transition("legacy", "rust")
        # A consumed ID may have escaped an uncertain/rolled-back target write.
        # Even without a row difference, rollback must not reuse that identity.
        execute(rust_url, f"SELECT nextval(pg_get_serial_sequence('{target}.api_agent_activity','id'));")
        try:
            transition("rust", "legacy")
        except ValueError as error:
            if str(error)!="zuse_rehearsal_rollback_sequence_changed":
                raise
        else:
            raise ValueError("zuse_rehearsal_sequence_rollback_unsafe")
        blocked(legacy_url, source)
        activity = execute(rust_url, f"INSERT INTO {ident(target)}.api_agent_activity(environment_id,account_id,session_id,kind,occurred_at) VALUES('fixture-environment','fixture-owner','fixture-session','completed',2) RETURNING json_build_object('id',id);")[-1]["id"]
        if activity != 125:
            raise ValueError("zuse_rehearsal_sequence_identity_reused")
        rollback_blocked = False
        try:
            transition("rust", "legacy")
        except ValueError as error:
            if str(error) != "zuse_rehearsal_rollback_target_changed":
                raise
            rollback_blocked = True
        if not rollback_blocked:
            raise ValueError("zuse_rehearsal_unsafe_rollback_allowed")
        blocked(legacy_url, source)
        admission = execute(url, f"SELECT json_build_object('nativeOwners',(SELECT count(*) FROM {ident(target)}.zuse_rust_control_owners),'jobs',(SELECT count(*) FROM {ident(target)}.zuse_rust_jobs));")[-1]
        if admission != {"nativeOwners": 0, "jobs": 0}:
            raise ValueError("zuse_rehearsal_incompatible_runtime_admitted")
        report = dict(plan(), fixturePassed=True, importedTableCount=len(TABLES), importedRowCount=sum(len(value) if isinstance(value,list) else 1 for value in data.values()),
                      quarantinedTableCount=len(QUARANTINE), quarantinedFixtureRowCount=len(quarantine_data),
                      quarantinedRowsPreservedAndInactive=True, quarantineTableDigests=quarantine_digests,
                      sourceSchemaSha256=hashlib.sha256(baseline.encode()).hexdigest(),
                      targetMigrationManifest=[{"name":m["name"],"sha256":m["sha256"]} for m in applied_migrations],
                      stableIdentitiesAndCiphertextPreserved=True, sourceTableDigests=source_digests,
                      copiedFixtureObjectCount=len(inventory), fixtureObjectChecksumsVerified=True,
                      isolatedRoleWriterFencingVerified=True, cleanRollbackVerified=True,
                      rollbackAfterTargetWritesBlocked=True, rollbackAfterSequenceConsumptionBlocked=True,
                      sequenceHighWaterMarkPreserved=True, largeIntegerFinancialAmountPreserved=True,
                      nativeControlOwnersImported=0, providerJobsImported=0,
                      fixtureResourcesRemovedOnExit=True)
        return report


def audit(source_url_file, source_schema):
    """Explicit read-only aggregate audit. Never selects history or credential data."""
    path = Path(source_url_file)
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode):
        raise ValueError("zuse_rehearsal_private_source_file_required")
    descriptor=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    with os.fdopen(descriptor) as source:
        metadata=os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid not in (0,os.getuid()) or stat.S_IMODE(metadata.st_mode)&0o077 or metadata.st_size>16384:
            raise ValueError("zuse_rehearsal_private_source_file_required")
        url=source.read().strip()
    ident(source_schema)
    available = execute(url, "BEGIN READ ONLY; SET LOCAL search_path=pg_catalog; SELECT json_build_object('tables',coalesce(json_agg(tablename),'[]')) FROM pg_tables WHERE schemaname=" + database.literal(source_schema) + ";COMMIT;")[-1]["tables"]
    present = [table for table in TABLES + QUARANTINE if table in available]
    # Every table name comes from the fixed migration inventory, never from SQL input.
    sql = "BEGIN READ ONLY;SET LOCAL search_path=pg_catalog;SET LOCAL statement_timeout='15s';SELECT json_build_object('counts',coalesce(jsonb_object_agg(table_name,row_count),'{}'::jsonb)) FROM (" + " UNION ALL ".join(
        "SELECT " + database.literal(table) + f" AS table_name,(SELECT count(*) FROM {ident(source_schema)}.{ident(table)}) AS row_count" for table in present) + ") inventory;COMMIT;"
    counts = execute(url, sql)[-1]["counts"] if present else {}
    return {"mode": "read-only-counts", "counts": counts, "missingTables": [t for t in TABLES + QUARANTINE if t not in present],
            "unclassifiedTables": [t for t in available if t.startswith("api_") and t not in TABLES + QUARANTINE],
            "sourceTableInventory": table_inventory(),
            "customerRowsExported": False, "plaintextSelected": False, "migrationExecuted": False,
            "realRetainedDatasetVerified": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--environment", choices=("staging",), required=True)
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("plan")
    sub.add_parser("rehearse")
    audit_parser = sub.add_parser("audit")
    audit_parser.add_argument("--source-url-file", required=True)
    audit_parser.add_argument("--source-schema", default="public")
    args = parser.parse_args()
    result = plan() if args.action == "plan" else rehearse() if args.action == "rehearse" else audit(args.source_url_file, args.source_schema)
    print(json.dumps(result, sort_keys=True, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        code = str(error) if re.fullmatch(r"zuse_(rehearsal|schema)_[a-z_]+", str(error)) else "zuse_rehearsal_failed"
        raise SystemExit(code + "; no credentials or row data printed")
