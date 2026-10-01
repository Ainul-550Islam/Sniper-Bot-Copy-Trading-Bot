-- 0025_tenant_runtime_registry_config.sql — STEP 3 of the enterprise
-- tenant-isolation program (PROMPT 2/10 PHASE 4).
--
-- The durable substrate for the tenant runtime registry and the tenant
-- configuration engine:
--
--   * `tenant_runtimes` — one row per tenant runtime INSTANCE. This is
--     NOT a second scheduler and NOT a second claims table: per-execution
--     ownership stays with `execution_claims` (0009) and process-level HA
--     stays with the `ha` layer (0016). This table answers exactly one
--     question: "which runtime instance is currently allowed to execute
--     for this organization, at which fencing generation?" The runtime
--     registry service (`crates/server/src/runtime_registry`) uses it for
--     registration, heartbeat, rotation and stale-worker fencing.
--   * `tenant_configs` — one row per organization: the tenant's typed
--     configuration document with an optimistic-concurrency version.
--     Precedence (global immutable safety bounds > tenant config >
--     runtime effective config > operation constraints) is resolved in
--     Rust (`crates/server/src/tenant_config`); the table stores only the
--     tenant's own layer.
--
-- Design rules honored:
--   * forward-only, additive, idempotent (IF NOT EXISTS everywhere);
--   * migration 0024 is NOT touched (it is validated and green);
--   * no ON CONFLICT arbiter of any existing table is altered;
--   * organization_id references organizations(id) ON DELETE RESTRICT —
--     a tenant with runtimes/config cannot be deleted out from under
--     them (fail-closed ownership, the same rule 0023 established for
--     the trading tables);
--   * names follow the existing snake_case conventions.

-- ══════════════════════════════════════════════════════════════════════
-- 1. Tenant runtimes
-- ══════════════════════════════════════════════════════════════════════

