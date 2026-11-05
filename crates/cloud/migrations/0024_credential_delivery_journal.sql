-- Authority-only encrypted results; access grants stay bound to one enrolled recipient.
CREATE TABLE zuse_rust_credential_rotations (
 rotation_id text PRIMARY KEY,
 owner_id text NOT NULL,
 connection_id text NOT NULL,
 agent text NOT NULL,
 authority_epoch bigint NOT NULL CHECK(authority_epoch>0),
 source_version bigint NOT NULL CHECK(source_version>0),
 lease_id text NOT NULL,
 phase text NOT NULL CHECK(phase IN ('dispatching','result','applied','uncertain','superseded')),
 result_ciphertext text,
 provider_account_id text,
 account_label text,
 credential_expires_at bigint,
 started_at bigint NOT NULL,
 lease_expires_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 UNIQUE(owner_id,connection_id,source_version),
 CHECK(result_ciphertext IS NULL OR phase='result')
);
CREATE INDEX zuse_rust_credential_rotations_cleanup ON zuse_rust_credential_rotations(owner_id,updated_at) WHERE phase IN ('applied','superseded','uncertain');
CREATE INDEX zuse_rust_credential_rotations_dispatch ON zuse_rust_credential_rotations(owner_id,lease_expires_at) WHERE phase='dispatching';
CREATE INDEX zuse_rust_credential_rotations_results ON zuse_rust_credential_rotations(owner_id,updated_at) WHERE phase='result';
CREATE TABLE zuse_rust_provider_grant_receipts (
 workspace_id text NOT NULL,
 runtime_generation bigint NOT NULL CHECK(runtime_generation>0),
 recipient_thumbprint text NOT NULL,
 request_id text NOT NULL,
 request_digest text NOT NULL,
 owner_id text NOT NULL,
 credential_owner_id text NOT NULL,
 connection_id text NOT NULL,
 authority_epoch bigint NOT NULL CHECK(authority_epoch>0),
 source_version bigint NOT NULL CHECK(source_version>0),
 phase text NOT NULL CHECK(phase IN ('processing','ready','uncertain')),
 lease_id text NOT NULL,
 lease_expires_at bigint NOT NULL,
 grant_ciphertext text,
 expires_at bigint NOT NULL,
 created_at bigint NOT NULL,
 updated_at bigint NOT NULL,
 PRIMARY KEY(workspace_id,runtime_generation,recipient_thumbprint,request_id),
 CHECK((phase='ready')=(grant_ciphertext IS NOT NULL))
);
CREATE INDEX zuse_rust_provider_grant_receipts_cleanup ON zuse_rust_provider_grant_receipts(expires_at);
