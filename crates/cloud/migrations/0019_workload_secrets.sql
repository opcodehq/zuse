-- Project workload secrets are independent of recipes, image caches and agent credentials.
CREATE TABLE IF NOT EXISTS zuse_rust_workload_secrets (
    secret_id text PRIMARY KEY,
    owner_id text NOT NULL,
    project_id text NOT NULL REFERENCES api_cloud_projects(project_id),
    environment_name text NOT NULL,
    revision bigint NOT NULL CHECK (revision>0),
    ciphertext text,
    revoked_at bigint,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    UNIQUE(owner_id,project_id,environment_name),
    CHECK ((revoked_at IS NULL) = (ciphertext IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS zuse_rust_workload_secrets_project ON zuse_rust_workload_secrets(owner_id,project_id,updated_at,secret_id);

-- Publish only compact secret metadata in the same transaction. Isolated vault
-- databases may intentionally omit the optional account-event foundation.
CREATE FUNCTION zuse_rust_workload_secret_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    IF to_regclass('zuse_rust_account_event_outbox') IS NULL THEN RETURN NEW; END IF;
    IF TG_OP='UPDATE' AND NEW.revision=OLD.revision THEN RETURN NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.owner_id,'environment',NEW.project_id,
        jsonb_build_object('secretId',NEW.secret_id,'revision',NEW.revision,'revoked',NEW.revoked_at IS NOT NULL),
        NEW.updated_at);
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_workload_secret_events AFTER INSERT OR UPDATE ON zuse_rust_workload_secrets
FOR EACH ROW EXECUTE FUNCTION zuse_rust_workload_secret_event_outbox();
