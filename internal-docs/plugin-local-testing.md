# Interactive local plugin testing

The `plugin_local` example serves the real Rust API router and actual Executor
SDK on `127.0.0.1:8088`, with PostgreSQL and a built-in echo MCP server. It uses
the fixed development bearer `local-dev` and personal tenant `personal:local-user`.
This fixture identity is only in the example; the production binary still uses
WorkOS. The example binds loopback, requires a localhost database and uses a
separate persistent `plugin_local` schema. It enables loopback MCP endpoints.

Requires Docker, Rust and Bun on PATH. From the repository root:

```sh
docker run -d --name zuse-plugin-local \
  -e POSTGRES_PASSWORD=local-plugin -e POSTGRES_DB=zuse_plugins \
  -p 127.0.0.1:55440:5432 postgres:17
docker exec zuse-plugin-local pg_isready -U postgres

export ZUSE_CLOUD_TEST_DATABASE_URL='postgres://postgres:local-plugin@127.0.0.1:55440/zuse_plugins'
export ZUSE_CLOUD_PLUGIN_ENCRYPTION_KEY="$(openssl rand -base64 32)"
cargo run -p zuse-cloud --example plugin_local
```

Wait for PostgreSQL readiness before starting the example. Keep the same encryption
key when restarting against existing state. The example applies local fixture and
SDK migrations before serving. It does not read `ZUSE_CLOUD_DATABASE_URL`.

In another terminal, list connections and connect the built-in MCP server:

```sh
curl -sS http://127.0.0.1:8088/v1/plugins \
  -H 'Authorization: Bearer local-dev' -H 'Content-Type: application/json' \
  -d '{"action":"list"}'

curl -sS http://127.0.0.1:8088/v1/plugins \
  -H 'Authorization: Bearer local-dev' -H 'Content-Type: application/json' \
  -d '{"action":"addServer","tenantId":"personal:local-user","requestId":"fbb348a5-6cd9-411f-885b-550843d5afc5","name":"Local Echo","url":"http://127.0.0.1:8088/dev/mcp"}'

curl -sS http://127.0.0.1:8088/v1/plugins/tools \
  -H 'Authorization: Bearer local-dev' -H 'Content-Type: application/json' \
  -d '{"action":"search","query":"echo"}'
```

Copy the returned tool `address` into these requests:

```sh
curl -sS http://127.0.0.1:8088/v1/plugins/tools \
  -H 'Authorization: Bearer local-dev' -H 'Content-Type: application/json' \
  -d '{"action":"schema","address":"COPY_ADDRESS_HERE"}'

curl -sS http://127.0.0.1:8088/v1/plugins/tools \
  -H 'Authorization: Bearer local-dev' -H 'Content-Type: application/json' \
  -d '{"action":"call","address":"COPY_ADDRESS_HERE","arguments":{"message":"hello from local Executor"}}'
```

Expect the message in the MCP text result. Try a numeric message to observe input
validation, omit the bearer to observe authorization rejection, and restart the
example with the same key to verify the saved connection survives.

Inspect local metadata:

```sh
docker exec -it zuse-plugin-local psql -U postgres -d zuse_plugins \
  -c 'SELECT tenant_id, connection_id, state FROM plugin_local.zuse_plugin_connections;'
```

Stop the API with Ctrl-C and the database with `docker stop zuse-plugin-local`.
Restart the database with `docker start zuse-plugin-local`; retain its container
and your encryption key to retain connections.

This tests interactive API/execution/storage, not real WorkOS login, browser CORS,
organization memberships or provider OAuth compatibility. For those use the
normal API binary with WorkOS configuration and a reachable HTTPS callback origin;
see [deployment configuration](cloud-native-plugins.md).

## Desktop against this API

The compiled UI acceptance host can install the same local fixture identity into
an isolated engine. Build it and the native UI explicitly with test support:

```sh
cargo build -p zuse-cloud --example cloud-ui-host --features runtime-acceptance
cargo build -p zuse --features zuse-engine/test-support
mkdir -p .context/plugin-desktop-profile
target/debug/examples/cloud-ui-host .context/plugin-desktop-profile \
  /tmp/zuse-plugin-desktop-port --plugin-local
```

With the local API still running, launch the desktop in another terminal:

```sh
export ZUSE_DATA_DIR="$PWD/.context/plugin-desktop-profile"
export ZUSE_IPC_PORT="$(cat /tmp/zuse-plugin-desktop-port)"
target/debug/zuse
```

Open Settings → Plugins to see Local Echo. This fixture uses a signed-in local
account without WorkOS login. Plaintext API connections are permitted only for
numeric `127.0.0.1` in builds with `zuse-plugins/test-support`; normal builds retain
HTTPS enforcement. Stop both the fixture engine and UI when finished.
