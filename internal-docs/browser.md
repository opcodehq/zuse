# Embedded browser previews

The browser frontend's right sidebar has a Browser pane with an address field, reload and open-in-new-tab controls. It uses the existing `WatchPreviews` engine subscription to discover project services. Opening a project preview uses the viewing engine's existing preview proxy; it does not create another proxy or poll the engine for page layout.

`crates/viewport/src/browser.rs` owns the GPUI controls and lifetime. `apps/web/src/browser_frame.js` owns one DOM iframe positioned over the pane's measured content area. Commands and callbacks carry a generation: navigation, thread switches and connection changes retire the old surface. Settings and other application overlays hide it while preserving the page's form state. Address Enter is separate from composer submission. Closing or switching away from the pane cancels discovery; switching threads closes the page.

The iframe requires the browser's credentialless capability. This creates an ephemeral browsing context without application cookies/storage and permits embedding beneath the application's cross-origin isolation policy. It is sandboxed without parent navigation, and its referrer and camera/microphone/location access are disabled. Addresses accept HTTP(S), reject embedded credentials and reject the application origin. Unsupported browsers retain an external-tab action. See [credentialless iframe behavior](https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/IFrame_credentialless).

Project proxy URLs resolve on the viewing engine's computer. Automatic preview buttons are therefore available only to a direct loopback browser client. Paired and LAN clients can enter a published project URL; a preview tunnel through the paired encrypted connection remains unfinished. Native desktop webviews continue to use their existing platform implementation.

Sites may refuse embedding through their own CSP or frame policy. Cross-origin pages own their navigation and storage; the address field tracks requested URLs, not a fabricated readable browser history. Open-in-new-tab is available for those pages.

## Verification

- `node --test apps/web/tests/browser_frame.mjs` checks adapter lifetime, isolation options, unsupported capability and URL admission.
- `cargo test -p zuse-viewport --lib browser::tests` checks composer separation, stale callbacks and viewing-engine discovery scope.
- `node scripts/acceptance/browser.mjs` runs the compiled GPUI/WASM client against a disposable actual engine and HTTP/module/fetch/WebSocket project fixture. It checks page input, credential separation, hiding and thread cleanup. The shared `scripts/acceptance/harness.mjs` owns temporary profiles and browser/engine cleanup for this and visual acceptance. Neither scenario calls a paid model API.

Acceptance artifacts are written beneath `.context/`; startup logs with private launch links stay in the private temporary profile and are never emitted by the harness.
