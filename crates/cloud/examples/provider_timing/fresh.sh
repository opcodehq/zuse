. /tmp/zuse-timing-common.sh
t=$(now); as_task /usr/local/bin/zuse-cloud-runtime --version >/dev/null; runtime=$(( $(now)-t ))
t=$(now); as_task "$CLAUDE" --version >/dev/null 2>&1; claude=$(( $(now)-t ))
codex=$(agent_start)
codex_warm=$(agent_start)
d=$(as_task mktemp -d); t=$(now); as_task git clone -q --depth 1 https://github.com/tokio-rs/tokio "$d/repo" >/dev/null 2>&1 && clone=$(( $(now)-t )) || clone=-1
files=$(as_task find "$d/repo" -type f 2>/dev/null | wc -l)
printf '{"runtimeExecMs":%s,"claudeColdVersionMs":%s,"codexColdInitializeMs":%s,"codexWarmInitializeMs":%s,"shallowCloneMs":%s,"cloneFiles":%s,"cpus":%s}\n' "$runtime" "$claude" "$codex" "$codex_warm" "$clone" "$files" "$(nproc)"
