# Cloud Bridge

Cloud chats can run commands on an owner's explicitly attached computers. Each command carries a fixed computer enrollment, chat, command text and working directory. The bridge uses its own enrolled proof key and outbound HTTPS requests.

## Start a connector

In native Settings → Devices, load Cloud chat computers and choose **Allow cloud chats to use this computer**. The engine enrolls the computer and owns its background connector. **Stop computer connector** stops admission and active work. The connector retries after a network failure and stops if the account changes.

On an already signed-in CLI profile:

```sh
zuse bridge-enroll --name "Build Mac"
zuse bridge
```

The standalone connector holds the profile lock. Use the engine-owned connector while the desktop app is open. A CLI profile needs a saved general-account session. A standalone headless sign-in-code enrollment flow remains pending.

Unix computers execute through /bin/sh. Windows computers use Windows PowerShell. The default working directory is the host user's home directory. An explicit directory belongs to that host.

For a host-defined task with fixed arguments, run `zuse bridge-task /absolute/path/to/executable argument` from its working directory.

## Attach and approve

In the mobile or web cloud-chat composer, open **Computers** and attach one or more enrolled computers. Native Settings → Devices exposes attachment and approval controls for the selected cloud chat.

- **Approve once** admits that exact request.
- **Approve for this session** permits later commands from this chat on this computer.
- **Approve always** permits later commands from this owner's chats on this computer.
- **Deny** prevents the request from being claimed.

Detach clears the chat's requests and session grant. Persistent grants have a separate revoke action. Archiving a chat clears attachments. Removing an enrollment clears its grants and attachments. Re-enrollment receives a new enrollment ID.

## Agent tools

`list_attached_computers` returns online computers with their device ID, shell, OS user, and default working directory. `run_on_computer` accepts device_id, command, and optional cwd, returning a request ID. `computer_task_results` returns recent results and bounded intermediate output. Plan-mode chats can discover and read results. Command execution requires leaving plan mode.

The runtime derives the account and workspace from its enrolled credential and validates the active chat. Approval and enrollment APIs remain owner-authenticated. The tools also support host-defined tasks through `list_computer_tasks` and `run_computer_task`.

## Execution and recovery

The host saves an admission receipt before spawning, then writes completed output to its private profile journal. A database claim is consumed once. Submissions use stable request IDs and recognize duplicate submissions.

The host checks authorization and publishes output every two seconds during execution. Each stream retains its first 12 KiB. Mobile and web result cards offer **Stop**. Detachment, revocation, sign-out, timeout and connector shutdown terminate the process group. Commands currently have a 60-second deadline.

If a claimed request has no confirmed result after 80 seconds, the result shows **Outcome unknown**. An uncertain run is never automatically re-executed. On restart, the connector republishes saved completed results. An admitted request without a saved result retains its unknown outcome.

The cloud API requires the device authority, runtime writes and migration 0055. Physical macOS and Windows acceptance, automatic verified “this computer” selection, and dedicated native composer controls remain pending.
