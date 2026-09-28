-- 0018_saas_runtime_records.sql — durable, multi-replica TASK 7A adapter.
--
-- Migration 0017 defines the normalized public SaaS schema. The application
-- domain records are richer Rust values and must be reconstructed without
-- lossy enum/default mapping. This table is the server adapter's canonical
-- serialized runtime projection: every request reads it and every mutation
-- writes it before updating the local cache. PostgreSQL unique constraints
-- provide cross-replica identity/idempotency guarantees.
--
-- No plaintext session token, API key, invite token, or password is present:
-- the serialized records contain only the same hashes already represented by
-- 0017. Trading truth remains in the TASK 1–6 tables.

CREATE TABLE IF NOT EXISTS saas_runtime_records (
    kind        text        NOT NULL,
    id          text        NOT NULL,
    organization_id uuid,
    user_id     uuid,
    lookup_key  text,
    record      jsonb       NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, id)
);

CREATE UNIQUE INDEX IF NOT EXISTS saas_runtime_records_lookup_key
    ON saas_runtime_records (kind, lookup_key)
    WHERE lookup_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS saas_runtime_records_org
    ON saas_runtime_records (kind, organization_id, created_at);
CREATE INDEX IF NOT EXISTS saas_runtime_records_user
    ON saas_runtime_records (kind, user_id, created_at);
