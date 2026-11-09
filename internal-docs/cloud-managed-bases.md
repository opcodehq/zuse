# Managed base admission

Managed bases are operator releases. They contain no repositories, task storage or
account credentials. An environment cache and a task computer remain separate
artifacts. Credentials are delivered after enrollment and never require a base rebuild.

The guest image preparation command accepts `managedBase: true` with an empty
`repositories` list. Ordinary image plans continue to require repositories. Base
preparation rejects repository caches, a populated workspace, runtime identity,
agent credentials and unconsumed secret files, including when rechecking a completed
manifest. It records the same runtime/storage/recipe/fetch capabilities as project
images. This is sanitation evidence, not a substitute for provider/agent certification.

An operator release document contains `provider`, immutable `releaseId`, provider
`artifactId`, `architecture` (`x86_64` or `aarch64`), `capabilities`, the actual guest
`manifest`, `harnesses`, and `certificationReportSha256`. Each harness records its
agent, pinned version, executable SHA-256 and the compatible `catalog` returned by
discovery (`id`, `name`, `defaultModel`, `models`). Keep the certification report with
the release artifacts. Never generate a success report from key presence or API health.

After building and certifying the artifact, register it using the operator database
role and the native API executable:

```sh
zuse-cloud-api register-managed-base --release /private/releases/release.json
```

Supply `ZUSE_CLOUD_RELEASE_DATABASE_URL` privately to that process. It intentionally
does not fall back to the application's database URL. Application roles can read
the managed-base catalog but cannot insert, update or delete it. Registration is
idempotent only for an identical verified release. Publish a new release ID to change
an artifact, harness, model catalog, architecture or capabilities.

Migration `0010_managed_bases.sql` permits a workspace to reference an explicit
platform release instead of a customer build. The reference is constrained by a
foreign key; existing build references remain valid. No synthetic customer build,
snapshot billing row or cleanup record is created for the platform artifact.

The authority-enabled creation path uses a compatible verified release. Legacy
account caches are not reused until they carry a certified parent release and
preparation attestation for the selected-connection runtime. Admission requires entitlement, billing capacity,
an explicitly selected connected agent account and a certified harness/model/size.
The current controller admits Boxd; the other adapters are not enabled merely by
registering their artifacts. Public repositories use an empty checkout and fetch
the requested ref once. Private cache misses return `github_runtime_credentials_required`
until scoped runtime Git delivery is implemented. Missing compatible releases return
`managed_base_unavailable_for_agent`; retry cannot install missing server configuration.

The new path's model catalog includes only promoted harnesses with a connected
account. Legacy deployments retain their original catalog until authority activation.

Retire a release using operator tooling to stop new admission. Retired releases must
retain their provider artifacts while referenced tasks or archives exist. Existing
archives restore against their pinned release; failed or missing releases require
explicit recovery. A restore never silently switches to the newest base. Ordinary
retained resume preserves its checkout journal and does not fetch or rebase.

Deployment remains gated. The migration, release registration, authority tools and
provider/agent acceptance must pass in staging before enabling customer creation.

Owner-only staging candidate pins retain a default capacity of one computer, or two
when `allowStagingCompute` is enabled. An operator may set `maxComputers` from 2 to
10 on that same pin to permit normal staging chat testing without deleting retained
work. An explicit `unlimitedComputers: true` removes this staging count cap entirely;
it cannot be combined with `maxComputers`. Both overrides require the staging compute
allowance and preserve owner, actor, deployment cohort and expiry checks. Admission,
worker checks and retained resume honor the same policy. Production entitlements and
billing limits are unaffected.
