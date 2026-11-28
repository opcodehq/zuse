-- Every computer whose provider machine may be awake, with the last time
-- anything happened on it and, when the max-quiet-awake backstop must not act,
-- why. The backstop deliberately ignores the eligibility in
-- idle_candidates.sql: if that query misses a computer for any reason, this
-- one still sees it. $1 account filter, $3 now, $4 optional workspace.
--
-- quiet_since: the latest user activity, command write, lifecycle admission or
-- launch request. hold:
--   NULL                  the backstop may sleep it once quiet long enough
--   live-turn             a turn is running (an early launch turn may run
--                         during setup) and its runtime renewed its
--                         credential within the credential lifetime: never
--                         interrupted
--   recreation, repair,   another operation owns the machine identity and
--   memory-recovery       has its own attention state; reported, not raced
--   no-retained-storage   no retained disk to pause; reported only
-- lost_at: with a turn still recorded as starting, running or waiting for
-- input but no live runtime to run it (hold is not live-turn), when that
-- runtime was last known alive: the end of its credential's lifetime, or the
-- latest activity if later. The backstop sleeps such a computer shortly after
-- instead of a full quiet bound later. NULL without such a turn.
SELECT w.workspace_id,
  GREATEST(w.last_activity_at, COALESCE(e.updated_at,0), COALESCE(l.created_at,0), COALESCE(i.created_at,0))::bigint AS quiet_since,
  CASE
    WHEN w.state IN ('ready','setup') AND w.runtime_state<>'offline'
     AND COALESCE((w.request_config->>'runtimeCredentialExpiresAtMs')::bigint,0)>$3
     AND EXISTS(SELECT 1 FROM zuse_rust_task_command_execution x WHERE x.workspace_id=w.workspace_id
       AND x.state IN('starting','running','waiting-input')
       AND x.runtime_generation=COALESCE((w.request_config->>'runtimeGeneration')::bigint,1))
      THEN 'live-turn'
    WHEN w.request_config#>>'{rustRecreation,operationId}' IS NOT NULL
     AND COALESCE(w.request_config#>>'{rustRecreation,phase}','') NOT IN ('ready','cancelled')
      THEN 'recreation'
    WHEN COALESCE(w.request_config#>>'{rustRuntimeRepair,phase}','ready')<>'ready' THEN 'repair'
    WHEN w.request_config#>>'{rustMemoryTransportRecovery,operationId}'=w.request_config#>>'{rustLifecycleOperation,id}' THEN 'memory-recovery'
    WHEN w.request_config#>>'{rustRuntimeStorage,storageIncarnationId}' IS NULL THEN 'no-retained-storage'
  END AS hold,
  CASE
    WHEN EXISTS(SELECT 1 FROM zuse_rust_task_command_execution x WHERE x.workspace_id=w.workspace_id
       AND x.state IN('starting','running','waiting-input'))
      THEN GREATEST(w.last_activity_at, COALESCE(e.updated_at,0), COALESCE(l.created_at,0), COALESCE(i.created_at,0),
        COALESCE((w.request_config->>'runtimeCredentialExpiresAtMs')::bigint,0))::bigint
  END AS lost_at
FROM api_cloud_workspaces w
JOIN zuse_rust_control_owners o
  ON o.resource_kind='workspace' AND o.resource_id=w.workspace_id
 AND o.backend='rust'
 AND o.generation=COALESCE((w.request_config->>'runtimeGeneration')::bigint,1)
LEFT JOIN LATERAL (SELECT max(x.updated_at) AS updated_at FROM zuse_rust_task_command_execution x WHERE x.workspace_id=w.workspace_id) e ON true
LEFT JOIN LATERAL (SELECT max(x.created_at) AS created_at FROM zuse_rust_lifecycle_operations x WHERE x.workspace_id=w.workspace_id) l ON true
LEFT JOIN api_cloud_workspace_launch_intents i ON i.workspace_id=w.workspace_id
WHERE ($1::text[] IS NULL OR w.account_id=ANY($1))
  AND ($4::text IS NULL OR w.workspace_id=$4)
  AND w.state IN ('provisioning','setup','ready','pausing')
  AND w.desired_state IN ('ready','paused')
  AND w.provider_sandbox_id IS NOT NULL
  AND w.request_config#>>'{cloudMailboxLifecyclePending,action}' IS DISTINCT FROM 'delete'
  AND w.request_config#>>'{cloudMailboxLifecycleDelivered,action}' IS DISTINCT FROM 'delete'
