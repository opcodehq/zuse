# Custom ACP agents

Settings → Providers → Custom agents connects locally installed agents that speak ACP over standard input/output. Native Settings targets the computer selected in its device picker; browser Settings targets its connected engine. Refresh loads that computer’s definitions. Add ACP agent opens the setup form; Edit updates the existing identity, and Remove removes its definition without deleting saved conversations.

Choose a unique lowercase `acp-` identity, a display name, and an executable path or command available on the execution computer’s PATH. Put one argument per line. Values are passed directly to the executable with no shell expansion. Empty or multiline arguments can use JSON string quotes; existing quoted/multiline values are encoded on edit. Environment entries use `NAME=value`, one per line, with the same optional JSON string encoding for values. Duplicate keys and overrides of Zuse’s chat/device/IPC origin are rejected. Optional fallback model IDs appear when the agent does not advertise a catalog.

The executable must already be installed. Zuse does not install or automatically update arbitrary custom commands. A missing executable keeps the provider unavailable until installed. Authentication belongs to that agent’s own login/configuration or its configured environment. The custom provider uses the common ACP engine for model discovery, sessions, streamed turns, approvals, uploads, tool bridges and turn-boundary queue delivery. Defining several providers gives each a distinct identity and configuration; ordinary model/agent selection controls can select them independently.

## Storage and concurrency

Definitions live in the engine profile’s private `custom-acp-agents.json`, separate from synchronized workspace documents. Explicit setup RPCs return the private launch configuration, including environment values, to the connected client. Browser/LAN and paired clients use the existing authenticated transports; native engine IPC retains its existing trusted local-process boundary. Ordinary provider descriptors and persisted model catalog identity contain no launch environment values. Configuration changes alter the catalog fingerprint, preventing reuse across different launch credentials.

GetCustomAcpAgents returns `{revision, agents}`. SetCustomAcpAgent accepts `{agent, expectedRevision}`; DeleteCustomAcpAgent accepts `{harness, expectedRevision}`. All three support `targetDeviceId` forwarding. Mutation replies contain the new snapshot. Refresh after a revision conflict; mutations are never automatically replayed after disconnect. A disconnected browser clears its loaded configuration and draft credentials before reconnecting.

Changes serialize and wait for the existing provider’s execution lease. Idle warm agents retire at the boundary; a busy provider returns an actionable error after the bounded wait. A successful save uses a private temporary file, fsync and atomic replacement before publishing the registry entry. Malformed on-disk configurations are preserved and refuse API overwrites. Direct file edits require an engine restart; use the setup APIs while the engine is running.

The shared wire protocol bounds identity length, executable/argument/environment sizes and provider count. Removing an agent preserves chat references to its ID; those chats cannot launch it until a definition with the same ID is restored. Changing a name or executable through Edit retains the ID.

## Acceptance

The deterministic subprocess scenario runs two independent ACP providers concurrently, discovers separate models, drains three queued messages per conversation and verifies reply isolation. Storage scenarios cover revision conflicts, private file permissions, restart loading, malformed-file preservation and cancellation while waiting for a runtime lease. Protocol scenarios cover stable built-in IDs, invalid custom IDs, literal arguments and lossless empty/multiline setup values.

These fixtures do not prove compatibility with every third-party ACP executable, live provider authorization, hosted plugin execution or physical macOS/Linux UI behavior. Those remain acceptance gates in `acceptance.md`.
