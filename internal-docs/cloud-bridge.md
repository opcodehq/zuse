# Cloud Bridge

Cloud chats can run commands on the owner's own computers. Each command carries a fixed computer enrollment, chat, command text, working directory and deadline. Every computer runs an enrolled connector that makes outbound HTTPS requests to the cloud API with its own proof key; it never accepts inbound connections.

The bridge is independent of remote access. It uses none of the pairing, relay, direct-route, tunnel, SSH or device-link code or keys, and its settings live in their own section. A test fails if the bridge modules reference those paths.

## Primary and secondary computers

- **Primary** is the computer the user is typing on. Each message sent from the desktop app first proves, with that computer's own enrolled key, that it comes from there (`POST /v1/cloud/bridge/primary`); the server derives the device from the key, never from a client-supplied ID. If the sending computer isn't enabled, the chat's primary is cleared. When the next message comes from another enabled computer, the primary follows it.
- **Secondaries** are the owner's other enrolled computers, attached to the chat explicitly. The agent may use one only when it clearly needs to: the user named that computer, or the task can only be done there. It is never a default or a fallback.

`run_on_computer` without a `device_id` runs on the primary; with no primary it fails with `no_primary_computer` and the agent asks the user. A secondary needs its explicit `device_id`, and its approval card says "(secondary)". Approvals are per computer: "this chat" or "always" for the primary never covers a secondary, and vice versa. When the primary moves, the previous one finishes its running work but is no longer a target unless it was attached as a secondary. Messages sent from mobile or web don't change the primary.

## Start a connector

**Desktop app.** In a cloud chat, open the **Computers** chip in the composer. When the app runs on an enrolled computer it lists **This computer** first, matched by its enrollment ID (not by name). If this computer isn't enrolled yet, choose **Allow cloud chats to use this computer**. Settings → Devices shows the same connector with its status and a single Allow/Stop action.

Allowing enrolls the computer and starts the engine-owned connector. The choice is saved in the profile (`computer-bridge/connector.json`), so the connector comes back after an app or engine restart. It is bound to the account that allowed it: while another account (or none) is signed in it pauses, and it resumes when that account returns. Network failures, sleep and Wi-Fi changes reconnect with exponential backoff (1 s doubling to 30 s, reset after a minute online). The composer chip and Settings show `online`, `starting`, `reconnecting` (with the reason) or `paused`. **Stop** stops running commands, stops admission and forgets the choice.

Allowing never runs anything by itself. An enabled computer becomes a chat's primary only when the user sends from it, attaching a secondary is a separate explicit choice per chat, and every command still needs approval (or an earlier "this session" / "always" grant for that computer).

**Headless computers.** On a CLI profile:

```sh
zuse bridge-enroll --name "Build box"
zuse bridge
```

Without a saved account session, `bridge-enroll` prints a sign-in address and code. Approve it from any signed-in device (OAuth device authorization), and the computer enrolls. `zuse bridge` runs until interrupted, reconnects with the same backoff, and stops running commands on Ctrl-C. The standalone connector holds the profile lock, so it cannot run beside an engine on the same profile.

Unix computers execute through /bin/sh. Windows computers use Windows PowerShell. The default working directory is the host user's home directory. An explicit directory belongs to that host.

For a host-defined task with fixed arguments, run `zuse bridge-task /absolute/path/to/executable argument` from its working directory.

## Attach and approve

In the desktop cloud-chat composer, the **Computers** chip shows the primary and attaches or detaches secondaries. Mobile and web cloud chats have attach controls under **Computers**.

Requests appear inline above the composer, like local permission prompts, showing the computer, OS user, shell, working directory, command and deadline:

- **Approve once** admits that exact request.
- **This chat** (approve for this session) permits later commands from this chat on this computer.
- **Always** permits later commands from this owner's chats on this computer.
- **Deny** prevents the request from being claimed.

Running commands show their latest output with **Stop**. A chat can hold at most 10 undecided requests.

Detach clears the chat's requests and session grant. Persistent grants have a separate revoke action; revoking returns requests it approved that haven't started to waiting for a decision, and running ones stop at their next check. Archiving a chat clears attachments. Removing an enrollment clears its grants and attachments. Re-enrollment after removal receives a new enrollment ID.

## Agent tools

- `list_attached_computers` returns the primary (first) and attached secondaries, online or not, with `role`, device ID, name, platform, shell, OS user, default working directory and deadline limits.
- `run_on_computer` accepts `command`, optional `device_id` (omit for the primary), optional `cwd` and optional `timeout_seconds`, returning the request ID, device and role.
- `computer_task_results` returns recent requests newest first with `state` (`pending`, `approved`, `running`, `finished`, `denied`, `cancelled`, `unknown`) and bounded output, including output so far while running.
- `list_computer_tasks` and `run_computer_task` cover host-defined tasks.

Plan-mode chats can discover and read results. Command execution requires leaving plan mode.

The runtime derives the account and workspace from its enrolled credential and validates the active chat. Approval and enrollment APIs remain owner-authenticated (personal accounts only). Requests are bound to the owner, chat and enrollment; another owner's enrollment is never reachable.

## Execution and recovery

Each command has its own deadline: 10 minutes by default, up to 60 minutes when requested (`timeout_seconds`, 1–3600). The server stores it with the request and the host enforces it; the server also stops authorizing a run once its deadline (plus the unknown window) has passed. A computer runs up to four approved commands at once.

The host saves an admission receipt before spawning, then writes completed output to its private profile journal. A database claim is consumed once. Submissions use stable request IDs: a retry is recognized even if the computer went offline or restarted meanwhile, and an ID is remembered for the life of the chat, so it can never run again after a detach and reattach. A reconnecting connector gets a new offer ID but still reaches requests approved for the same computer and shell. A result is deleted from the host's journal only once the cloud has it; a refused proof (for example after clock skew) or sign-out keeps it for the next session.

While a command runs, the host rechecks its authorization and publishes output every two seconds. Output keeps the first 4 KiB and the last 8 KiB of each stream, noting how much was omitted. Stop, detachment, revocation, sign-out, the deadline and connector shutdown terminate the process group and record why. A host that can't reach the cloud for 60 seconds stops its commands, since it can no longer confirm their approval.

A claimed request with no report for 80 seconds, or past its deadline, shows **Outcome unknown**. An uncertain run is never automatically re-executed, and the agent is told not to resubmit it. On restart, the connector republishes saved completed results; an admitted request without a saved result keeps its unknown outcome.

## Deployment

The cloud API requires runtime writes and migration 0055 (additive: new `api_bridge_*` tables, including single-use connector proofs kept five minutes, one trigger function and two triggers that clear attachments when a chat or computer is archived). It does not need the native device authority. Runtime images need the matching guest runtime for the agent tools. Physical macOS and Windows acceptance remains pending.
