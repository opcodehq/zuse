#!/bin/sh
set -eu
# Production/default startup selects a certified channel. An exact candidate pin
# is an explicit owner-only staging bootstrapping path, never stable promotion.
root="${ZUSE_HARNESS_ROOT:-/opt/zuse/harnesses}"
if [ -n "${ZUSE_AUTH_CANDIDATE_RELEASE:-}" ]; then
  [ "${ZUSE_DEPLOYMENT_ENVIRONMENT:-}" = staging ] || { echo 'Candidate authority requires staging.' >&2; exit 1; }
  candidate="$ZUSE_AUTH_CANDIDATE_RELEASE"
  [ "${#candidate}" -eq 64 ] || exit 1
  case "$candidate" in *[!0-9a-f]*) exit 1;; esac
  owner="${ZUSE_AUTH_CANDIDATE_ACCOUNT_ID:?Set one staging owner account}"
  case "$owner" in ''|*','*|*'*'*|*' '*|*'
'*) exit 1;; esac
  [ "${ZUSE_CLOUD_AUTH_WRITE_ACCOUNTS:-}" = "$owner" ] || { echo 'Candidate authority requires the exact owner-only auth cohort.' >&2; exit 1; }
  [ -z "${ZUSE_CLOUD_CREATE_ACCOUNTS:-}" ] || [ "$ZUSE_CLOUD_CREATE_ACCOUNTS" = "$owner" ] || { echo 'Candidate authority cannot widen the compute cohort.' >&2; exit 1; }
  python3 /usr/local/lib/zuse/harness-release.py --root "$root" verify --release "$candidate" >/dev/null
  pin="$(mktemp /tmp/zuse-authority-candidate.XXXXXX)"
  printf '{"release":"%s"}\n' "$candidate" > "$pin"
  chmod 400 "$pin"
  exec python3 /usr/local/lib/zuse/harness-release.py --root "$root" exec \
    --pin "$pin" --runtime-version "${ZUSE_AUTH_RUNTIME_VERSION:?Set the tested runtime version}" \
    -- /usr/local/bin/zuse-cloud-api "$@"
fi
exec python3 /usr/local/lib/zuse/harness-release.py --root "$root" exec \
  --channel "${ZUSE_HARNESS_CHANNEL:-staging}" \
  --runtime-version "${ZUSE_AUTH_RUNTIME_VERSION:?Set the tested runtime version}" \
  -- /usr/local/bin/zuse-cloud-api "$@"
