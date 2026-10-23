#!/usr/bin/env bash
# Installed into prepared Rust images. The guest's lifetime lock serializes retries.
set -euo pipefail
umask 077
: "${ZUSE_CLOUD_WORKSPACE_ID:?}"
: "${ZUSE_API_URL:?}"
: "${ZUSE_RUNTIME_GENERATION:?}"
: "${ZUSE_STORAGE_INCARNATION_ID:?}"
if [[ "${ZUSE_RUNTIME_RETAINED:-false}" != true ]]; then
  : "${ZUSE_RUNTIME_BOOT_TOKEN_FILE:?}"
fi
export ZUSE_USER_DATA="${ZUSE_USER_DATA:-/home/zuse/.zuse-data}"
export ZUSE_CLOUD_WORKSPACE_ROOT="${ZUSE_CLOUD_WORKSPACE_ROOT:-/home/zuse/workspace}"
status_dir="${ZUSE_WORKSPACE_RUNTIME_STATUS_DIR:-/var/lib/zuse/workspace}"
binary="${ZUSE_RUNTIME_BINARY:-/usr/local/bin/zuse-cloud-runtime}"
[[ -x "$binary" && -d "$ZUSE_CLOUD_WORKSPACE_ROOT" ]]
boot_args=()
if [[ "${ZUSE_RUNTIME_RETAINED:-false}" == true ]]; then
  [[ -f "$ZUSE_USER_DATA/cloud-runtime-identity.json" ]]
  unset ZUSE_RUNTIME_BOOT_TOKEN_FILE ZUSE_RUNTIME_BOOT_TOKEN
else
  [[ -f "$ZUSE_RUNTIME_BOOT_TOKEN_FILE" && ! -L "$ZUSE_RUNTIME_BOOT_TOKEN_FILE" ]]
  boot_args=(--boot-token-file "$ZUSE_RUNTIME_BOOT_TOKEN_FILE")
fi
mkdir -p "$ZUSE_USER_DATA" "$status_dir"
if [[ "${1:-}" != --foreground ]]; then
  nohup "$0" --foreground >>"$status_dir/rust-runtime.log" 2>&1 </dev/null &
  exit 0
fi
# Never pass the reusable provider key or any parent runtime lock into a new guest.
unset ZUSE_WORKSPACE_RUNTIME_LOCK_FD BOXD_API_KEY CLOUD_DATA_ENCRYPTION_KEY CLOUD_CREDENTIAL_VAULT_KEY RELAY_MINT_PRIVATE_JWK WORKOS_API_KEY
exec "$binary" --workspace-id "$ZUSE_CLOUD_WORKSPACE_ID" --api-url "$ZUSE_API_URL" \
  --generation "$ZUSE_RUNTIME_GENERATION" --storage-incarnation-id "$ZUSE_STORAGE_INCARNATION_ID" \
  --data-dir "$ZUSE_USER_DATA" --workspace-root "$ZUSE_CLOUD_WORKSPACE_ROOT" \
  "${boot_args[@]}"
