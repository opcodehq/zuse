# One machine image per user

Status: design, approved direction (owner, 2026-11-06; repositories added
2026-11-14). Replaces per-project snapshot caches (off by default) and the
older per-account repository image.

## Model

```
managed base (operator, one per provider/region)
  └── user image (one per account: personal or organization)
        built from the account's machine recipe, plus a depth-1 copy of
        each of the account's repositories (see "Repositories" below)
        └── task computer: fresh VM from the user image, checkout started
            from the baked copy and fetching only newer commits (a clone
            when the repository isn't baked), repository setup script, agent
```

- **Machine recipe** (account-level, edited in Settings → Cloud → Machine):
  - setup script run once at build time as `zuse` with sudo (apt packages,
    `bun`/`pnpm`/`uv` installs, global `npm i -g`, warming a pnpm store by
    installing a seed lockfile, …);
  - skills copied into every agent's skills directory;
  - user-level instructions (`AGENTS.md`/`CLAUDE.md`);
  - dotfiles without secrets (`.gitconfig`, `.npmrc` without tokens).
  Secrets never enter an image; they stay in the secret store and are
  delivered per task.
- **No recipe** means no user image: the account starts from the managed base.
- **No object storage layer.** Everything the recipe installs lives inside the
  image, so a task computer has it from the first second.

## Build and selection

- Saving a recipe records a new recipe version and queues one build for the
  account. A build is: builder VM from the current managed base → run the
  recipe (writing only outside pinned runtime paths) → sanitize → snapshot →
  verifier VM boots the snapshot and re-checks → publish.
- Image identity: account, provider, managed base release, recipe digest and
  the repository set (which repositories, their default branches). Commits
  and credentials are not inputs: new commits are fetched by each computer,
  and the image is refreshed when its baked commits are a week old.
- Admission picks the newest `ready` user image whose base release is still
  verified; otherwise the managed base. The old user image keeps serving
  until a new one verifies. A failed build never replaces a ready image.
- The image keeps the base's runtime, supervisor, unit, helper and manifest
  files byte-identical, so the fresh-boot pins are the base's pins and the
  existing launch verification applies unchanged. Only the artifact id and
  version differ (recorded per image).

## Updates

| Change | Effect |
|---|---|
| New managed base (OS, toolchains, agent CLIs, runtime) | Each account with a recipe gets one background rebuild. Until it verifies, new tasks keep using the previous user image while its base release stays verified; when that base is retired, tasks fall back to the new base. |
| Recipe edit | One rebuild for that account. |
| Runtime fix | Ships as a new managed base (the runtime is pinned by digest at boot). The server keeps a capability window so computers on recent runtimes keep working. |

Retired user images are deleted once no computer references them.

## Reuse

The project-cache pipeline (`crates/cloud/src/project_caches/`) already builds
base-derived images with identical pins, verifies them with a second boot and
falls back to the base. User images reuse it as a repository-less cache kind.
The older repository-baking account image (`crates/cloud/src/images.rs`, its
worker and routes) cannot satisfy fresh-boot pins and is retired.

## Per-chat delivery (no image)

Instructions, skills and home files do not wait for an image. A runtime that
advertises `rust-machine-delivery-v1` receives the account's latest recipe
version as `machineDelivery` in its bootstrap (the setup script is not
included) and writes it before any agent starts
(`crates/cloud-runtime/src/machine.rs`):

| Recipe part | Written to |
|---|---|
| Instructions | `$CLAUDE_CONFIG_DIR/CLAUDE.md` (default `~/.claude/CLAUDE.md`), `$CODEX_HOME/AGENTS.md` (default `~/.codex/AGENTS.md`) |
| Skill `name` | `~/.claude/skills/<name>/SKILL.md`, `~/.agents/skills/<name>/SKILL.md` |
| File `path` | `~/<path>` (credential and runtime paths refused) |

Files are written atomically, owner-only, never through a link and never
inside the checkout, so the repository's own `AGENTS.md` stays authoritative
for the repository. The version is fixed per enrollment: a save reaches the
next chat; a running computer picks it up at its next enrollment (a new
runtime generation), not mid-session. Older runtimes don't advertise the
capability and receive nothing new.