CREATE TABLE IF NOT EXISTS tenant_runtimes (
    -- Runtime instance identity (uuid v4 minted at registration).
    id uuid PRIMARY KEY,
    -- The tenant this runtime executes for.
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    -- Fencing generation: every rotation increments it. A worker holding
    -- an older generation is stale and must be denied (fence).
    generation bigint NOT NULL CHECK (generation >= 1),
    -- Lifecycle of the runtime instance:
    --   provisioning → active → draining → stopped
    --   (retired = stopped rows kept for audit; nothing ever returns
    --    from stopped/retired — rotation mints a fresh row)
    status text NOT NULL CHECK (status IN ('provisioning','active','draining','stopped','retired')),
    -- The process identity that registered (host/pid or operator name).
    worker_id text NOT NULL,
    -- When this runtime registered.
    started_at timestamptz NOT NULL DEFAULT now(),
    -- Last heartbeat (the registry's liveness signal).
    heartbeat_at timestamptz NOT NULL DEFAULT now(),
    -- Optional hard lease: when set, the runtime is only live while it
    -- has not expired (defense in depth on top of heartbeats).
    lease_expires_at timestamptz,
    -- When the runtime left the active set, if it did.
    stopped_at timestamptz,
    -- Non-secret operator metadata (labels, versions). Never credentials.
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb
);

-- Exactly one live (provisioning/active) runtime per organization: two
-- active runtimes for one tenant is a split-brain this schema refuses.
CREATE UNIQUE INDEX IF NOT EXISTS tenant_runtimes_one_live_per_org_idx
    ON tenant_runtimes (organization_id)
    WHERE status IN ('provisioning', 'active');

-- Registry queries: "what is the current generation for this tenant?"
CREATE INDEX IF NOT EXISTS tenant_runtimes_org_generation_idx
    ON tenant_runtimes (organization_id, generation DESC);

-- Reaping: find live runtimes whose heartbeat went stale.
CREATE INDEX IF NOT EXISTS tenant_runtimes_stale_heartbeat_idx
    ON tenant_runtimes (heartbeat_at)
    WHERE status IN ('provisioning', 'active');

-- ══════════════════════════════════════════════════════════════════════
-- 2. Tenant configurations (optimistic-concurrency document per tenant)
-- ══════════════════════════════════════════════════════════════════════

CREATE TABLE IF NOT EXISTS tenant_configs (
    -- One configuration document per organization.
    organization_id uuid PRIMARY KEY REFERENCES organizations(id) ON DELETE RESTRICT,
    -- Optimistic-concurrency version (the store CASes on it).
    version bigint NOT NULL CHECK (version >= 1),
    -- The tenant's typed configuration document (JSON form of
    -- tenant_config::model::TenantConfigModel). Validated in Rust before
    -- it can ever be stored; never contains secrets.
    config jsonb NOT NULL,
    -- Who last changed it (user id, operator label or "system").
    updated_by text,
    -- When it was last changed.
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- ══════════════════════════════════════════════════════════════════════
-- Post-apply verification (informational; run manually to audit):
--   * exactly two new tables, both tenant-owned:
--       SELECT table_name FROM information_schema.tables
--       WHERE  table_schema = 'public'
--       AND    table_name IN ('tenant_runtimes','tenant_configs');
--   * the split-brain guard exists:
--       SELECT indexname FROM pg_indexes
--       WHERE  indexname = 'tenant_runtimes_one_live_per_org_idx';
--   * every row of both tables is tenant-attributed (NOT NULL FKs).
--   * re-running this migration is a no-op (IF NOT EXISTS everywhere).
-- ══════════════════════════════════════════════════════════════════════

-- ----------------------------------------------------------------------------
-- tenant_config_audit — append-only audit trail for tenant configuration
-- writes (STEP 3 tenant_config/audit.rs). Every accepted write lands here
-- with who/when, from/to versions and the machine-readable diff. There is
-- deliberately no UPDATE or DELETE path; retention is table-level.
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tenant_config_audit (
    id              bigserial PRIMARY KEY,
    organization_id uuid NOT NULL,
    from_version    bigint,
    to_version      bigint NOT NULL CHECK (to_version >= 1),
    changes         jsonb NOT NULL DEFAULT '[]'::jsonb,
    updated_by      text,
    at              timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS tenant_config_audit_org_recent_idx
    ON tenant_config_audit (organization_id, at DESC, to_version DESC);

-- ----------------------------------------------------------------------------
-- tenant_bindings — the wallet/signer binding registry (STEP 3
-- server/src/tenant/registry.rs). One row per (tenant, kind, label):
-- wallets are keyed by label, signers by "provider/key_ref". The
-- reference column is PUBLIC identity only (an address or key
-- reference); secrets never enter this table.
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tenant_bindings (
    id              bigserial PRIMARY KEY,
    organization_id uuid NOT NULL,
    kind            text NOT NULL CHECK (kind IN ('wallet', 'signer')),
    label           text NOT NULL,
    reference       text NOT NULL,
    active          boolean NOT NULL DEFAULT true,
    created_at      timestamptz NOT NULL DEFAULT now(),
    -- One binding per tenant+kind+label (upsert target).
    CONSTRAINT tenant_bindings_one_per_label
        UNIQUE (organization_id, kind, label)
);

CREATE INDEX IF NOT EXISTS tenant_bindings_org_kind_idx
    ON tenant_bindings (organization_id, kind, label);

-- ----------------------------------------------------------------------------
-- tenant_decision_log — the append-only gateway decision log (STEP 3
-- server/src/tenant_observability/decision_log.rs). Every allow AND
-- deny, with the machine label; no update or delete path exists.
-- ----------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS tenant_decision_log (
    id              bigserial PRIMARY KEY,
    organization_id uuid NOT NULL,
    decision        text NOT NULL,
    detail          text NOT NULL DEFAULT '',
    module          text NOT NULL DEFAULT '',
    mode            text NOT NULL DEFAULT '',
    origin          text NOT NULL DEFAULT '',
    principal       text NOT NULL DEFAULT '',
    at              timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS tenant_decision_log_org_recent_idx
    ON tenant_decision_log (organization_id, at DESC, id DESC);
