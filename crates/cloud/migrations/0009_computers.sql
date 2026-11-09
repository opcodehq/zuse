-- Additive application tables. Existing execution/data ownership is not transferred.
CREATE TABLE cloud_environment_versions (
    owner_id text NOT NULL,
    project_id text NOT NULL,
    revision bigint NOT NULL CHECK (revision > 0),
    source text NOT NULL CHECK (source IN ('project','repository')),
    recipe jsonb NOT NULL CHECK (jsonb_typeof(recipe)='object'),
    digest text NOT NULL,
    created_at bigint NOT NULL,
    PRIMARY KEY(owner_id,project_id,revision)
);

CREATE TABLE cloud_managed_bases (
    provider text NOT NULL CHECK(provider IN ('boat','boxd','e2b','daytona')),
    release_id text NOT NULL,
    artifact_id text NOT NULL,
    architecture text NOT NULL,
    capabilities jsonb NOT NULL,
    state text NOT NULL CHECK(state IN ('preparing','verified','retired','failed')),
    created_at bigint NOT NULL,
    PRIMARY KEY(provider,release_id)
);

CREATE TABLE cloud_agent_connections (
    connection_id text PRIMARY KEY,
    owner_id text NOT NULL,
    idempotency_key text NOT NULL,
    request_digest text NOT NULL,
    agent text NOT NULL CHECK(agent IN ('claude','codex','grok')),
    method text NOT NULL CHECK(method IN ('api-key','setup-token','device-code')),
    state text NOT NULL CHECK(state IN ('pending','connected','reconnect-required','revoked','error')),
    account_label text,
    provider_account_id text,
    credential_ciphertext text,
    expires_at bigint,
    version bigint NOT NULL DEFAULT 1 CHECK(version>0),
    authority_epoch bigint NOT NULL DEFAULT 1 CHECK(authority_epoch>0),
    refresh_lease_id text,
    refresh_lease_expires_at bigint,
    last_error_code text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE(owner_id,connection_id),
    UNIQUE(owner_id,idempotency_key)
);
CREATE INDEX cloud_agent_connections_owner_idx ON cloud_agent_connections(owner_id,agent,created_at);

CREATE TABLE cloud_auth_operations (
    operation_id text PRIMARY KEY,
    owner_id text NOT NULL,
    actor_id text NOT NULL,
    connection_id text NOT NULL REFERENCES cloud_agent_connections(connection_id),
    idempotency_key text NOT NULL,
    state text NOT NULL CHECK(state IN ('queued','authorizing','connected','expired','cancelled','error')),
    operation_ciphertext text,
    expires_at bigint NOT NULL,
    lease_id text,
    lease_expires_at bigint,
    last_error_code text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE(owner_id,idempotency_key),
    FOREIGN KEY(owner_id,connection_id) REFERENCES cloud_agent_connections(owner_id,connection_id)
);
CREATE UNIQUE INDEX cloud_auth_one_pending_login_idx ON cloud_auth_operations(connection_id) WHERE state IN ('queued','authorizing');

CREATE TABLE cloud_tasks (
    task_id text PRIMARY KEY,
    owner_id text NOT NULL,
    project_id text NOT NULL,
    environment_revision bigint,
    connection_id text NOT NULL REFERENCES cloud_agent_connections(connection_id),
    agent text NOT NULL CHECK(agent IN ('claude','codex','grok')),
    model text NOT NULL,
    title text NOT NULL,
    base_ref text NOT NULL,
    branch text NOT NULL,
    idempotency_key text NOT NULL,
    request_digest text NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE(owner_id,idempotency_key),
    UNIQUE(owner_id,task_id),
    FOREIGN KEY(owner_id,connection_id) REFERENCES cloud_agent_connections(owner_id,connection_id)
);
CREATE INDEX cloud_tasks_owner_updated_idx ON cloud_tasks(owner_id,updated_at DESC,task_id);

