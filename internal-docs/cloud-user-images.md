# One machine image per user

Status: design, approved direction (owner, 2026-11-06). Replaces per-project
snapshot caches (off by default) and the older per-account repository image.

## Model

```
managed base (operator, one per provider/region)
  └── user image (one per account: personal or organization)
        built from the account's machine recipe, NO repositories
        └── task computer: fresh VM from the user image, shallow clone of
            the task's repository, repository setup script, agent
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
- Image identity: account, provider, managed base release, recipe digest.
  It contains no repository, commit or credential, so it is rebuilt only when
  the recipe or the base changes.
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

- **Image input** is the setup script and its time limit only
  (`image_digest`). Instructions, skills and dotfiles are delivered per chat;
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
- **Providers**: Boxd only. Other providers start from the managed base.
- **Fresh boots never wait for an image.** With no ready image, the computer
  boots the managed base without the setup script's tools. Running the setup
  script on that computer as a fallback is a follow-up.

Operations: the worker builds wherever it has `BOXD_API_KEY`
(`ZUSE_CLOUD_USER_IMAGES=0` turns builds off). Each build briefly holds two
VMs and one snapshot; each account keeps one or two ready snapshots plus any
still referenced by computers. Builds require billing capacity like a
computer. Not yet exercised against a live Boxd base: the detached setup run
(`systemd-run` availability, upload ownership), memory-snapshot capture of a
builder whose resident never started, and the verifier's fresh boot.

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
