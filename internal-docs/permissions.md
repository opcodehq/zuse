# Chat permissions

A new chat starts with the saved permission default. Its composer can select Ask for approval, Full access, or Read only. An existing chat retains its own configuration when the default changes. Read only remains selectable for existing restrictive workflows; it is not an automatic approval-judging mode.

The canonical run contract is `RunRequest.sandbox` plus `autoApprove`. Approval bypass requires **both** `danger-full-access` and `autoApprove: true`, expressed by `RunRequest::auto_approves_tools()`. A stale auto-approval flag cannot authorize bypass for a restricted run. The native composer and browser persist existing-chat changes through `setChatConfig`, so later queued turns use the saved posture. Unconfirmed saves fence new sends until the current mode has been read again.

| Selection | Codex adapter | Claude adapter | Zuse harness |
| --- | --- | --- | --- |
| Ask for approval | Workspace-write sandbox, network restricted, on-request approvals | Default CLI permission mode; permission callbacks ask through Zuse's input bridge | Approval-required tool policy |
| Full access | Danger-full-access sandbox and no wire approvals when explicitly opted in | Bypass permission mode when explicitly opted in | Full-access tool policy when explicitly opted in; sensitive-path checks still apply |
| Read only | Read-only sandbox; escalation approval requests declined | Plan permission mode; permission callbacks for tools are denied | Plan tool policy |

Claude's ordinary permission rules still apply before its callback. Its plan mode and permission callback are provider controls, rather than a uniform OS sandbox supplied by Zuse. The CLI sends write requests through the callback in plan mode; Zuse denies those requests. The adapter targets CLI 2.1.228, which is newer than the documented shell-write callback change in 2.1.212. See [Claude's permission evaluation and plan-mode documentation](https://code.claude.com/docs/en/agent-sdk/permissions).

Content questions remain interactive even in Full access. Tool approval questions accept only an explicit Yes for their own question ID; No, missing answers, and a dropped resolver decline the action. Read-only escalation requests do not ask the user to bypass that mode. Changes to the sandbox require a new native provider runtime, rather than silently reusing a process launched with different permissions.

ACP adapters select advertised permission modes through config options or the first-class session mode API. Restricted runs do not default to a bypass mode; Read only requires an advertised planning/read-only mode. An agent that rejects the selected mode cannot start the prompt. Permission callbacks are scoped to the current session: foreign and missing owners are cancelled, content questions remain interactive, and only explicit Full access automatically chooses an allow option. That automatic choice prefers one-time permission over a persistent rule and never falls back to an unknown option kind. See the [ACP session mode contract](https://agentclientprotocol.com/protocol/v1/session-modes).

The original automatic approval-judging mode, uniform OS isolation for Zuse tools, the remaining native SDK adapter permission audit, and physical desktop/live installed-provider acceptance remain open gates. Subprocess fixture coverage verifies wire policies, callback routing and decisions; it does not prove OS isolation of installed tools or every provider integration.
