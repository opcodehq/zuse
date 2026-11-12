. /tmp/zuse-timing-common.sh
# What the guest now does: read the agent binary while the checkout runs.
t0=$(now)
( t=$(now); as_task cat "$CODEX" > /dev/null; echo $(( $(now)-t )) > /tmp/zuse-warm-ms ) &
warm_pid=$!
d=$(as_task mktemp -d); t=$(now); as_task git clone -q --depth 1 https://github.com/tokio-rs/tokio "$d/repo" >/dev/null 2>&1 && clone=$(( $(now)-t )) || clone=-1
wait "$warm_pid"
both=$(( $(now)-t0 ))
codex=$(agent_start)
printf '{"shallowCloneMs":%s,"warmReadMs":%s,"cloneAndWarmMs":%s,"codexInitializeAfterWarmMs":%s,"checkoutToAgentReadyMs":%s}\n' "$clone" "$(cat /tmp/zuse-warm-ms)" "$both" "$codex" "$(( both + codex ))"
