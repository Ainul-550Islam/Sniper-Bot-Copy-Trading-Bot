-- RLS foundation. Roles are group roles with no login credentials; runtime
-- connection principals must be explicitly granted membership by deployment.
DO $$ BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_app') THEN
        CREATE ROLE bot_app NOLOGIN NOBYPASSRLS;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_system') THEN
        CREATE ROLE bot_system NOLOGIN BYPASSRLS;
    END IF;
END $$;
ALTER ROLE bot_app NOBYPASSRLS;
ALTER ROLE bot_system BYPASSRLS;

CREATE SCHEMA IF NOT EXISTS app;
CREATE OR REPLACE FUNCTION app.current_org_id()
RETURNS uuid
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    SELECT NULLIF(current_setting('app.org_id', true), '')::uuid
$$;

GRANT USAGE ON SCHEMA public, app TO bot_app, bot_system;
GRANT EXECUTE ON FUNCTION app.current_org_id() TO bot_app, bot_system;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO bot_app, bot_system;
GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO bot_app, bot_system;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO bot_app, bot_system;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO bot_app, bot_system;
