-- 0054_trading_runtime_integrity.sql — durable runtime-integrity fields for
-- the limit-order and DCA workers, plus a Postgres-backed pending store for
-- tenant SSO (OIDC + PKCE) so the callback survives a restart and works on
-- more than one control-plane replica.
--
-- Additive only. Nothing here rewrites or drops data from 0038/0052/0053.
-- Every statement is IF NOT EXISTS / guarded, so re-running is a no-op.
--
-- ROLLBACK (manual, documented — the repository has no down-migrations).
-- Reverse order; run only after stopping the workers:
--   DROP TABLE IF EXISTS sso_pending_auth;
--   DROP TABLE IF EXISTS dca_runs;
--   ALTER TABLE dca_schedules
--       DROP COLUMN IF EXISTS lease_owner,
--       DROP COLUMN IF EXISTS lease_expires_at,
--       DROP COLUMN IF EXISTS attempts,
--       DROP COLUMN IF EXISTS last_error,
--       DROP COLUMN IF EXISTS last_run_at;
--   ALTER TABLE limit_orders
--       DROP COLUMN IF EXISTS lease_owner,
--       DROP COLUMN IF EXISTS lease_expires_at,
--       DROP COLUMN IF EXISTS attempts,
--       DROP COLUMN IF EXISTS last_error,
--       DROP COLUMN IF EXISTS executed_at;
--
-- Design rules:
--   * a lease is (lease_owner, lease_expires_at). A worker may claim a row
--     only when lease_expires_at IS NULL or lease_expires_at < now(); the
--     claim itself is a single UPDATE ... WHERE ... RETURNING, so two
--     replicas can never both hold the same row.
--   * `attempts` is incremented on every claim; `last_error` keeps the most
--     recent failure text (truncated by the application to 512 chars).
--   * `dca_runs(schedule_id, slot_at)` is UNIQUE: one intent per schedule per
--     interval slot, even if the worker crashes after inserting and retries.
--   * `sso_pending_auth` is consumed with DELETE ... RETURNING so a state can
--     be redeemed exactly once.

-- ---------------------------------------------------------------------------
-- Limit orders: lease + attempt accounting
-- ---------------------------------------------------------------------------
ALTER TABLE limit_orders
    ADD COLUMN IF NOT EXISTS lease_owner      text,
    ADD COLUMN IF NOT EXISTS lease_expires_at timestamptz,
    ADD COLUMN IF NOT EXISTS attempts         integer NOT NULL DEFAULT 0
        CHECK (attempts >= 0),
    ADD COLUMN IF NOT EXISTS last_error       text,
    ADD COLUMN IF NOT EXISTS executed_at      timestamptz;

COMMENT ON COLUMN limit_orders.lease_owner IS
    'Opaque worker id holding the lease; NULL when unclaimed.';
COMMENT ON COLUMN limit_orders.lease_expires_at IS
    'Lease deadline; a claim is allowed once this is in the past.';
COMMENT ON COLUMN limit_orders.executed_at IS
    'When the trigger was handed to the execution pipeline (intent, not fill).';

-- Scan path for the worker: active orders whose lease is free or expired.
CREATE INDEX IF NOT EXISTS limit_orders_claimable_idx
    ON limit_orders (lease_expires_at, trigger_kind, price_sol)
    WHERE status = 'active';

-- ---------------------------------------------------------------------------
-- DCA schedules: lease, attempt accounting, last run marker
-- ---------------------------------------------------------------------------
ALTER TABLE dca_schedules
    ADD COLUMN IF NOT EXISTS lease_owner      text,
    ADD COLUMN IF NOT EXISTS lease_expires_at timestamptz,
    ADD COLUMN IF NOT EXISTS attempts         integer NOT NULL DEFAULT 0
        CHECK (attempts >= 0),
    ADD COLUMN IF NOT EXISTS last_error       text,
    ADD COLUMN IF NOT EXISTS last_run_at      timestamptz;

COMMENT ON COLUMN dca_schedules.last_run_at IS
    'Slot time of the last intent created; the next slot is derived from it.';

CREATE INDEX IF NOT EXISTS dca_schedules_claimable_idx
    ON dca_schedules (lease_expires_at, next_run_at)
    WHERE status = 'active';

-- ---------------------------------------------------------------------------
-- DCA run ledger: exactly-once intent per (schedule, interval slot)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS dca_runs (
    id              bigserial PRIMARY KEY,
    -- Tenant scope. Every query MUST filter on this column.
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    schedule_id     text NOT NULL REFERENCES dca_schedules(id) ON DELETE CASCADE,
    -- The interval slot this run belongs to (whole-interval aligned).
    slot_at         timestamptz NOT NULL,
    -- SOL committed by this run (> 0). Summed against budget by the worker.
    amount_sol      numeric(24, 9) NOT NULL CHECK (amount_sol > 0),
    -- RunStatus: 'intent' | 'submitted' | 'failed'.
    status          text NOT NULL DEFAULT 'intent'
                    CHECK (status IN ('intent', 'submitted', 'failed')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT dca_runs_one_intent_per_slot UNIQUE (schedule_id, slot_at)
);

COMMENT ON TABLE dca_runs IS
    'One row per DCA schedule per interval slot. UNIQUE(schedule_id, slot_at) '
    'makes intent creation exactly-once across crashes and replicas.';

CREATE INDEX IF NOT EXISTS dca_runs_org_created_idx
    ON dca_runs (organization_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- SSO pending authorization state (OIDC + PKCE)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS sso_pending_auth (
    -- The opaque `state` value sent to the IdP. Random, >= 256 bits.
    state           text PRIMARY KEY CHECK (char_length(state) BETWEEN 32 AND 256),
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    config_id       uuid NOT NULL REFERENCES tenant_sso_configs(id) ON DELETE CASCADE,
    -- PKCE verifier (43–128 unreserved chars per RFC 7636).
    code_verifier   text NOT NULL CHECK (char_length(code_verifier) BETWEEN 43 AND 128),
    redirect_uri    text NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    -- Hard expiry. Consumption refuses rows past this instant.
    expires_at      timestamptz NOT NULL,
    CONSTRAINT sso_pending_auth_ttl CHECK (expires_at > created_at)
);

COMMENT ON TABLE sso_pending_auth IS
    'Pending OIDC authorizations. Consumed exactly once with DELETE ... RETURNING. '
    'Holds a PKCE verifier, so it is never exposed through any API.';

CREATE INDEX IF NOT EXISTS sso_pending_auth_expires_idx
    ON sso_pending_auth (expires_at);
