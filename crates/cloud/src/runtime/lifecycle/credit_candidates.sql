SELECT w.workspace_id
FROM api_cloud_workspaces w
JOIN zuse_rust_control_owners o
  ON o.resource_kind='workspace' AND o.resource_id=w.workspace_id
 AND o.backend='rust'
 AND o.generation=COALESCE((w.request_config->>'runtimeGeneration')::bigint,1)
WHERE w.account_id=ANY($1)
  AND w.request_config->>'providerConnectionId' IS NULL
  AND w.state='ready' AND w.desired_state='ready' AND w.runtime_state='online'
  AND w.next_action_at<=$3
  AND w.request_config->>'commandDeliveryMode'=$4
  AND w.request_config->>'rustCommandAdmissionVersion'='2'
  AND w.request_config->>'rustCommandMailboxEnabled'='true'
  AND w.request_config->>'runtimeRpcProtocol'='zuse-rpc-v1'
  AND w.request_config#>>'{rustStorageEnrollment,phase}'='committed'
  AND w.request_config#>>'{runtimeBootstrapReceipt,workspaceId}'=w.workspace_id
  AND w.request_config#>>'{runtimeBootstrapReceipt,generation}'=COALESCE(w.request_config->>'runtimeGeneration','1')
  AND w.request_config#>>'{runtimeBootstrapReceipt,gatewayEpoch}'=COALESCE(w.request_config->>'gatewayEpoch',w.request_config->>'runtimeGeneration','1')
  AND w.request_config#>>'{rustStorageEnrollment,generation}'=COALESCE(w.request_config->>'runtimeGeneration','1')
  AND w.request_config#>>'{rustStorageEnrollment,sourceSandboxId}'=w.provider_sandbox_id
  AND w.request_config#>>'{rustStorageEnrollment,storageIncarnationId}'=w.request_config#>>'{rustRuntimeStorage,storageIncarnationId}'
  AND w.request_config#>>'{rustStorageEnrollment,credentialKeyThumbprint}'=w.request_config->>'runtimeCredentialKeyThumbprint'
  AND w.request_config#>>'{rustStorageEnrollment,signingKeyThumbprint}'=w.request_config->>'runtimeSigningKeyThumbprint'
  AND w.request_config#>'{rustStorageEnrollment,capabilities}' @> '["rust-workspace-lifecycle-v1","rust-session-commands-v1","rust-task-command-admission-v2"]'::jsonb
  AND COALESCE(w.request_config->>'runtimeReleaseChangeRequested','false')<>'true'
  AND COALESCE(w.request_config#>>'{runtimeActivation,phase}','') NOT IN ('preparing','launching','confirming','rolling-back')
  AND (w.request_config#>>'{rustRecreation,operationId}' IS NULL OR w.request_config#>>'{rustRecreation,phase}' IN ('ready','cancelled'))
  AND COALESCE(w.request_config#>>'{rustLifecycleOperation,phase}','ready')='ready'
  AND EXISTS(SELECT 1 FROM api_cloud_billing_periods p WHERE p.account_id=w.account_id AND p.shared_credit_account AND p.shared_credit_hold AND p.period_start<=$2 AND p.period_end>$2)
ORDER BY w.next_action_at,w.workspace_id
LIMIT 1
