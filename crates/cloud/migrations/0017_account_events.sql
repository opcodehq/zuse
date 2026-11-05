-- Transactional compact event outbox. This never calls pg_notify/LISTEN/NOTIFY.
-- User cursor allocation happens after domain commits in the dispatcher; sequence
-- gaps caused by out-of-order transactions cannot skip a future committed event.
CREATE TABLE zuse_rust_account_event_outbox (
    outbox_id bigserial PRIMARY KEY,
    owner_id text NOT NULL,
    kind text NOT NULL CHECK(kind IN ('workspace','conversation','connection','authentication','github','environment')),
    resource_id text NOT NULL,
    workspace_id text,
    payload jsonb NOT NULL DEFAULT '{}',
    created_at bigint NOT NULL,
    CHECK(octet_length(payload::text)<=4096)
);
CREATE TABLE zuse_rust_account_event_cursors (
    owner_id text PRIMARY KEY,
    next_sequence bigint NOT NULL DEFAULT 1 CHECK(next_sequence>0)
);
CREATE TABLE zuse_rust_account_events (
    owner_id text NOT NULL,
    sequence bigint NOT NULL CHECK(sequence>0),
    kind text NOT NULL,
    resource_id text NOT NULL,
    workspace_id text,
    payload jsonb NOT NULL,
    created_at bigint NOT NULL,
    PRIMARY KEY(owner_id,sequence),
    CHECK(octet_length(payload::text)<=4096)
);
CREATE INDEX zuse_rust_account_events_retention ON zuse_rust_account_events(created_at);
-- Ciphertext publication can change every streaming tick. Record only structural
-- message/status changes here; token deltas stay on the opened task connection.
CREATE TABLE zuse_rust_account_event_summaries (
    workspace_id text NOT NULL REFERENCES api_cloud_workspaces(workspace_id) ON DELETE CASCADE,
    session_id text NOT NULL,
    signature text NOT NULL,
    PRIMARY KEY(workspace_id,session_id)
);
CREATE FUNCTION zuse_rust_workspace_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
DECLARE item record; BEGIN
    IF TG_OP='UPDATE' AND (NEW.state,NEW.desired_state,NEW.runtime_state,NEW.status_code,NEW.revision)
        IS NOT DISTINCT FROM (OLD.state,OLD.desired_state,OLD.runtime_state,OLD.status_code,OLD.revision) THEN RETURN NEW; END IF;
    IF TG_OP='DELETE' THEN item:=OLD; ELSE item:=NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at)
    VALUES(item.account_id,'workspace',item.workspace_id,item.workspace_id,
        jsonb_build_object('revision',item.revision,'deleted',TG_OP='DELETE'),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $body$;
CREATE TRIGGER zuse_rust_workspace_events AFTER INSERT OR UPDATE OR DELETE ON api_cloud_workspaces
FOR EACH ROW EXECUTE FUNCTION zuse_rust_workspace_event_outbox();
CREATE FUNCTION zuse_rust_connection_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.state,NEW.version,NEW.account_label,NEW.last_error_code)
        IS NOT DISTINCT FROM (OLD.state,OLD.version,OLD.account_label,OLD.last_error_code) THEN RETURN NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.owner_id,'connection',NEW.connection_id,jsonb_build_object('state',NEW.state,'version',NEW.version),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_connection_events AFTER INSERT OR UPDATE ON cloud_agent_connections
FOR EACH ROW EXECUTE FUNCTION zuse_rust_connection_event_outbox();
CREATE FUNCTION zuse_rust_authentication_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.state,NEW.operation_ciphertext,NEW.last_error_code)
        IS NOT DISTINCT FROM (OLD.state,OLD.operation_ciphertext,OLD.last_error_code) THEN RETURN NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.owner_id,'authentication',NEW.operation_id,jsonb_build_object('state',NEW.state,'actorId',NEW.actor_id),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_authentication_events AFTER INSERT OR UPDATE ON cloud_auth_operations
FOR EACH ROW EXECUTE FUNCTION zuse_rust_authentication_event_outbox();
-- Cross-device project/environment changes carry only identity and revision.
CREATE FUNCTION zuse_rust_environment_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.owner_id,'environment',NEW.project_id,jsonb_build_object('revision',NEW.revision),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_environment_events AFTER INSERT ON cloud_environment_versions
FOR EACH ROW EXECUTE FUNCTION zuse_rust_environment_event_outbox();
CREATE FUNCTION zuse_rust_project_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
DECLARE item record; BEGIN
    IF TG_OP='UPDATE' AND (NEW.state,NEW.included,NEW.configuration_digest,NEW.last_error_code)
        IS NOT DISTINCT FROM (OLD.state,OLD.included,OLD.configuration_digest,OLD.last_error_code) THEN RETURN NEW; END IF;
    IF TG_OP='DELETE' THEN item:=OLD; ELSE item:=NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(item.account_id,'environment',item.project_id,jsonb_build_object('state',item.state,'deleted',TG_OP='DELETE'),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $body$;
CREATE TRIGGER zuse_rust_project_events AFTER INSERT OR UPDATE OR DELETE ON api_cloud_projects
FOR EACH ROW EXECUTE FUNCTION zuse_rust_project_event_outbox();
CREATE FUNCTION zuse_rust_github_operation_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    IF TG_OP='UPDATE' AND NEW.phase IS NOT DISTINCT FROM OLD.phase THEN RETURN NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.owner_id,'github',NEW.operation_id,jsonb_build_object('phase',NEW.phase,'actorId',NEW.actor_id),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_github_operation_events AFTER INSERT OR UPDATE ON zuse_rust_github_operations
FOR EACH ROW EXECUTE FUNCTION zuse_rust_github_operation_event_outbox();
CREATE FUNCTION zuse_rust_github_user_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.state,NEW.epoch,NEW.version)
        IS NOT DISTINCT FROM (OLD.state,OLD.epoch,OLD.version) THEN RETURN NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.actor_id,'github',NEW.actor_id,jsonb_build_object('state',NEW.state),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_github_user_events AFTER INSERT OR UPDATE ON zuse_rust_github_users
FOR EACH ROW EXECUTE FUNCTION zuse_rust_github_user_event_outbox();
CREATE FUNCTION zuse_rust_github_installation_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.state,NEW.revision,NEW.allowed_repositories)
        IS NOT DISTINCT FROM (OLD.state,OLD.revision,OLD.allowed_repositories) THEN RETURN NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.owner_id,'github',NEW.installation_id::text,jsonb_build_object('state',NEW.state,'revision',NEW.revision),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_github_installation_events AFTER INSERT OR UPDATE ON zuse_rust_github_installations
FOR EACH ROW EXECUTE FUNCTION zuse_rust_github_installation_event_outbox();
