# Cloud Bridge

Cloud chats can run commands on the owner's own computers. Each command carries a fixed computer enrollment, chat, command text, working directory and deadline. Every computer runs an enrolled connector that makes outbound HTTPS requests to the cloud API with its own proof key; it never accepts inbound connections.

The bridge is independent of remote access. It uses none of the pairing, relay, direct-route, tunnel, SSH or device-link code or keys, and its settings live in their own section. A test fails if the bridge modules reference those paths.

## Which computer runs a command

The user never attaches computers. Every computer they enrolled is available, and every command still needs their approval.

- **The computer the user is typing on** is the default. After each message sent from the desktop app is admitted (and after a new chat is created), the app proves with that computer's own enrolled key that the message came from there (`POST /v1/cloud/bridge/primary` with the message's `commandId`). Claims are ordered by that message's admission, so a delayed claim never replaces a newer one, and a message that wasn't admitted never moves the default. The server derives the device from the key, never from a client-supplied ID. If the sending computer isn't enabled, the chat has no default until the user sends from one that is. When the next message comes from another enabled computer, the default follows it. Messages sent from mobile or web don't change it.
- **The user's other enrolled computers** run a command only when the agent names one: the user asked for that computer, or the task can only be done there. It is never a default or a fallback.

User-facing text always uses the computer's own name (its device name at enrollment, editable in Settings), never a role. The agent tools carry a `role` flag (`primary` for the computer the user is typing on, `secondary` otherwise). `run_on_computer` without a `device_id` uses the computer the user is typing on; with none it fails with `no_primary_computer` and the agent asks the user. Approvals are per computer: "this chat" or "always" for one computer never covers another.

## Start a connector

**Desktop app.** Settings → Cloud computer → **Your computers** shows this computer's connector with its status, a single Allow/Stop action, and the enrolled computers with rename, revoke and remove. It is not part of remote access: the bridge has its own enrollments, IDs, keys and routes, and names come only from bridge enrollment (the OS host name by default). On the web, Cloud → Setup lists the same computers.

Allowing enrolls the computer and starts the engine-owned connector. The choice is saved in the profile (`computer-bridge/connector.json`), so the connector comes back after an app or engine restart. It is bound to the account that allowed it: while another account (or none) is signed in it pauses, and it resumes when that account returns. Network failures, sleep and Wi-Fi changes reconnect with exponential backoff (1 s doubling to 30 s, reset after a minute online). Settings shows `online`, `starting`, `reconnecting` (with the reason) or `paused`. **Stop** stops running commands, stops admission and forgets the choice.

Allowing never runs anything by itself: every command still needs approval (or an earlier "this chat" / "always" grant for that computer).

**Headless computers.** On a CLI profile:

```sh
zuse bridge-enroll --name "Build box"
zuse bridge
```

Without a saved account session, `bridge-enroll` prints a sign-in address and code. Approve it from any signed-in device (OAuth device authorization), and the computer enrolls. `zuse bridge` runs until interrupted, reconnects with the same backoff, and stops running commands on Ctrl-C. The standalone connector holds the profile lock, so it cannot run beside an engine on the same profile.

Unix computers execute through /bin/sh. Windows computers use Windows PowerShell. The default working directory is the host user's home directory. An explicit directory belongs to that host.

For a host-defined task with fixed arguments, run `zuse bridge-task /absolute/path/to/executable argument` from its working directory.

## Approve

A request replaces the composer, exactly like a local permission prompt (Esc denies, Cmd/Ctrl+Enter allows once). It shows the computer's name and icon, the working directory and the command:

- **Allow once** admits that exact request.
- **Allow for this chat** permits later commands from this chat on this computer.
- **Allow always** permits later commands from this owner's chats on this computer.
- **Deny** prevents the request from being claimed.

In the transcript, a command run on one of the user's computers shows the computer's icon before the tool icon, and its name. Mobile and web show requests and results under **Computers**, with **Stop** on running ones. A chat can hold at most 10 undecided requests.

Revoking an "always" grant returns requests it approved that haven't started to waiting for a decision, and running ones stop at their next check. Archiving a chat clears its requests and grants. Removing an enrollment clears its grants and requests. Re-enrollment after removal receives a new enrollment ID.

## Agent tools

- `list_attached_computers` returns the user's enrolled computers, the one they are typing on first, online or not, with `role`, device ID, name, platform, shell, OS user, default working directory and deadline limits.
- `run_on_computer` accepts `command`, optional `device_id` (omit for the computer the user is typing on), optional `cwd` and optional `timeout_seconds`, returning the request ID, device and role.
- `computer_task_results` returns recent requests newest first with `state` (`pending`, `approved`, `running`, `finished`, `denied`, `cancelled`, `unknown`) and bounded output, including output so far while running.
- `list_computer_tasks` and `run_computer_task` cover host-defined tasks.

Plan-mode chats can discover and read results. Command execution requires leaving plan mode.

