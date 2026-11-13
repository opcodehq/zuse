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
mkdir -p "$ZUSE_USER_DATA" "$status_dir"
if [[ "${1:-}" != --foreground ]]; then
  supervisor="${ZUSE_RUNTIME_SUPERVISOR:-/usr/local/lib/zuse/runtime-supervisor.py}"
  if [[ -f "$supervisor" && ! -L "$supervisor" ]]; then
    # A bounded private handoff is allowed to finish; the provider's resident
    # boot process owns the guest, independently of this exec's PID namespace.
    exec python3 "$supervisor" --data-dir "$ZUSE_USER_DATA" launch
  fi
  # Legacy detachment is never inferred from a successful exec or local fixture.
  # Only an explicit provider-scoped release attestation may enable this path.
  [[ "${ZUSE_RUNTIME_PERSISTENT_EXEC_CERTIFIED:-0}" == 1 ]] || { printf '%s\n' runtime_supervisor_missing >&2; exit 1; }
  case "${ZUSE_RUNTIME_PROVIDER:-}" in boat|e2b|daytona) ;; *) printf '%s\n' runtime_persistent_exec_uncertified >&2; exit 1;; esac
  command -v setsid >/dev/null || { printf '%s\n' runtime_detach_unavailable >&2; exit 1; }
  nohup setsid "$0" --foreground >>"$status_dir/rust-runtime.log" 2>&1 </dev/null &
  exit 0
fi
boot_args=()
if [[ "${ZUSE_RUNTIME_RETAINED:-false}" == true ]]; then
  [[ -f "$ZUSE_USER_DATA/cloud-runtime-identity.json" ]]
  unset ZUSE_RUNTIME_BOOT_TOKEN_FILE ZUSE_RUNTIME_BOOT_TOKEN
else
  [[ -f "$ZUSE_RUNTIME_BOOT_TOKEN_FILE" && ! -L "$ZUSE_RUNTIME_BOOT_TOKEN_FILE" ]]
  boot_args=(--boot-token-file "$ZUSE_RUNTIME_BOOT_TOKEN_FILE")
fi
# Never pass the reusable provider key or any parent runtime lock into a new guest.
unset ZUSE_WORKSPACE_RUNTIME_LOCK_FD BOXD_API_KEY BOAT_API_KEY E2B_API_KEY DAYTONA_API_KEY PRIZED_API_KEY CLOUD_DATA_ENCRYPTION_KEY CLOUD_CREDENTIAL_VAULT_KEY RELAY_MINT_PRIVATE_JWK WORKOS_API_KEY
harness_args=()
if [[ -n "${ZUSE_HARNESS_ROOT:-}" ]]; then
  runtime_version="$("$binary" --version)"
  runtime_version="${runtime_version##* }"
  harness_args=(python3 "${ZUSE_HARNESS_RELEASE_TOOL:-/usr/local/lib/zuse/harness-release.py}" --root "$ZUSE_HARNESS_ROOT" exec
    --channel "${ZUSE_HARNESS_CHANNEL:-stable}"
    --runtime-version "$runtime_version" --pin "$ZUSE_USER_DATA/cloud-harness-release.json" --)
fi
exec "${harness_args[@]}" "$binary" --workspace-id "$ZUSE_CLOUD_WORKSPACE_ID" --api-url "$ZUSE_API_URL" \
  --generation "$ZUSE_RUNTIME_GENERATION" --storage-incarnation-id "$ZUSE_STORAGE_INCARNATION_ID" \
  --data-dir "$ZUSE_USER_DATA" --workspace-root "$ZUSE_CLOUD_WORKSPACE_ROOT" \
  "${boot_args[@]}"
