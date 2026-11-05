# Standalone cloud database bootstrap

`database.py` initializes an empty private application schema with no customer or
fixture rows. It runs all reviewed SQL in one transaction, records migration hashes,
serializes concurrent migration attempts, rejects checksum drift and rejects taking
over an existing unmanaged schema. It never modifies the public schema or installs
a database server.

The initial application baseline preserves the current Rust SQL contract's `api_*`
names and storage formats. It was exported from the source application's canonical
schema at `a0155d2dd11f07495a99d3737c9bd40a5c8b4508` with drizzle-orm 0.44.3 and
drizzle-kit 0.31.4. Foreign keys resolve within the selected private schema; fresh
index creation is transactional. This is a compatibility baseline, not the proposed
Postgres hot-history redesign or a legacy data migration.

```sh
# No DB connection or mutation; inspect migration identities/checksums.
python3 infra/cloud/database.py --environment staging plan

# Read ZUSE_CLOUD_DATABASE_URL privately from the deployment secret environment.
python3 infra/cloud/database.py --environment staging apply
python3 infra/cloud/database.py --environment staging check
```

Staging uses `zuse_staging`; production uses `zuse_production`. Remote URLs require
`sslmode=verify-full`; include the verified `sslrootcert` file for Supabase. The URL
is never printed, passed as a command argument, or inferred from DATABASE_URL.
Inherited PG service/options are removed so a shell cannot redirect the operation.
The tool needs the `psql` client and a migration owner authorized to create its schema.

Provision separate restricted application roles before deploying services. Each role
needs its own default search_path/URL options, USAGE only on its environment schema,
and appropriate table/sequence privileges. Keep migration ledgers writable only by
the migration owner. Do not use the Supabase administrator credential in a deployed
service. The single-host deployment helper accepts a shared database only when both URLs
explicitly name the distinct expected environment schema search paths and restricted
login roles; arbitrary different usernames do not satisfy its isolation check.

This tool does not migrate legacy rows, transfer runtime ownership, register GitHub
or model accounts, seed billing entitlements, or configure auth keys. Importing
encrypted legacy records also requires the original keys or a reviewed re-encryption
migration. It deliberately does not apply old ownership records or enable writes to
legacy provider resources.

Validation:

```sh
python3 infra/cloud/tests/database.py
```

Six tests run against a fresh temporary localhost Postgres cluster, which is stopped
and removed afterwards. No deployment database credentials are used. These tests
cover empty staging/production bootstrap, idempotence, drift rejection, transaction
rollback, unmanaged-schema protection and TLS/environment handling.

The Hetzner deployment subsequently applied both private Supabase schemas and
restricted environment roles. No customer rows were imported and public tables
were not modified. See the single-host runbook for deployment state.

Migration rehearsal and the optional private-file read-only legacy inventory are
documented in [MIGRATION_REHEARSAL.md](MIGRATION_REHEARSAL.md). Its temporary-cluster
fixture proves row/ciphertext/reference preservation and writer/rollback mechanics;
it does not migrate real retained customer data or authorize execution from legacy
images.
