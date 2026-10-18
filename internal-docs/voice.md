# Voice calls

Native and browser calls use the same host-owned Voice V2 protocol and shared caption, speaker activity and orb presentation state. A call captures its execution host at start, independently of the conversation subsequently selected. The host creates or resumes its projectless Codex orchestrator; starting voice does not submit an ordinary chat or queued agent command.

## Browser implementation

The header microphone starts a call and can hide/show the stage while the call continues. The stage provides captions, activity, mute and hangup; the header keeps hangup available while the stage is hidden. Local errors return to the composer with dismissible guidance.

The browser requests audio capture, immediately disables its tracks, and prepares a WebRTC peer. It acquires the host lease and ownership subscription before sending its SDP offer. Capture and speaker output stay muted through answer application and host confirmation. Activation applies the latest local mute choice, including changes made while audio-context activation was pending. SDP, PCM and local media handles are transient.

Each callback and signaling receipt is bound to the initiating attempt, RPC and host lease. A canceled permission prompt, delayed SDP, obsolete ownership event or lost acknowledgement cannot activate a later call. Setup stages and media reports have receipt deadlines; unrelated RPC responses remain available to their owners. Owner loss, disconnection, page exit and hangup close tracks, peer connections, nodes and pending listeners. The JavaScript adapter also has an independent capture watchdog: host report acknowledgements renew a 15-second active lease, so capture closes if the UI stops processing host receipts. No call is automatically replayed after reconnect.

The shared orb uses the browser monotonic clock and honors reduced motion. Hidden stages pause animation updates. Native builds retain the existing async orchestrator API and share presentation state without pulling native audio/async dependencies into WASM.

## Conversation continuity

A long voice transcript rotates into a fresh segment while continuing its verified provider session. Cursor ownership, cursor value and working directory are stored together in the workspace registry and survive snapshot restoration. Rotation carries only verified ownership from durable metadata or the source typed journal; it does not manufacture transcript events. Legacy cursors with unknown ownership cannot authorize a provider resume. Failed preparation removes the empty successor while preserving the previous segment for retry.

## Evidence and remaining acceptance

- Seven JavaScript adapter scenarios cover mute/activation ordering, cancellation, pending-listener cleanup, newest-mute wins, pending permission mute, permission refusal and watchdog expiry.
- Three GPUI controller scenarios cover host-scoped signaling, owner/lease ordering, cleanup, obsolete callbacks and late receipts without consuming unrelated responses.
- Seventeen shared orb/session scenarios preserve native presentation and orchestration behavior.
- Engine Voice V2 integration exercises preparation, negotiation, ownership, cancellation, cross-engine routing and failed-rotation retry. Cursor tests cover durable binding, foreign-provider refusal and inheritance without fabricated events.
- The compiled GPUI browser completes a call against a real local WebRTC peer, with Chromium fake capture and scripted Voice V2 host responses. Capture remains muted through host confirmation, activates afterward and ends on hangup; the host receives one stop. A headed GPU capture renders the stage. This is browser/media integration evidence, not live provider acceptance.

Live Codex authorization and voice, physical microphone/speaker/AEC behavior, device selection, voice settings, exact native/browser stage visual parity and current physical Mac/Linux acceptance remain open. Hosted CI currently cannot start because of the GitHub account billing limit.
