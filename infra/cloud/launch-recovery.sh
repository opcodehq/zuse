#!/usr/bin/env bash
# One recovery process group per persisted operation; scoped config never enters argv/logs.
set -euo pipefail
umask 077
operation="${1:?}"
[[ "$operation" =~ ^[a-zA-Z0-9_-]{1,128}$ ]]
root="${ZUSE_RECOVERY_STATUS_ROOT:-/var/lib/zuse/recovery}"
state="$root/$operation"
mkdir -p "$state"
if [[ "${2:-}" == --cancel ]]; then
  : >"$state/cancelled"
  if [[ -f "$state/pid" ]]; then
    read -r recovery_pid <"$state/pid"
    if [[ "$recovery_pid" =~ ^[0-9]+$ ]] && [[ -r "/proc/$recovery_pid/cmdline" ]]; then
      command_line="$(tr '\0' ' ' <"/proc/$recovery_pid/cmdline")"
      if [[ "$command_line" == *"launch-rust-recovery $operation --foreground"* ]]; then
        kill -TERM -- "-$recovery_pid" 2>/dev/null || true
        for _ in {1..50}; do
          kill -0 -- "-$recovery_pid" 2>/dev/null || break
          sleep .1
        done
        if kill -0 -- "-$recovery_pid" 2>/dev/null; then
          kill -KILL -- "-$recovery_pid" 2>/dev/null || true
        fi
      fi
    fi
  fi
  exit 0
fi
if [[ "${2:-}" != --foreground ]]; then
  [[ -f "$state/config.json" && ! -L "$state/config.json" ]]
  nohup setsid "$0" "$operation" --foreground >>"$state/helper.log" 2>&1 </dev/null &
  exit 0
fi
exec 9>"$state/runner.lock"
flock -n 9 || exit 0
if [[ -f "$state/cancelled" ]]; then exit 0; fi
if [[ -f "$state/uploaded" || -f "$state/restored" ]]; then exit 0; fi
printf '%s\n' "$$" >"$state/pid"
trap 'rm -f -- "$state/pid"' EXIT
export ZUSE_USER_DATA="${ZUSE_USER_DATA:-/home/zuse/.zuse-data}"
export ZUSE_ARCHIVE_CONFIG="$(cat "$state/config.json")"
unset BOXD_API_KEY CLOUD_DATA_ENCRYPTION_KEY CLOUD_CREDENTIAL_VAULT_KEY RELAY_MINT_PRIVATE_JWK WORKOS_API_KEY
binary="${ZUSE_RUNTIME_BINARY:-/usr/local/bin/zuse-cloud-runtime}"
if "$binary" recover >"$state/result.next"; then
  if grep -qx 'workspace_archive_uploaded' "$state/result.next"; then
    mv -- "$state/result.next" "$state/uploaded"
  elif grep -qx 'workspace_archive_restored' "$state/result.next"; then
    mv -- "$state/result.next" "$state/restored"
  else
    exit 1
  fi
else
  printf '%s\n' archive-upload-failed >"$state/failed"
  exit 1
fi