The runtime derives the account and workspace from its enrolled credential and validates the active chat. Approval and enrollment APIs remain owner-authenticated (personal accounts only). Requests are bound to the owner, chat and enrollment; another owner's enrollment is never reachable.

## Execution and recovery

Each command has its own deadline: 10 minutes by default, up to 60 minutes when requested (`timeout_seconds`, 1–3600). The server stores it with the request and the host enforces it; the server also stops authorizing a run once its deadline (plus the unknown window) has passed. A computer runs up to four approved commands at once.

The host saves an admission receipt before spawning, then writes completed output to its private profile journal. A database claim is consumed once. Submissions use stable request IDs: a retry is recognized even if the computer went offline or restarted meanwhile, and an ID is remembered for the life of the chat, so it can never run again after its records are removed. A reconnecting connector gets a new offer ID but still reaches requests approved for the same computer and shell. A result is deleted from the host's journal only once the cloud has it; a refused proof (for example after clock skew) or sign-out keeps it for the next session.

While a command runs, the host rechecks its authorization and publishes output every two seconds. Output keeps the first 4 KiB and the last 8 KiB of each stream, noting how much was omitted. Stop, detachment, revocation, sign-out, the deadline and connector shutdown terminate the process group and record why. A host that can't reach the cloud for 60 seconds stops its commands, since it can no longer confirm their approval.

A claimed request with no report for 80 seconds, or past its deadline, shows **Outcome unknown**. An uncertain run is never automatically re-executed, and the agent is told not to resubmit it. On restart, the connector republishes saved completed results; an admitted request without a saved result keeps its unknown outcome.

## Sync into your checkout

A cloud chat's files can be put into the user's own checkout of the same repository on the computer they type on, so a dev server, containers or builds already running in that folder pick up what the agent changes. It is one way: edits made in the folder are never sent back. It needs this computer's connector to be allowed.

- **Turning it on** (computer menu → **Sync into your checkout**) uses the project Zuse already knows for that repository on this computer (matched by its GitHub owner and name), or asks for the folder. The engine checks the folder is the repository's top folder, that its remote is the chat's repository and that no merge, rebase or similar is in progress.
- **Saving.** Before any file changes, the checkout's branch, HEAD and tracked changes (a stash-style commit, staged and unstaged) are recorded under hidden refs (`refs/zuse/checkout-sync/*`) and in the profile (`checkout-sync/<folder hash>.json`, written and synced to disk first). Only one chat is synced into a folder; turning it on for another chat puts the folder back first, then syncs the new one.
- **What syncs.** The chat's files come from the same snapshot transfer as "Sync to a local directory" (git-selected: tracked files plus new files git doesn't ignore, as the chat's working tree has them, including deletions). Bytes this checkout already has are never sent again. The engine writes them as a checkpoint commit (parent: the saved HEAD; submodule pointers stay as the checkout had them) and checks it out detached with `git read-tree -m -u`, which refuses to overwrite untracked or ignored files. `node_modules`, `target`, `.env` and other untracked or ignored files in the folder are never written, moved or removed.
- **While on**, a new snapshot is taken every 3 seconds after a change, backing off to 30 seconds while nothing changes (sooner on a wake). It never wakes the cloud computer: while it sleeps or isn't connected the status is "Waiting for the cloud computer". Edits made in the folder (or checking out another branch) pause sync instead of being overwritten; **Keep my edits aside and resume** moves them to the folder's stash list and continues. The menu shows the state and a badge beside the header's cloud icon shows the folder while synced.
- **Turning it off** — or archiving the chat, stopping this computer's connector, or switching accounts — puts back the branch, its commit and the saved tracked changes, staged and unstaged as they were. Edits made while synced go to the stash list first. If the branch moved meanwhile and the earlier changes no longer apply, they stay in the stash list. Putting it back is recorded before it starts and finishes after an app restart.
- **Commands start there.** When the computer proves a message came from it (`POST /v1/cloud/bridge/primary`) it also sends `syncedCwd`, the folder the chat is synced into there (also sent, without moving the default computer, when sync turns on or off). The server keeps it per chat and computer (`api_bridge_attachments.synced_cwd`, migration 0056). A command on that computer without an explicit `cwd` starts there, so the approval shows that exact folder. `list_attached_computers` returns it as `syncedCheckout` (and as `cwd`), and `run_on_computer` returns the `cwd` it will use.

## Deployment

Sync into your checkout adds migration 0056 (additive: one nullable column, `api_bridge_attachments.synced_cwd`, and its column grant). It uses the existing snapshot RPC, so cloud runtimes need no update.

The cloud API requires runtime writes and migration 0055 (additive: new `api_bridge_*` tables, including single-use connector proofs kept five minutes, one trigger function and two triggers that clear attachments when a chat or computer is archived). It does not need the native device authority. Runtime images need the matching guest runtime for the agent tools. Physical macOS and Windows acceptance remains pending.
