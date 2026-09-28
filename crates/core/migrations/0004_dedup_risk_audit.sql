-- 0004_dedup_risk_audit.sql — restart-safe deduplication, the risk-event
-- trail and the append-only, hash-chained audit log.

-- Durable dedup windows. `INSERT ... ON CONFLICT DO NOTHING` is the atomic
-- first-arrival check: the inserting transaction wins exactly once, across
-- processes and restarts. Optional TTL lets old keys be cleaned up.
CREATE TABLE IF NOT EXISTS dedup_keys (
    namespace    text NOT NULL,
    key          text NOT NULL,
    first_seen_at timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz,
    PRIMARY KEY (namespace, key)
);
CREATE INDEX IF NOT EXISTS dedup_keys_expires_idx ON dedup_keys (expires_at)
    WHERE expires_at IS NOT NULL;

-- Every risk decision that blocked or altered trading, plus breaker trips.
CREATE TABLE IF NOT EXISTS risk_events (
    id       bigserial PRIMARY KEY,
    ts       timestamptz NOT NULL DEFAULT now(),
    module   text NOT NULL,
    kind     text NOT NULL CHECK (kind IN (
                 'rejected', 'scaled', 'breaker_tripped', 'breaker_reset',
                 'kill_switch', 'limit_changed', 'drift_flag')),
    symbol   text,
    reason   text NOT NULL,
    snapshot jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX IF NOT EXISTS risk_events_ts_idx ON risk_events (ts);
CREATE INDEX IF NOT EXISTS risk_events_module_ts_idx ON risk_events (module, ts DESC);

-- Append-only audit trail with a SHA-256 hash chain: each row hashes
-- (prev_hash || canonical row content), so any retroactive modification or
-- deletion is detectable by re-walking the chain. The application exposes no
-- update/delete path for this table (only INSERT and read endpoints).
CREATE TABLE IF NOT EXISTS audit_events (
    id        bigserial PRIMARY KEY,
    ts        timestamptz NOT NULL DEFAULT now(),
    actor     text NOT NULL,
    action    text NOT NULL,
    target    text,
    outcome   text NOT NULL CHECK (outcome IN ('success', 'failure', 'denied')),
    detail    jsonb NOT NULL DEFAULT '{}'::jsonb,
    prev_hash text NOT NULL,
    hash      text NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_events_ts_idx ON audit_events (ts);
CREATE INDEX IF NOT EXISTS audit_events_action_idx ON audit_events (action, ts DESC);
CREATE INDEX IF NOT EXISTS audit_events_actor_idx ON audit_events (actor, ts DESC);
