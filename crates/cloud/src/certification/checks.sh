# Machine certification checks, run inside a managed-base computer by the
# certification runner. Usage: bash checks.sh fresh|before|resume
# Each mode prints exactly one JSON line. State lives under $HOME because Boat
# keeps only home directories on a cold restore.
set -u
mode="${1:?mode required}"
now() { date +%s%3N; }
as_task() {
  if id zuse >/dev/null 2>&1 && [ "$(id -un)" != zuse ]; then
    sudo -n -u zuse env HOME=/home/zuse PATH=/usr/local/bin:/usr/bin:/bin "$@"
  else
    "$@"
  fi
}
ok() { if "$@" >/dev/null 2>&1; then echo true; else echo false; fi; }
CODEX=$(find /opt/zuse/harnesses/releases -maxdepth 5 -type f -name codex -perm -u+x 2>/dev/null | head -1)
CLAUDE=$(find /opt/zuse/harnesses/releases -maxdepth 5 -type f -name claude -perm -u+x 2>/dev/null | head -1)
STATE="$HOME/.zuse-certification"

# True when PID $1 is the Codex app-server itself, not the shell that started it.
is_codex() { [ -n "$1" ] && [ "$(tr '\0' '\n' </proc/"$1"/cmdline 2>/dev/null | head -2 | tr '\n' ' ')" = "$CODEX app-server " ]; }
codex_pid() { local p; for p in $(pgrep -u "$1" -f 'codex app-server'); do is_codex "$p" && { echo "$p"; return; }; done; }

# Spawn the Codex app-server and time until it answers initialize; -1 if it never does.
codex_start() {
  [ -n "$CODEX" ] || { echo -1; return; }
  local t line=""
  t=$(now)
  coproc CX { as_task "$CODEX" app-server 2>/dev/null; }
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"zuse-certification","version":"1"},"capabilities":{"experimentalApi":true}}}' >&"${CX[1]}"
  read -t 60 -r line <&"${CX[0]}"
  local ms=$(( $(now) - t ))
  kill "$CX_PID" 2>/dev/null
  wait "$CX_PID" 2>/dev/null
  # Only a successful initialize counts; an error response also carries id 1.
  case "$line" in
  *'"error"'*) echo -1 ;;
  *'"id":1'*'"result"'* | *'"result"'*'"id":1'*) echo "$ms" ;;
  *) echo -1 ;;
  esac
}

case "$mode" in
fresh)
  runtime_sha=$(sha256sum /usr/local/bin/zuse-cloud-runtime 2>/dev/null | cut -d' ' -f1)
  runtime_version=$(as_task /usr/local/bin/zuse-cloud-runtime --version 2>/dev/null)
  claude_ok=$(ok as_task "$CLAUDE" --version)
  codex_ms=$(codex_start)
  mkdir -p "$STATE"
  echo retained > "$STATE/disk-marker" && sync
  printf '{"runtimeSha256":"%s","runtimeVersion":"%s","claudeAnswers":%s,"codexInitializeMs":%s,"git":%s,"sudo":%s,"sshd":%s,"taskUser":%s,"cpus":%s}\n' \
    "$runtime_sha" "$runtime_version" "$claude_ok" "$codex_ms" \
    "$(ok git --version)" "$(ok as_task sudo -n true)" "$(ok test -x /usr/sbin/sshd)" "$(ok id zuse)" "$(nproc)"
  ;;
before)
  mkdir -p "$STATE"
  cat /proc/sys/kernel/random/boot_id > "$STATE/boot-id"
  # A resident agent, as the runtime keeps one between turns.
  as_task setsid bash -c "sleep 1000000 | $CODEX app-server >/dev/null 2>&1" </dev/null >/dev/null 2>&1 &
  sleep 2
  codex_pid zuse > "$STATE/agent-pid"
  printf '{"agentRunning":%s}\n' "$( [ -s "$STATE/agent-pid" ] && echo true || echo false )"
  ;;
resume)
  [ "$(cat /proc/sys/kernel/random/boot_id)" = "$(cat "$STATE/boot-id" 2>/dev/null)" ] && same_boot=true || same_boot=false
  pid=$(cat "$STATE/agent-pid" 2>/dev/null)
  # A new boot can reuse the PID, so survival needs the same boot and program.
  [ "$same_boot" = true ] && is_codex "$pid" && survived=true || survived=false
  restart=0
  [ "$survived" = false ] && restart=$(codex_start)
  [ "$(cat "$STATE/disk-marker" 2>/dev/null)" = retained ] && disk=true || disk=false
  as_task pkill -f 'codex app-server' >/dev/null 2>&1
  printf '{"sameBoot":%s,"agentSurvived":%s,"agentRestartMs":%s,"diskKept":%s}\n' "$same_boot" "$survived" "$restart" "$disk"
  ;;
*) echo '{"error":"unknown mode"}'; exit 2 ;;
esac
