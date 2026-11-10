. /tmp/zuse-timing-common.sh
[ "$(cat /proc/sys/kernel/random/boot_id)" = "$(cat $HOME/.zuse-boot-id)" ] && same_boot=true || same_boot=false
pid=$(cat $HOME/.zuse-agent-pid); [ -n "$pid" ] && [ -d "/proc/$pid" ] && alive=true || alive=false
restart=0; runtime=0
if [ "$alive" = false ]; then
  # Everything restarts: the runtime process, then the agent, from a cold disk.
  t=$(now); as_task /usr/local/bin/zuse-cloud-runtime --version >/dev/null; runtime=$(( $(now)-t ))
  restart=$(agent_start)
fi
printf '{"sameBoot":%s,"agentProcessSurvived":%s,"runtimeColdExecMs":%s,"agentRestartMs":%s}\n' "$same_boot" "$alive" "$runtime" "$restart"
