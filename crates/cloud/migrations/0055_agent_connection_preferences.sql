ALTER TABLE cloud_agent_connections
    ADD COLUMN position integer NOT NULL DEFAULT 0 CHECK (position >= 0),
    ADD COLUMN enabled boolean NOT NULL DEFAULT true,
    ADD COLUMN alias text;

WITH ordered AS (
    SELECT connection_id, (row_number() OVER (PARTITION BY owner_id ORDER BY created_at, connection_id) - 1)::integer AS position
    FROM cloud_agent_connections WHERE state <> 'revoked'
)
UPDATE cloud_agent_connections c SET position = o.position
FROM ordered o WHERE c.connection_id = o.connection_id AND c.position <> o.position;

CREATE INDEX cloud_agent_connections_priority_idx ON cloud_agent_connections(owner_id, position, connection_id)
    WHERE state <> 'revoked';

CREATE INDEX cloud_agent_connections_provider_account_idx ON cloud_agent_connections(owner_id, agent, provider_account_id)
    WHERE state <> 'revoked' AND provider_account_id IS NOT NULL;

CREATE FUNCTION zuse_rust_append_agent_connection() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    PERFORM pg_advisory_xact_lock(hashtextextended('cloud-agent-connections:' || NEW.owner_id, 0));
    SELECT COALESCE(MAX(position) + 1, 0) INTO NEW.position
    FROM cloud_agent_connections WHERE owner_id = NEW.owner_id AND state <> 'revoked';
    RETURN NEW;
END $body$;
CREATE TRIGGER zuse_rust_append_agent_connection BEFORE INSERT ON cloud_agent_connections
FOR EACH ROW EXECUTE FUNCTION zuse_rust_append_agent_connection();

CREATE OR REPLACE FUNCTION zuse_rust_connection_event_outbox() RETURNS trigger
LANGUAGE plpgsql SET search_path FROM CURRENT AS $body$
BEGIN
    IF TG_OP='UPDATE' AND (NEW.state,NEW.version,NEW.account_label,NEW.last_error_code,NEW.position,NEW.enabled,NEW.alias)
        IS NOT DISTINCT FROM (OLD.state,OLD.version,OLD.account_label,OLD.last_error_code,OLD.position,OLD.enabled,OLD.alias) THEN RETURN NEW; END IF;
    INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at)
    VALUES(NEW.owner_id,'connection',NEW.connection_id,jsonb_build_object('state',NEW.state,'version',NEW.version),
        (extract(epoch FROM clock_timestamp())*1000)::bigint);
    RETURN NEW;
END $body$;
