now(){ date +%s%3N; }
as_task(){ if id zuse >/dev/null 2>&1 && [ "$(id -un)" != zuse ]; then sudo -n -u zuse env HOME=/home/zuse PATH=/usr/local/bin:/usr/bin:/bin "$@"; else "$@"; fi; }
CODEX=$(find /opt/zuse/harnesses/releases -maxdepth 5 -type f -name codex -perm -u+x 2>/dev/null | head -1)
CLAUDE=$(find /opt/zuse/harnesses/releases -maxdepth 5 -type f -name claude -perm -u+x 2>/dev/null | head -1)
# Spawn the Codex app-server and time until it answers initialize.
agent_start(){
  local t=$(now) line=""
  coproc CX { as_task "$CODEX" app-server 2>/dev/null; }
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"zuse-probe","version":"0"},"capabilities":{"experimentalApi":true}}}' >&"${CX[1]}"
  read -t 60 -r line <&"${CX[0]}"
  local ms=$(( $(now)-t ))
  kill "$CX_PID" 2>/dev/null; as_task pkill -f 'codex app-server' 2>/dev/null; wait "$CX_PID" 2>/dev/null
  case "$line" in *'"id":1'*) echo "$ms";; *) echo "-1";; esac
}
# True when PID $1 is the Codex app-server itself, not the shell that started it.
is_codex(){ [ -n "$1" ] && [ "$(tr '\0' '\n' </proc/"$1"/cmdline 2>/dev/null | head -2 | tr '\n' ' ')" = "$CODEX app-server " ]; }
# PID of the running Codex app-server, if any.
codex_pid(){ local p; for p in $(pgrep -f 'codex app-server'); do is_codex "$p" && { echo "$p"; return; }; done; }
