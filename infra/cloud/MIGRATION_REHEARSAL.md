# Cloud data migration rehearsal

The current tool proves the mechanical import and handoff safeguards on synthetic
records in an isolated PostgreSQL cluster. It does not authorize customer cutover,
convert the TypeScript guest database into Rust engine storage, or certify that
real retained history can be rendered.

```sh
# Offline inventory; no credentials or database connection.
python3 infra/cloud/migration-rehearsal.py --environment staging plan

# Creates, exercises, stops and removes its own temporary localhost cluster.
# Requires initdb, pg_ctl and psql. No deployment URL is accepted as a write target.
python3 infra/cloud/migration-rehearsal.py --environment staging rehearse

# Optional read-only aggregate inventory. The file must be regular, mode0600,
# and contain the original database URL privately. The URL is never printed or
# passed in psql command arguments. Remote connections require verified TLS.
python3 infra/cloud/migration-rehearsal.py --environment staging audit \
  --source-url-file /private/legacy-readonly-url --source-schema public

python3 infra/cloud/tests/migration-rehearsal.py
```

The source schema is the canonical TypeScript compatibility baseline pinned to
`a0155d2dd11f07495a99d3737c9bd40a5c8b4508`; the target applies the reviewed Rust
migrations selected by `database.py`. The report records the exact schema and
migration hashes used. It never prints row contents or decrypted credentials.

The fixture preserves twenty-four table families covering WorkOS/account references,
Stripe customer and subscription IDs, entitlements, billing periods and ledger
source IDs, project/repository identities, task/chat/session/command IDs, opaque
message/tool ciphertext, transcript object/checksum references, GitHub/model authority identities, opaque credential envelopes
and enrollment revisions, retained machines/snapshots, wrapped-key metadata and
workspace settings. Imported ciphertext is compared byte-for-byte through stable
row digests. Two synthetic ciphertext objects are copied and checksum verified.
A consumed sequence high-water mark is retained, including gaps absent from rows.
The fixture also rejects rollback after a target consumes a sequence value without
creating a row, and preserves a financial integer above JavaScript's exact integer
range. Nonempty target tables stop the import before any records are copied.

Two restricted PostgreSQL login roles model legacy and Rust writers. A separate
privileged fence rejects the inactive writer, locks drain in-flight writes before
handoff, and importer access exists only while both writers are stopped. The
rehearsal proves a clean rollback, then makes a target write and verifies that a
rollback is rejected while the legacy writer remains fenced. It imports neither
native runtime ownership nor pending provider/payment jobs. No provider SDK or
machine operation is invoked.

For an actual migration, the following remain required:

- A consistent, approved legacy export or private read-only connection, together
  with a complete table/object inventory and reconciliation counts.
- The original encryption and wrapped-key inventory, retained account/device
  identities and an authorized key transfer or re-encryption process. Keeping
  opaque ciphertext intact does not prove the new reader can decrypt it.
- Original object-store read access and immutable checksums for messages, tools,
  attachments and recovery archives. Fixture files are not an R2 data rehearsal.
- A real service writer shutdown and database-role/lease fence, including payment
  and provider workers, with a named handoff owner. The fixture role fence is not
  automatically installed in the legacy deployment.
- A verified retained engine/session conversion or an explicitly supported legacy
  reader. Do not start a Rust guest from an incompatible TypeScript image.
- A staging import rehearsal on the real exported dataset, stable identity and
  financial reconciliation, permission/key tests, measured history rendering and
  a reviewed rollback point before enabling customer traffic.

The existing worker bridge and production services remain authoritative until
those checks pass. Production mutation is outside this tool's interface.

## Actual encrypted-object retention rehearsal

`legacy-object-rehearsal.py` reuses an approved private export; it never opens a
source database connection. `inventory` verifies the export manifest and dump
SHA, then reads selected COPY tables through private `pg_restore` pipes.
`inventory-bucket` performs bounded metadata-only R2 listing. Neither command
decrypts records or prints object keys. Private inventories and copy mappings are
owner-only files beside the export.

`copy` takes explicit private source/destination R2 environment files and an
explicit source bucket from the pinned Worker configuration. It reads at most
five existing payloads totaling 25 MiB, checks retained byte/hash references,
preserves HTTP/custom metadata, and compares the copied bytes through a fresh
GET. Each destination intent is durable before PUT, including a lost PUT reply.
Cleanup deletes only this invocation's unique staging prefix and verifies HEAD
404. Existing journals cannot be replayed; `cleanup` can reconcile their exact
destination keys without another source read or copy. These credentials must
never be passed as command arguments or printed.

The approved `a13a32e1c474448f` dump rehearsal found 53 visible transcript
checkpoint references, all present with matching lengths in the source bucket,
and no recovery archive records. A complete, point-in-time metadata listing found
11,598 objects: 9,403 transcript versions and 2,195 transcript pages, totaling
447,249,138 stored bytes. No attachment or archive object paths appeared.
This listing is not an atomic snapshot or a decrypted semantic reference audit:
280 message rows remain sealed, and tool/attachment references inside ciphertext
cannot yet be authenticated or converted.

Five actual transcript ciphertexts totaling 132,172 bytes matched the dump's
lengths and SHA-256 values and passed staging byte/metadata readback. A separate
process verified all five destination HEAD responses were 404, their prefix was
empty, and the source ETags/lengths were unchanged. No database/container or
provider machine was started. All source object operations were read-only.

**Storage isolation release gate:** the credential configured for
`zuse-rust-staging` could read the distinct production bucket
`zuse-cloud-transcripts`. Its access must be reviewed and restricted before a
production migration release; this rehearsal did not rotate keys or test source
write permissions. The original sealing key/version remains unverified. Sampled
ciphertext retention does not certify decryption, full payload migration, session
conversion, or writer cutover.
