-- 0021_saas_lifecycle.sql — restart-safe tenant closure/deprovisioning and retention state.
--
-- Supports suspend -> close -> retention -> purge workflow.
-- Must be resumable after process restart. Never immediately destroy financial/audit truth.
-- Distinguish logical closure from irreversible purge.

-- Lifecycle jobs: durable state machine for tenant closure/deprovisioning
CREATE TABLE IF NOT EXISTS tenant_lifecycle_jobs (
    id                  uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    requested_action    text        NOT NULL CHECK (requested_action IN ('suspend','resume','close','purge')),
    -- Execution phase: ordered cursor for restart-safe resume
    phase               text        NOT NULL DEFAULT 'requested'
                        CHECK (phase IN ('requested','trading_disabled','credentials_revoked','sessions_invalidated','custody_revoked','resources_cleaned','retention','completed','failed')),
    -- High-level lifecycle state
    state               text        NOT NULL DEFAULT 'pending'
                        CHECK (state IN ('pending','running','waiting','completed','failed','canceled')),
    retry_count         integer     NOT NULL DEFAULT 0 CHECK (retry_count >= 0),
    max_retries         integer     NOT NULL DEFAULT 5 CHECK (max_retries >= 0),
    scheduled_at        timestamptz NOT NULL DEFAULT now(),
    started_at          timestamptz,
    completed_at        timestamptz,
    last_attempt_at     timestamptz,
    next_attempt_at     timestamptz,
    failure_reason      text        NOT NULL DEFAULT '',
    -- Retention deadline: when it is eligible to reconsider purging (not when it MUST purge)
    retention_deadline  timestamptz,
    -- Who requested it
    requested_by        uuid        REFERENCES users(id) ON DELETE SET NULL,
    requested_by_actor  text        NOT NULL DEFAULT 'system',
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, requested_action, phase, state) DEFERRABLE INITIALLY DEFERRED
);
-- Single active job per organization: at most one non-terminal lifecycle job
CREATE UNIQUE INDEX IF NOT EXISTS tenant_lifecycle_jobs_active_unique
    ON tenant_lifecycle_jobs (organization_id)
    WHERE state IN ('pending','running','waiting');
CREATE INDEX IF NOT EXISTS tenant_lifecycle_jobs_org_state_idx ON tenant_lifecycle_jobs (organization_id, state, created_at);
CREATE INDEX IF NOT EXISTS tenant_lifecycle_jobs_scheduled_idx ON tenant_lifecycle_jobs (scheduled_at, state) WHERE state IN ('pending','running','waiting');
CREATE INDEX IF NOT EXISTS tenant_lifecycle_jobs_retry_idx ON tenant_lifecycle_jobs (next_attempt_at) WHERE state IN ('pending','running','waiting') AND retry_count < max_retries;
CREATE INDEX IF NOT EXISTS tenant_lifecycle_jobs_phase_idx ON tenant_lifecycle_jobs (phase, state);

-- Retention policies: per-category retention rules, configured once, enforced deterministically
CREATE TABLE IF NOT EXISTS retention_policies (
    id                  uuid        PRIMARY KEY,
    category            text        NOT NULL UNIQUE CHECK (category IN ('operational','credentials','sessions','api_keys','control_plane_audit','financial_accounting','legal_compliance')),
    retention_days      integer     NOT NULL CHECK (retention_days >= 0),
    -- Whether this category is eligible for purge at all (financial truth is NEVER purged by default)
    purge_allowed       boolean     NOT NULL DEFAULT false,
    -- Purge only after retention_deadline + retention_days AND after lifecycle phase = retention/completed
    description         text        NOT NULL DEFAULT '',
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);
-- Seed default retention policies (idempotent)
INSERT INTO retention_policies (id, category, retention_days, purge_allowed, description)
VALUES
    ('00000000-0000-0000-0000-000000000001', 'operational', 90, true, 'Operational data: trading configs, module state, non-financial runtime records'),
    ('00000000-0000-0000-0000-000000000002', 'credentials', 0, true, 'Credentials: immediate revocation on close, purge after retention'),
    ('00000000-0000-0000-0000-000000000003', 'sessions', 0, true, 'Sessions: immediate invalidation on close'),
    ('00000000-0000-0000-0000-000000000004', 'api_keys', 0, true, 'API keys: immediate revocation on close'),
    ('00000000-0000-0000-0000-000000000005', 'control_plane_audit', 2555, false, 'Control-plane audit: 7 year retention, never automatically purged'),
    ('00000000-0000-0000-0000-000000000006', 'financial_accounting', 2555, false, 'Financial/accounting records: immutable, never purged automatically'),
    ('00000000-0000-0000-0000-000000000007', 'legal_compliance', 2555, false, 'Legal/compliance records: never purged automatically')
ON CONFLICT (category) DO NOTHING;

-- Retention tracking per organization: when each category was marked for retention and when it becomes eligible
CREATE TABLE IF NOT EXISTS tenant_retention_state (
    organization_id     uuid        NOT NULL REFERENCES organizations(id) ON DELETE RESTRICT,
    category            text        NOT NULL REFERENCES retention_policies(category) ON DELETE RESTRICT,
    retention_deadline  timestamptz NOT NULL,
    purged              boolean     NOT NULL DEFAULT false,
    purged_at           timestamptz,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, category)
);
CREATE INDEX IF NOT EXISTS tenant_retention_state_deadline_idx ON tenant_retention_state (retention_deadline, purged) WHERE purged = false;
CREATE INDEX IF NOT EXISTS tenant_retention_state_org_idx ON tenant_retention_state (organization_id, purged);
