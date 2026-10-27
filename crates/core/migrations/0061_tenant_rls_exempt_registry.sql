-- Exceptions are intentionally explicit and auditable. An empty seed is
-- correct until a tenant-keyed table is proven to contain global identity data.
CREATE TABLE IF NOT EXISTS public.rls_exempt_tables (
    table_name text PRIMARY KEY,
    reason text NOT NULL CHECK (length(btrim(reason)) > 0)
);
GRANT SELECT, INSERT, UPDATE, DELETE ON public.rls_exempt_tables TO bot_app, bot_system;
