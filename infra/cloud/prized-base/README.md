# Prized managed base

Prized has no custom images and no root. Our managed base is a normal sandbox,
set up as the user `prized` under `/home/prized/.zuse` and then **paused**.
Each production computer is a fork of that paused parent
(`POST /sandboxes/{id}/fork`). A fork also gets the parent's idle runtime
supervisor, still running.

## Build

Run this on a host that has the bundle. The staging host works well: uploading
the 1 GB bundle takes about 10 s and the whole build about 35 s.

```sh
python3 -I infra/cloud/prized-base/build.py \
  --bundle /path/to/bundle --key-file /path/to/prized-key
```

`build.py` needs only the Python standard library. It never follows redirects,
and it sends the key only to `*.prized.dev` over HTTPS. The script:

1. Computes the build id `zuse-prized-base-<digest>`. The digest covers every
   input file: the bundle files, the repository launchers and supervisor next to
   this directory, and the files in this directory.
2. Checks for an existing build. If a paused sandbox labelled
   `zuse-role=managed-base,zuse-release=<build id>` exists, it reuses it and
   stops. Any other sandbox with those labels is deleted.
3. Creates a sandbox (template `prized-base-v1`, 2 vCPU / 4 GiB / 20 GB). It
   uploads the inputs to `~/.zuse-input` in parts of at most 95 MiB, joins the
   parts and checks every sha256.
4. Runs `install.sh` in the background and waits for it to finish. Then it
   removes the uploaded inputs and runs the checks: runtime version, an idle
   supervisor ping, `gh` 2.x through the login-shell PATH, `claude --version`,
   and a codex `app-server` initialize call.
5. Pauses the sandbox. If any step fails, the sandbox is deleted
   (`--keep-on-failure` keeps it for debugging). `--cleanup` deletes every
   sandbox labelled with this build.

## Layout (`R=/home/prized/.zuse`)

| Path | Contents |
| --- | --- |
| `R/bin/zuse-cloud-runtime` | runtime binary (`zuse-grok-auth` symlink next to it) |
| `R/bin/gh` | managed wrapper → `R/libexec/gh` (checksum-verified gh 2.x release) |
| `R/lib/zuse-launch` | worker entry point; ensures a supervisor, then `launch-rust-runtime` |
| `R/lib/zuse-supervise` | starts a detached supervisor if none is serving |
| `R/lib/launch-rust-runtime` | `infra/cloud/launch-runtime.sh` |
| `R/lib/launch-rust-recovery` | wrapper supplying relocated paths → `R/lib/recovery-core/launch-rust-recovery` (`infra/cloud/launch-recovery.sh`) |
| `R/lib/runtime-{supervisor,repair,recreation}.py`, `R/lib/harness-release.py` | from `infra/cloud` |
| `R/harnesses` | harness release root, channel `staging` (read-only) |
| `R/status`, `R/recovery`, `R/boot`, `R/state` | runtime status, recovery state, staged and installed boot credentials |
| `R/preparation/{plan.json,status/manifest.json}` | image plan and the manifest from `prepare-image` |
| `~/.zuse-data`, `~/workspace` | runtime data (supervisor IPC only) and an empty workspace |

`~/.profile` and `~/.bashrc` put `R/bin` at the front of PATH. In `.bashrc` the
PATH line comes first, ahead of the early return for non-interactive shells.

## Limits of the home-directory base

- **Boot credential path.** The worker writes the boot credential to
  `R/state/rust-boot-<op>`, but the supervisor only accepts
  `<status dir>/rust-boot-<op>`. `zuse-launch` moves the file into `R/status`
  before handing over.
- **GitHub CLI.** The runtime's brokered GitHub CLI (`github-from-profile` and
  the manifest's `githubCli` receipt) is hard-coded to a root-owned
  `/usr/bin/gh`, `/opt/zuse/bin/gh` and uid 10001. That means `R/bin/gh` runs
  the real `gh` directly, without brokered auth, and the image manifest has no
  `githubCli` entry.
- **Runtime updates.** There is no root helper (`/usr/local/libexec/zuse/runtime-update.py`
  is absent), so the runtime swaps `R/bin/zuse-cloud-runtime` itself by
  `rename()` (the task user owns `R/bin`) and execs it in place. The
  supervisor and `zuse-launch` always start `$ZUSE_RUNTIME_BINARY` by path, so
  a later cold launch picks up the swapped file. No launch step checks the
  runtime SHA on this layout.
- **Agent warm-up.** The supervisor's `warm_agent` only looks under
  `/opt/zuse/harnesses`, so it does nothing on this layout.
