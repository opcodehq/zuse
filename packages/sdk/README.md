# Zuse SDK

`@zuse/sdk` controls a running Zuse engine using the application's versioned API.
It supports local engines and authenticated cloud endpoints without starting an
engine or implementing another agent loop.

```ts
import { createZuseClient } from '@zuse/sdk';

const zuse = createZuseClient({
  endpoint: 'wss://workspace.example/api/control',
  token: process.env.ZUSE_CONTROL_TOKEN!,
});
console.log(await zuse.info());
for await (const zots of zuse.zots.watch()) {
  console.log(zots);
}
zuse.close();
```

Local engine discovery lives at `<profile>/zots/control.json`. Keep it private:
it contains an installation-owner credential. The CLI discovers the default
local profile or accepts `--discovery` / `ZUSE_CONTROL_FILE` for another profile.
Remote endpoints require `wss://`; tokens never belong in a URL.

`call(method, params, { signal, targetDeviceId })` invokes the advertised
application operations. `watch` returns a cancellable asynchronous iterator.
Use the typed `zots` helpers for coordinator commands, versioned files and events.
`info()` checks API compatibility and returns the current operation catalog.
Errors expose a stable `code`; disconnects never automatically replay mutations.
Reuse a Zot command ID and identical input after reconciling an uncertain result.

```sh
zuse-control info
zuse-control call ZotFiles '{"zotId":"release"}'
zuse-control watch WatchZots
zuse-control mcp
```

The npm CLI launches the small binary installed by Zuse under `~/.zuse/bin`.
`ZUSE_CONTROL_BINARY` selects another installation. App bundles include the binary,
SDK tarball and checksum manifest in a `control` directory alongside the executable.
App startup verifies and installs these tools into versioned managed directories.
An existing global npm SDK installation updates automatically; project-local
dependencies and lockfiles remain managed by the project's package manager.
Use `ControlUpdateStatus` and `RetryControlUpdate` to inspect or retry updates.

Use Node.js 22 or newer. Build with `npm ci && npm test`.

To exercise the SDK, CLI and MCP against a temporary engine profile, build the
binaries from the repository root and run the integration suite:

```sh
cargo build --locked -p zuse-cli -p zuse-control
npm --prefix packages/sdk run test:engine
```

This suite uses no paid models and removes its temporary profile afterward. It
checks authentication, subscriptions, revision conflicts, versioned file history,
decision retention, fresh conversations, restart and idempotent command retries.

Regenerate contracts from the repository root:
`cargo run -p zuse-protocol --example control_types`.
