-- One gateway process owns a workspace's sockets. No execution or history lives here.
CREATE TABLE IF NOT EXISTS zuse_rust_gateway_leases (
    workspace_id text PRIMARY KEY,
    owner_token text NOT NULL,
    expires_at bigint NOT NULL
);
