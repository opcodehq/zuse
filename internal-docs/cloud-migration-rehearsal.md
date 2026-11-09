# Private retained-data migration rehearsal

The migration tools preserve the original Zuse schema pinned at
`a0155d2dd11f07495a99d3737c9bd40a5c8b4508`. They distinguish a database retention
rehearsal from an executable guest/account cutover. No production writer handoff
or legacy guest execution is enabled by a successful structural check.

`migration-rehearsal.py` classifies all66 original application tables:42 retained
in compatible application tables and24 retained as opaque, inactive quarantine
records. The latter include pending launches, payment delivery intents, renewable
credential leases and authorization caches. Unsupported work is retained rather
than replayed or silently dropped. The synthetic test includes financial values
above JavaScript's exact-integer range, GitHub installation/repository links,
provider event/execution/delivery identities, history/tool receipts, archives,
wrapped-key metadata and retained machine references. Native ownership and jobs
are never imported. It tests writer fencing and rejects unsafe rollback after
new writes or sequence consumption.

`legacy-export.py plan` reads catalog/count/size metadata only. Its export action
requires review of the exact source fingerprint and a new private output
location. Credentials remain in environment variables, not command arguments.
The dump uses a consistent read-only PostgreSQL snapshot, verified TLS, private
permissions, no owner/ACL replay and immutable checksums. PostgreSQL18 source data
requires a compatible dump client. `database.py` resolves `sslrootcert=system` to
the platform trust bundle for older clients while preserving `verify-full`.

`retained-rehearsal.py` accepts only that approved private dump and a pinned
PostgreSQL image. It cannot accept a deployment database URL. It creates a bounded
networkless temporary container, with no published ports, a private Unix socket,
and no production/staging schema mounts. Readiness requires the final postgres
PID1, avoiding the image's temporary initialization server. The clone restores
and imports with constraints enabled, compares complete canonical row hashes
without returning customer records, preserves sequence high-water marks and
`is_called`, and confirms zero native owners/jobs and no active target writer.
The source remains unchanged. A separate inspection verifies container/data
removal; the approved private dump remains available for review.

A table-only legacy dump includes four procedural trigger entries without their
function bodies. An exact TOC filter quarantines those four entries and preserves
their original TOC/DDL privately. It rejects unknown or partial trigger inventories
and refuses to quarantine a constraint trigger. Every PK/FK/CHECK/index/sequence
entry remains selected. Legacy catalogue workflow and snapshot-price immutability
trigger behavior is **not certified as migrated** by this retention rehearsal.
The target uses its own migration manifest, now including0026 and0027 after0025;
obsolete0022 is excluded.

The actual private retained snapshot passed structural retention checks across
all66 tables. The standalone dump crypto probe avoids starting another database:
it reads only the selected encrypted envelopes through private pipes and reports
AES-GCM open counts. Its AAD matches the pinned original account-bound GitHub
sealing format. Current configured keys did not authenticate the retained sample.
The original legacy Worker's sealing key/version remains a prerequisite; do not
request another database URL or treat preserved ciphertext as verified decryption.
Source keys, plaintext, customer records and the private snapshot are never stored
in this repository. Full evidence is kept in ignored operational metadata and on
the private host.

An actual R2 rehearsal reused the same approved dump without another export or
database restore. All53 visible transcript references were present with matching
lengths. Metadata-only source listing completed with11,598 objects/447,249,138
bytes (9,403 transcript versions;2,195 pages), with no attachment/archive paths.
Five existing ciphertext payloads totaling132,172 bytes matched retained SHA/size
and staging readback bytes/metadata. Independent checks confirmed all five staged
copies absent, the unique prefix empty, and sampled source ETags/lengths unchanged.
The private key mapping and full inventory remain on the protected host.

The staging R2 service credential could read the distinct production bucket.
This is a storage-isolation release gate, not a source-write test; no source write
or credential rotation was performed. Sealed attachment/tool references remain
opaque, and a point-in-time bucket listing is not a transactionally consistent
object snapshot. Remaining gates include original-key compatibility, full payload
and semantic-reference verification, legacy guest/session conversion, procedural
behavior parity, and a reviewed one-writer production handoff. Sampled ciphertext
retention does not certify those gates.