CREATE TABLE cloud_computers (
    computer_id text PRIMARY KEY,
    task_id text NOT NULL UNIQUE REFERENCES cloud_tasks(task_id),
    owner_id text NOT NULL,
    provider text NOT NULL CHECK(provider IN ('boat','boxd','e2b','daytona')),
    size_id text NOT NULL,
    provider_resource_id text,
    generation bigint NOT NULL DEFAULT 1 CHECK(generation>0),
    state text NOT NULL CHECK(state IN ('queued','preparing','running','sleeping','resuming','recreating','deleting','deleted','failed')),
    stage text NOT NULL,
    managed_release_id text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE(owner_id,computer_id),
    FOREIGN KEY(owner_id,task_id) REFERENCES cloud_tasks(owner_id,task_id)
);
CREATE UNIQUE INDEX cloud_computers_provider_resource_idx ON cloud_computers(provider,provider_resource_id) WHERE provider_resource_id IS NOT NULL;

CREATE TABLE cloud_operations (
    operation_id text PRIMARY KEY,
    owner_id text NOT NULL,
    computer_id text NOT NULL REFERENCES cloud_computers(computer_id),
    generation bigint NOT NULL,
    action text NOT NULL CHECK(action IN ('create','sleep','resume','restart-runtime','recreate','delete','prepare')),
    state text NOT NULL CHECK(state IN ('queued','running','completed','failed','uncertain','cancelled')),
    idempotency_key text NOT NULL,
    provider_operation_id text,
    lease_id text,
    lease_expires_at bigint,
    due_at bigint NOT NULL,
    attempts integer NOT NULL DEFAULT 0,
    last_error_code text,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE(owner_id,idempotency_key),
    FOREIGN KEY(owner_id,computer_id) REFERENCES cloud_computers(owner_id,computer_id)
);
CREATE INDEX cloud_operations_due_idx ON cloud_operations(due_at,lease_expires_at) WHERE state IN ('queued','running','uncertain');

CREATE TABLE cloud_messages (
    owner_id text NOT NULL,
    task_id text NOT NULL REFERENCES cloud_tasks(task_id),
    sequence bigint NOT NULL CHECK(sequence>0),
    message_id text NOT NULL,
    role text NOT NULL CHECK(role IN ('user','assistant','tool','system')),
    content jsonb NOT NULL,
    created_at bigint NOT NULL,
    PRIMARY KEY(task_id,sequence),
    UNIQUE(task_id,message_id),
    FOREIGN KEY(owner_id,task_id) REFERENCES cloud_tasks(owner_id,task_id)
);
CREATE INDEX cloud_messages_owner_task_idx ON cloud_messages(owner_id,task_id,sequence DESC);

CREATE TABLE cloud_tool_results (
    owner_id text NOT NULL,
    task_id text NOT NULL REFERENCES cloud_tasks(task_id),
    result_id text NOT NULL,
    message_id text NOT NULL,
    summary jsonb NOT NULL,
    inline_output text,
    object_ref text,
    output_bytes bigint NOT NULL CHECK(output_bytes>=0),
    digest text NOT NULL,
    created_at bigint NOT NULL,
    PRIMARY KEY(task_id,result_id),
    FOREIGN KEY(owner_id,task_id) REFERENCES cloud_tasks(owner_id,task_id),
    CHECK(inline_output IS NULL OR octet_length(inline_output)<=65536),
    CHECK(NOT(inline_output IS NOT NULL AND object_ref IS NOT NULL))
);

CREATE TABLE cloud_events (
    event_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner_id text NOT NULL,
    task_id text,
    kind text NOT NULL,
    source_id text NOT NULL,
    payload jsonb NOT NULL,
    created_at bigint NOT NULL,
    UNIQUE(owner_id,source_id)
);
CREATE INDEX cloud_events_owner_cursor_idx ON cloud_events(owner_id,event_id);

CREATE TABLE cloud_outbox (
    command_id text PRIMARY KEY,
    owner_id text NOT NULL,
    task_id text NOT NULL REFERENCES cloud_tasks(task_id),
    generation bigint NOT NULL,
    idempotency_key text NOT NULL,
    payload jsonb NOT NULL,
    state text NOT NULL CHECK(state IN ('admitted','running','completed','cancelled','uncertain','failed')),
    receipt jsonb,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE(task_id,idempotency_key),
    FOREIGN KEY(owner_id,task_id) REFERENCES cloud_tasks(owner_id,task_id)
);
