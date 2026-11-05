#!/usr/bin/env python3
"""Grant a bounded, non-billed staging pilot to one explicitly selected account."""
import argparse
import importlib.util
import json
from pathlib import Path
import re
import time
from urllib.parse import parse_qs, urlencode, urlsplit, urlunsplit

HERE = Path(__file__).resolve().parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


DB = module("pilot_database", HERE.parent / "database.py")
PROVISION = module("pilot_provision", HERE / "provision.py")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--account", required=True)
    parser.add_argument("--days", type=int, default=7, choices=range(1, 8))
    parser.add_argument("--allowance-dollars", type=int, default=5, choices=range(1, 51))
    parser.add_argument("--config-root", type=Path, default=Path("/etc/zuse/cloud"))
    args = parser.parse_args()
    if not re.fullmatch(r"user_[A-Za-z0-9]+", args.account):
        parser.error("An explicit WorkOS user ID is required")
    directory = args.config_root / "staging"
    runtime = PROVISION.DEPLOY.read_env(directory / "runtime.env")
    raw = urlsplit(runtime["ZUSE_CLOUD_DATABASE_URL"])
    if raw.username != "zuse_cloud_staging":
        raise ValueError("Pilot setup requires the isolated staging database role")
    query = parse_qs(raw.query)
    if query.get("options") != ["-csearch_path=zuse_staging,pg_catalog"]:
        raise ValueError("Pilot setup requires the isolated staging schema")
    query["sslrootcert"] = [str(args.config_root / "certs/supabase-ca.crt")]
    url = urlunsplit(raw._replace(query=urlencode(query, doseq=True)))
    now = int(time.time() * 1000)
    end = now + args.days * 86400000
    owner = DB.literal(args.account)
    entitlement = DB.literal("staging_pilot_" + args.account)
    period = DB.literal("staging_pilot_period_" + args.account)
    sql = f"""BEGIN;
SET LOCAL search_path TO zuse_staging,pg_catalog;
SET LOCAL statement_timeout='10s';
SELECT pg_advisory_xact_lock(hashtextextended('pilot:' || {owner},0));
DO $guard$ BEGIN
IF current_schema()<>'zuse_staging' OR current_user<>'zuse_cloud_staging' THEN
 RAISE EXCEPTION 'zuse_schema_environment_mismatch'; END IF;
IF EXISTS(SELECT 1 FROM zuse_rust_control_owners WHERE resource_id={owner}
 AND resource_kind IN ('project-catalog','image-catalog','billing') AND backend<>'rust') THEN
 RAISE EXCEPTION 'zuse_schema_existing_owner_requires_handoff'; END IF;
END $guard$;
INSERT INTO api_entitlements(entitlement_id,account_id,kind,offer_id,provider,status,period_start,paid_through,created_at,updated_at)
VALUES({entitlement},{owner},'cloud-workspace','cloud-workspace-standard-v1','manual','active',{now},{end},{now},{now})
ON CONFLICT(entitlement_id) DO NOTHING;
INSERT INTO api_cloud_billing_periods(period_id,account_id,billing_provider,status,currency,period_start,period_end,
base_price_micros,included_provider_cost_micros,markup_basis_points,price_catalog_version,overage_cap_micros,created_at,updated_at)
SELECT {period},{owner},'manual','active','USD',{now},{end},0,{args.allowance_dollars * 1000000},0,'staging-pilot-v1',0,{now},{now}
WHERE NOT EXISTS(SELECT 1 FROM api_cloud_billing_periods WHERE account_id={owner} AND period_end>{now})
ON CONFLICT DO NOTHING;
INSERT INTO zuse_rust_control_owners(resource_kind,resource_id,backend,generation,updated_at)
SELECT kind,{owner},'rust',1,{now} FROM unnest(ARRAY['project-catalog','image-catalog','billing']) kind
ON CONFLICT DO NOTHING;
SELECT json_build_object('environment','staging','accountId',account_id,'expiresAt',paid_through,'billingProvider',provider)
FROM api_entitlements WHERE entitlement_id={entitlement};
COMMIT;"""
    result = DB.execute(url, sql)
    accounts = {item for item in runtime.get("ZUSE_CLOUD_PROJECT_WRITE_ACCOUNTS", "").split(",") if item}
    accounts.add(args.account)
    PROVISION.update(directory / "runtime.env", {
        "ZUSE_CLOUD_PROJECT_WRITE_ACCOUNTS": ",".join(sorted(accounts)),
        # Same WorkOS identity deployment as this pilot; OAuth and App secrets
        # remain with the existing authority until explicit migration.
        "ZUSE_CLOUD_LEGACY_GITHUB_URL": "https://api.zuse.sh",
    })
    print(json.dumps({"pilot": result[-1], "creationEnabled": False,
                      "note": "Restart staging services. Prepared runtime and agent credentials are still required."}))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError):
        raise SystemExit("Staging pilot setup failed; private configuration was not printed")
