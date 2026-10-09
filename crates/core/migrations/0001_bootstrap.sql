-- 0001_bootstrap.sql — operators, API principals, wallets, strategies,
-- configuration versions and system events.
--
-- Conventions used by every migration in this directory:
--   * timestamptz everywhere, defaulting to now().
--   * `double precision` for amounts/prices: the whole application models
--     money as f64 (risk engine, positions, venues) and the database mirrors
--     that contract exactly — no silent rounding differences between the
--     in-memory and the durable representation.
--   * Every lookup path used by the app has an explicit index.

CREATE TABLE IF NOT EXISTS operators (
    id              uuid PRIMARY KEY,
    label           text NOT NULL,
    role            text NOT NULL CHECK (role IN ('owner', 'operator', 'readonly')),
    telegram_user_id bigint,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS operators_telegram_user_id_key
    ON operators (telegram_user_id) WHERE telegram_user_id IS NOT NULL;

-- API keys are stored ONLY as SHA-256 hashes; the plaintext key never
-- touches the database.
CREATE TABLE IF NOT EXISTS api_keys (
    id           uuid PRIMARY KEY,
    key_hash     text NOT NULL UNIQUE,
    label        text NOT NULL,
    role         text NOT NULL CHECK (role IN ('owner', 'operator', 'readonly')),
    operator_id  uuid REFERENCES operators(id) ON DELETE SET NULL,
    enabled      boolean NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    last_used_at timestamptz
);

CREATE TABLE IF NOT EXISTS wallets (
    id         uuid PRIMARY KEY,
    label      text NOT NULL,
    chain      text NOT NULL DEFAULT 'solana',
    address    text NOT NULL UNIQUE,
    kind       text NOT NULL DEFAULT 'hot' CHECK (kind IN ('hot', 'cold', 'read')),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS strategies (
    id         uuid PRIMARY KEY,
    module     text NOT NULL CHECK (module IN ('sniper', 'copy', 'polymarket')),
    name       text NOT NULL UNIQUE,
    enabled    boolean NOT NULL DEFAULT true,
    params     jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Every applied configuration, content-addressed. The running config is
-- written on startup; diffs between versions give the operational history.
CREATE TABLE IF NOT EXISTS config_versions (
    version    bigserial PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now(),
    source     text NOT NULL,
    sha256     text NOT NULL,
    snapshot   jsonb NOT NULL,
    note       text
);
CREATE INDEX IF NOT EXISTS config_versions_sha256_idx ON config_versions (sha256);

-- Structured system log for operator-visible incidents (startup, shutdown,
-- outage classification, recovery actions). Bounded by the cleanup worker.
CREATE TABLE IF NOT EXISTS system_events (
    id      bigserial PRIMARY KEY,
    ts      timestamptz NOT NULL DEFAULT now(),
    kind    text NOT NULL,
    module  text,
    severity text NOT NULL DEFAULT 'info' CHECK (severity IN ('debug', 'info', 'warn', 'error', 'fatal')),
    message text NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX IF NOT EXISTS system_events_ts_idx ON system_events (ts);
CREATE INDEX IF NOT EXISTS system_events_kind_ts_idx ON system_events (kind, ts);
