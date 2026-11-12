. /tmp/zuse-timing-common.sh
cat /proc/sys/kernel/random/boot_id > $HOME/.zuse-boot-id
# A resident agent, as the runtime keeps one between turns.
as_task setsid bash -c "sleep 1000000 | $CODEX app-server >/dev/null 2>&1" </dev/null >/dev/null 2>&1 &
sleep 2
codex_pid > $HOME/.zuse-agent-pid
pgrep -f 'zuse-cloud-runtime' >/dev/null && r=true || r=false
printf '{"agentPid":"%s","runtimeProcessBeforeSleep":%s}\n' "$(cat $HOME/.zuse-agent-pid)" "$r"