## Delivery phases

1. Recipe: protocol type, migration, save/read API, Settings card.
2. Build: repository-less cache kind, guest recipe runner, worker phases,
   rebuild on recipe save and on managed base registration.
3. Selection: admission and allocation pick the user image with base fallback.
4. Retire the old account-image builder; status UI shows build progress,
   last good image and errors.

## Implementation (2026-11-13)

Code: `crates/cloud/src/project_caches/user_images/`, guest helper
`infra/cloud/user-image-guest.py`, migration `0037_user_images.sql`, after `0036_harness_bundles.sql` (replaces
the unused `cloud_machine_images` placeholder from 0031).

- **Image input** is the setup script and its time limit
  (`image_digest`), plus, since 2026-11-14, the repository set (below). Instructions, skills and dotfiles are delivered per chat;
  editing them never rebuilds.
- **Build phases** (one durable row per build, provider IO outside SQL
  locks, effects recorded before each call, lost replies reconciled by the
  build-unique names `ui-b-…`, `ui-v-…`, `ui-s-…`): `allocate` (builder from
  the newest verified base that has fresh-boot pins; pins re-hashed) →
  `setup` (script uploaded to `/opt/zuse/user-image`, run once as `zuse` via
  `sudo -u zuse` under `timeout`, detached with `systemd-run`; status polled)
  → `sanitize` (kill leftover `zuse` processes, require the runtime unit was
  never started and `~/.zuse-data` is empty, remove credentials, shell
  history, npm logs, `/tmp`, token lines in `.npmrc`, the build journal,
  empty task directories, re-verify pins) → `capture` → `retire-builder` →
  `verify` (verifier from the snapshot: read-only cleanliness check, then the
  same fresh boot a task computer gets, with the base's pins) →
  `retire-verifier` → `publish` (under the managed-base admission lock; a
  retired base never gains an image). Any failure → `cleanup` (deletes
  builder, verifier and unpublished snapshot) → `failed` with an error code
  and a redacted 4 KiB log tail. A ready image is never replaced by a failure.
- **The resident supervisor never runs on the builder**, so a user image is
  captured in the same stopped state as its base and needs no new guest
  runtime command.
- **Selection**: admission uses the newest ready image whose base is still
  verified (an image on an older base serves while its rebuild runs, if that
  base offers the chat); otherwise the managed base. Restores (unarchive and
  "Update computer") use the newest ready image on the computer's release.
  The computer records `request_config.rustMachineImage`.
- **Triggers**: recipe save (15 s debounce, newest recipe replaces a waiting
  build, one build in flight per account), "Rebuild now"
  (`POST /v1/cloud/machine/rebuild`), an hourly staleness pass over accounts
  active in the last 14 days (rebuilds when the ready image's script digest
  or base release differs from the current ones; identical inputs never
  rebuild), and rebuild on next use for inactive accounts. Failed builds are
  retried by later passes with exponential backoff (1 h, 2 h, …), at most 3
  times for the same script and base; a script edit or "Rebuild now" resets.
- **Retirement**: a ready image retires (provider snapshot deleted) once a
  newer ready image on a verified base exists, its base is no longer
  verified, or the script was removed, and no non-deleted computer
  (archived included) references it.
- **Providers**: Boxd, Boat and Prized (below). Others start from the managed base.
  An account gets an image only on providers it uses (`used_providers_tx`):
  Boxd (the default) always, others once the account has a computer there
  that is not deleted or was active in the last 14 days. Recipe saves,
  "Rebuild now" and the hourly pass target only those providers; the first
  computer on a new provider boots its base and queues that provider's image
  on use. A queued build for a provider the account no longer uses is dropped
  (`user_image_provider_unused`), and a ready image on such a provider
  retires once no computer references it.
  On Boat an image is a *saved snapshot* named from the image ID; its
  `snapshotId` is the version (saving a name again gets a new one). Boat saves
  a running sandbox's disk in the background and may include later writes, so
  nothing touches the builder after the save request and the builder is
  deleted only once the save is `ready`. Builder and verifier count as up only
  once Boat reports their disk hydrated, matching the post-hydration first
  start of Boat bases (runtime unit disabled in the image).
- **Fresh boots never wait for an image.** With no ready image, the computer
  boots the managed base without the setup script's tools. Running the setup
  script on that computer as a fallback is a follow-up.

Operations: the worker builds for each provider it has credentials for
(`BOXD_API_KEY`, `BOAT_API_KEY`, `PRIZED_API_KEY`; `ZUSE_CLOUD_USER_IMAGES=0` turns builds
off). A queued build for a provider the worker has no builder for is dropped
(`superseded`, `user_image_provider_unavailable`), not kept waiting. Each build briefly holds two
VMs and one snapshot; each account keeps one or two ready snapshots plus any
still referenced by computers. Builds require billing capacity like a
computer. Not yet exercised against a live Boxd base: the detached setup run
(`systemd-run` availability, upload ownership), memory-snapshot capture of a
builder whose resident never started, and the verifier's fresh boot.

### Prized (2026-11-14)

Prized sandboxes have no disk snapshot or custom image. The documented way to
make a template is to pause a set-up sandbox and fork it. A fork copies
memory, disk and running processes. The managed base already works this way,
and a user image does too: the paused builder **is** the image
(`crates/cloud/src/project_caches/user_images/prized.rs`, guest helper
`infra/cloud/prized-base/user-image-guest.py`, run as `prized`, no sudo).

- `allocate` forks the paused base (`ui-b-…`, idempotency key = name). It
  first checks that the base is still the sandbox the plan froze (its
  `createdAt` is the base version). `baseline` then checks the runtime
  against the manifest's admitted runtime SHA and records `baseTreeSha256`,
  a digest of `~/.zuse` without its state folders and Python caches.
- `setup` writes the script and helper to `~/.zuse-user-image` through exec,
  then runs the script once, detached, under `timeout`, with
  `PATH=~/.local/bin:…` (npm's global prefix is `~/.local`).
- `sanitize` kills every leftover `prized` process, the idle supervisor
  included. It then removes credentials, history, task data, runtime state
  and the journal, re-checks the runtime and the tree, and restarts the idle
  supervisor, so the image pauses exactly like the base.
- `capture` pauses the builder; its sandbox ID becomes the artifact.
  `retire-builder` does nothing.
- `verify` forks the image (`ui-v-…`) and runs the read-only check on the
  copy: runtime, tree, no leftovers, and the inherited supervisor answering
  unbound. `retire-verifier` deletes the copy.
- Computers fork the image through the normal allocation path; Prized
  allocation already forks any paused `sbx_…` artifact. Retirement deletes
  the paused image.
- Plans for Prized carry no `initialRuntimeBootstrap`: the base has no root
  fresh-boot pins. Root-base plans must still carry one.
- The control plane never sends a command to a paused image. On Prized,
  every command wakes the sandbox it is sent to.

Limits of a paused-sandbox image: it lives on one Prized machine, so it is
lost if that machine fails and becomes unforkable (`409 parent_not_forkable`)
after a move that discards memory, exactly like the managed base. Forks land
on the parent's machine (`409 fork_capacity` when full). A paused image
still counts against the workspace's running CPU and memory quota.

## Retiring the repository-baking account image (2026-11-13)

New builds through `crates/cloud/src/images.rs` (`POST /v1/cloud/image/build`)
now return `cloud_image_builds_retired`. The `image-worker` keeps running
where configured, only to finish builds already in flight and to delete
legacy snapshots; its UI was already unreachable. A deployment without the
credential authority (the only path that still launches from those builds)
can opt back in with `ZUSE_CLOUD_LEGACY_IMAGE_BUILDS=1`.

Per-project caches stay off unless `ZUSE_CLOUD_PROJECT_CACHES=1`.

Follow-up, once no deployment runs without the credential authority and
`api_cloud_project_builds` has no ready rows: delete `images.rs`, its worker,
grant issuer, routes, the `CloudImage*` RPC methods and the `image-worker`
service, and drop their tables in a migration.

## Repositories in the image (2026-11-14)

Owner decision (2026-11-14): a cold start spent ~12 of ~15 s cloning. The
account's image now also holds its repositories, so a new computer only
fetches what changed and checks out the task branch. Still one image per
account, never one per repository, and never a credential in an image.

Code: `crates/cloud/src/project_caches/user_images/repositories.rs` (planning,
the provider-neutral step), `repository_source.rs` (server-side snapshots),
`crates/cloud/src/runtime/user_image_sources.rs` (fetch tokens), guest
commands `repos-*` in `infra/cloud/user-image-guest.py`, migration
`0041_user_image_repositories.sql`.

- **Which repositories.** The account's projects with `included=true`
  (`api_cloud_projects`), oldest first, frozen into the build plan. At most
  `MAX_REPOSITORIES` (8); one snapshot at most `MAX_REPOSITORY_BYTES` (1 GiB),
  all together at most `MAX_TOTAL_BYTES` (2 GiB). Organization images leave
  private repositories out (`organization_private_repository`): every member's
  computer starts from the image, and a member without access to a repository
  must not receive its source. Everything left out is listed with its reason
  in `cloud_machine_images.repositories.skipped` and in the machine status
  (`image.repositories`).
- **Depth.** A depth-1 snapshot of the default branch. The runtime's checkout
  fetch is `--depth=1 --filter=blob:none` and offers the baked tip as a
  `have`, so older history in the image never makes the fetch smaller — it
  only grows the image (measured on opcodehq/zuse: 126 MB at depth 1, 130 MB
  full; repositories with long histories differ far more). The fetch then
  transfers the new tip's trees and only the changed blobs.
- **Build step.** Phase `repositories`, after `setup` and before `sanitize`,
  run by the worker for every backend that exposes its builder
  (`UserImageBackend::builder_guest`; Boxd does, others bake nothing and
  record `provider_unsupported`). Per repository: the worker fetches a
  depth-1 bare repository into its own scratch (`$TMPDIR`), public ones
  anonymously, private ones with a token the API mints for that single
  repository from the owner's GitHub connection
  (`POST /v1/cloud/machine/images/{image}/repositories/{project}/source-credential`,
  a short-lived worker proof bound to the image and project, only while the
  image is in its repository phase). The token is an HTTP header in the
  environment of that one Git process. The snapshot is a tar of `HEAD`,
  `shallow`, `packed-refs`, `refs`, `objects` — no config, remote or hook.
  It is uploaded in 2 MiB chunks to `/var/tmp/zuse-repo-stage/<key>/` (a
  directory owned by the provider's exec user: Boxd uploads as `boxd`, which
  is why uploads into root- or `zuse`-owned directories failed), checked
  against its SHA-256, unpacked to `/opt/zuse/repos/<sha256(identity)>.git`
  with a config the guest writes itself, verified (branch at the expected
  commit, `fsck --connectivity-only`) and handed to `zuse` (Git refuses a
  repository owned by someone else). Progress lives on the builder, so a lost
  reply or a worker restart resumes from the chunks already there. A failing
  repository is retried 3 times, then left out; it never fails the build.
  Sanitize removes the staging area; `check` (sanitize and verifier) refuses
  any baked repository whose config mentions a URL, remote, credential,
  header, include or helper, or that has hooks, logs or links. The image row
  records `repositories` (baked: repository, branch, commit, bytes, path;
  skipped: reason) and `repositories_digest`.
- **Use at computer start.** When the selected image baked the task's
  repository, creation sets `rustRepositoryCheckout.cachePath` to its path and
  `sourceCommit` to the baked commit, keeping `fetchRemote` and
  `repositoryUrl` (`repositoryCache: "machine-image"`). The runtime clones the
  local copy, fetches the requested ref through its normal authenticated
  path and checks out the task branch from the fetched commit — no runtime
  change. Repositories the image doesn't hold are cloned as before.
- **Freshness.** The hourly staleness pass (and the rebuild-on-use check)
  compares the ready image's repository set digest with the account's
  current one: adding, removing or re-branching a repository rebuilds
  (`refresh`). New commits don't, until the image is `MAX_AGE_MS` (7 days)
  old, which rebuilds it so the delta stays small.
- **Opt-out.** `ZUSE_CLOUD_USER_IMAGE_REPOSITORIES=0` on the worker builds
  images without repositories.
- Still only accounts with a setup script have an image; baking repositories
  for accounts without one is a separate decision (each would get a build).
