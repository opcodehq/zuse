-- Durable, coalesced event work. A bounded worker sweep repairs lost in-process
-- dispatch. No database notification and no continuous client polling.
CREATE TABLE IF NOT EXISTS zuse_rust_github_join_jobs (
 installation_id bigint PRIMARY KEY CHECK(installation_id>0),
 requested_version bigint NOT NULL DEFAULT 1,
 completed_version bigint NOT NULL DEFAULT 0,
 lease_id text,
 lease_until bigint,
 next_attempt_at bigint NOT NULL DEFAULT 0,
 attempts integer NOT NULL DEFAULT 0,
 last_error text,
 updated_at bigint NOT NULL
);
CREATE INDEX IF NOT EXISTS zuse_rust_github_join_ready ON zuse_rust_github_join_jobs(next_attempt_at) WHERE requested_version>completed_version;
