# Interactive visuals

Agent `show_visual` publications are persisted as sandboxed Luau programs and assets attached to the conversation. The desktop retains its existing native visual renderer. The browser now opens the same programs through an execution-host runtime, instead of showing only a visual label.

## Browser behavior

Open visual loads a transient session. It renders stacks/grids, rich text, statistics, tables, buttons, tabs, switches, sliders, links, and bar/line/area/scatter charts. Charts support multiple series, gaps, negative values, explicit domains, stacking and horizontal bars. Canvas draw functions run on the execution host and return themed vector paths/text; browser canvases support pointer input, click handlers, responsive dimensions and paced animation. Closing or reopening a visual explicitly discards that view's interactive state. The stored program and assets remain available in conversation history.

Every control names the displayed revision and an opaque lease. The host accepts only handlers and argument kinds present in that revision, validates option/range values and rejects stale inputs. Slider input coalesces only for a compatible current slider; topology/range changes retire queued slider input. The owner stream supplies canonical trees. Control requests are never automatically retried. Errors with uncertain delivery require reopening; a callback failure retires the sandbox because it may already have changed state.

Visual sessions capture their execution host. Opening requires the visual to belong to the requested host-owned conversation, and source/assets load through the attachment path jail using an engine-issued identity. Navigation or connection changes cancel browser owners and discard late responses. Canvas work is paced, limited to one in-flight draw per view, scheduled fairly between canvases, and paused when its surface stops painting or settings obscure the transcript.

## Runtime boundary

A thread confines each Luau runtime; no interpreter or native audio/filesystem library is linked into WASM. Existing interpreter memory/time budgets remain active. All RPC services for one engine share eight runtime slots. Command and output channels are bounded; an output subscriber that falls behind cannot block a worker indefinitely. Frames are capped at 4 MiB. Closing the subscription retires its lease and releases the sandbox. Engine/account runtime shutdown closes all leases and refuses new sessions. Programs, media and interactive state are not mirrored into browser persistence.

The host advertises `visual-sessions-v1`. `OpenVisual` is an owning stream; `ControlVisual` and `DrawVisual` are unary operations. The three methods route to the selected execution computer through the existing relay transport. Draw functions can mutate program state, so drawing is deliberately excluded from uncertain-delivery retries too.

## Evidence and remaining work

Three engine scenarios cover transcript membership/host selection, isolated state, stale and invalid controls, actual canvas output, callback failure, owner-drop cleanup, runtime capacity and shared shutdown. Two viewport scenarios cover host/revision ownership, slider coalescing without duplicate callbacks, navigation retirement, stacked negative bars, scatter geometry and line gaps. Native Linux compilation, the production WASM bundle and the host server build pass.

A disposable real server runs a scripted ACP provider that publishes through the actual `PublishVisual` RPC during its agent turn. The compiled browser opens the actual host Luau session and performs five exact control writes: button, switch, tabs, slider and canvas click. Canonical trees progress from revision 0 to 5; the count reaches 11, Forecast is on, Monthly is selected, Scale is 4 and the resulting canvas text is rendered. Closing stops drawing. No visual responses are mocked; the provider itself is a fixture, not a live third-party provider. Evidence is `.context/visual-session-{engine-tests,viewport-tests,native-check,web-build,server-build,browser-final-check}.log`, `.context/visual-session-browser-acceptance.json` and `.context/visual-session-final.png`.

The disposable browser scenario is retained as `node scripts/acceptance/visuals.mjs`; it requires the built host binary, `dist/web`, Node and `agent-browser`. Its ACP and browser observers live under `scripts/acceptance/fixtures` and never modify the running preview or its profile.

Browser 3D scenes/shaders, complete chart axes/hover and table/layout parity, keyboard/accessibility parity for visual controls, browser reduced-motion integration, physical input/GPU and actual paired-device rendering remain open. Desktop scene/shader rendering remains available. Browser embedded web previews are a separate unfinished capability. This milestone does not establish full native/browser visual parity.
