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
