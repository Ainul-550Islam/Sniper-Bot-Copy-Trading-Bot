-- 0017_saas_control_plane.sql — TASK 7A (SaaS control plane foundation):
-- users, organizations (tenants), memberships, durable sessions,
-- tenant-scoped API keys, invites, plans, subscriptions, entitlements,
-- usage metering, provisioning jobs and the organization audit trail.
--
-- WHAT THIS MIGRATION IS NOT
-- --------------------------
-- It does NOT move any trading truth. Orders / executions (0002),
-- positions / trades (0003), reconciliation (0005), intents (0007),
-- execution claims (0009/0011), execution lifecycle (0012), copy journal
-- (0013), Polymarket journal (0014), the global ledger (0015) and the HA
-- worker / lease / cursor state (0016) remain the sources of truth for
-- WHAT happened. This migration only establishes WHO owns a resource, WHO
-- may touch it, and WHICH plan / entitlement applies.
--
-- The existing single-operator tables stay exactly as they are:
-- `operators` and `api_keys` (0001) keep working for deployment-level
-- credentials. The SaaS credential is the NEW `saas_api_keys` table, which
-- is tenant-scoped; nothing in 0001 is altered or dropped.
--
--   users
--       One human identity. `password_hash` is PBKDF2-HMAC-SHA256
--       (algorithm + iterations + salt encoded in the string); a plaintext
--       password is never stored, logged or returned.
--
--   organizations
--       One TENANT. Every SaaS-owned row below carries `organization_id`,
--       and every authorization decision resolves to it. A suspended or
--       deleted organization can be refused centrally.
--
--   organization_members
--       The (user, organization) membership with its role. A user may
--       belong to several organizations with different roles; the pair is
--       unique.
--
--   sessions
--       Durable browser/API sessions. Only `token_hash` is stored, so a
--       database leak cannot be replayed as a login. Revocation and expiry
--       are explicit columns, not derived from a cache.
--
--   saas_api_keys
--       Tenant-scoped programmatic credentials. `secret_hash` only; the
--       plaintext is shown exactly once at creation. `key_prefix` is the
--       public, non-secret identifier used in listings and logs. Because
--       the row is durable, a key created at runtime keeps working after a
--       restart (the fix for the previous in-memory-only behaviour).
--
--   invites
--       Pending organization invitations, again hash-only.
--
--   plans / subscriptions / entitlements
--       Provider-neutral billing foundation. `plans` is the catalogue,
--       `subscriptions` is what an organization currently has (with a
--       provider column so Stripe/Paddle adapters can arrive later without
--       a schema change), `entitlements` is the ONE place a feature check
--       reads: "may this tenant use feature X, and up to which limit".
--
--   usage_events
--       Append-only metering. `UNIQUE (organization_id, idempotency_key)`
--       is the guarantee that the same reported event cannot be billed
--       twice — the same rule the TASK 5 ledger uses for money.
--
--   provisioning_jobs
--       Restart-safe onboarding state machine (signup → user → org →
--       membership → plan → defaults → ready) with attempt counters, so a
--       crash mid-signup resumes instead of creating a half-built tenant.
--
--   organization_audit_events
--       Per-tenant audit trail for control-plane actions. The hash-chained
--       platform trail (`audit_events`, 0004) is unchanged and remains the
--       tamper-evident system log.
--
-- Additive and restart-safe: IF NOT EXISTS only, no DROP, no destructive
-- ALTER, no DELETE, nothing in 0001–0016 touched.

CREATE TABLE IF NOT EXISTS users (
    id              uuid        PRIMARY KEY,
    -- Lowercased, trimmed; the unique index enforces one account per address.
    email           text        NOT NULL,
    email_verified  boolean     NOT NULL DEFAULT false,
    display_name    text        NOT NULL DEFAULT '',
    -- PBKDF2-HMAC-SHA256 encoded string: `pbkdf2-sha256$<iters>$<salt_b64>$<hash_b64>`.
    -- NEVER a plaintext password.
    password_hash   text        NOT NULL,
    status          text        NOT NULL DEFAULT 'active'
                                  CHECK (status IN ('active', 'suspended', 'deactivated')),
    -- Platform-level (cross-tenant) administrator. Normal customers are false.
    platform_admin  boolean     NOT NULL DEFAULT false,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    last_login_at   timestamptz
);
CREATE UNIQUE INDEX IF NOT EXISTS users_email_key ON users (lower(email));
CREATE INDEX IF NOT EXISTS users_status_idx ON users (status);

CREATE TABLE IF NOT EXISTS organizations (
    id          uuid        PRIMARY KEY,
    -- URL-safe unique handle (`acme-capital`).
    slug        text        NOT NULL,
    name        text        NOT NULL,
    status      text        NOT NULL DEFAULT 'active'
                              CHECK (status IN ('active', 'trialing', 'past_due',
                                                'suspended', 'closed')),
    -- The user who created it; membership still governs access.
    created_by  uuid        REFERENCES users(id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    suspended_at timestamptz,
    suspend_reason text     NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX IF NOT EXISTS organizations_slug_key ON organizations (lower(slug));
CREATE INDEX IF NOT EXISTS organizations_status_idx ON organizations (status);

CREATE TABLE IF NOT EXISTS organization_members (
    id              uuid        PRIMARY KEY,
    organization_id uuid        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id         uuid        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role            text        NOT NULL CHECK (role IN (
                        'platform_admin', 'org_owner', 'org_admin', 'trader',
                        'security_admin', 'billing_admin', 'auditor', 'viewer')),
    status          text        NOT NULL DEFAULT 'active'
                                  CHECK (status IN ('active', 'suspended', 'removed')),
    invited_by      uuid        REFERENCES users(id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, user_id)
);
CREATE INDEX IF NOT EXISTS organization_members_user_idx ON organization_members (user_id);
CREATE INDEX IF NOT EXISTS organization_members_org_idx
    ON organization_members (organization_id, status);

CREATE TABLE IF NOT EXISTS sessions (
    id              uuid        PRIMARY KEY,
    user_id         uuid        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- The organization this session is acting for. NULL = not yet scoped
    -- (right after login, before the tenant is chosen).
    organization_id uuid        REFERENCES organizations(id) ON DELETE CASCADE,
    -- SHA-256 of the opaque session token. NEVER the token itself.
    token_hash      text        NOT NULL UNIQUE,
    -- Public, non-secret prefix for log correlation (`ses_ab12cd34`).
    token_prefix    text        NOT NULL DEFAULT '',
    user_agent      text        NOT NULL DEFAULT '',
    ip              text        NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_seen_at    timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    revoked_at      timestamptz,
    revoke_reason   text        NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS sessions_user_idx ON sessions (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS sessions_live_idx ON sessions (expires_at)
    WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS saas_api_keys (
    id              uuid        PRIMARY KEY,
    organization_id uuid        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Public identifier shown in listings/logs (`sk_live_ab12cd34`); it is
    -- NOT sufficient to authenticate.
    key_prefix      text        NOT NULL,
    -- SHA-256 of the full secret. NEVER the secret itself.
    secret_hash     text        NOT NULL UNIQUE,
    label           text        NOT NULL DEFAULT '',
    role            text        NOT NULL CHECK (role IN (
                        'platform_admin', 'org_owner', 'org_admin', 'trader',
                        'security_admin', 'billing_admin', 'auditor', 'viewer')),
    -- Optional narrowing of the role's permissions; empty = the role's set.
    scopes          text[]      NOT NULL DEFAULT '{}',
    created_by      uuid        REFERENCES users(id) ON DELETE SET NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    last_used_at    timestamptz,
    expires_at      timestamptz,
    revoked_at      timestamptz,
    revoke_reason   text        NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS saas_api_keys_org_idx ON saas_api_keys (organization_id);
CREATE INDEX IF NOT EXISTS saas_api_keys_live_idx ON saas_api_keys (organization_id)
    WHERE revoked_at IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS saas_api_keys_prefix_key ON saas_api_keys (key_prefix);

CREATE TABLE IF NOT EXISTS invites (
    id              uuid        PRIMARY KEY,
    organization_id uuid        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    email           text        NOT NULL,
    role            text        NOT NULL CHECK (role IN (
                        'platform_admin', 'org_owner', 'org_admin', 'trader',
                        'security_admin', 'billing_admin', 'auditor', 'viewer')),
    -- SHA-256 of the invitation token. NEVER the token itself.
    token_hash      text        NOT NULL UNIQUE,
    invited_by      uuid        REFERENCES users(id) ON DELETE SET NULL,
    status          text        NOT NULL DEFAULT 'pending'
                                  CHECK (status IN ('pending', 'accepted', 'revoked', 'expired')),
    created_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    accepted_at     timestamptz,
    accepted_by     uuid        REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS invites_org_idx ON invites (organization_id, status);
CREATE INDEX IF NOT EXISTS invites_email_idx ON invites (lower(email), status);

CREATE TABLE IF NOT EXISTS plans (
    id            uuid        PRIMARY KEY,
    -- Machine-readable, stable across price changes.
    code          text        NOT NULL,
    name          text        NOT NULL,
    status        text        NOT NULL DEFAULT 'active'
                                CHECK (status IN ('active', 'deprecated', 'private')),
    -- Feature limits as `{"feature_key": <number|bool|null>}`; `null` = unlimited.
    -- Commercial pricing lives with the billing provider, not in trading code.
    limits        jsonb       NOT NULL DEFAULT '{}'::jsonb,
    description   text        NOT NULL DEFAULT '',
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS plans_code_key ON plans (lower(code));

CREATE TABLE IF NOT EXISTS subscriptions (
    id                   uuid        PRIMARY KEY,
    organization_id      uuid        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    plan_id              uuid        NOT NULL REFERENCES plans(id) ON DELETE RESTRICT,
    -- Provider-neutral: `manual` today, `stripe` / `paddle` later without a
    -- schema change.
    provider             text        NOT NULL DEFAULT 'manual'
                                       CHECK (provider IN ('manual', 'stripe', 'paddle')),
    -- The provider's own subscription id, when there is one.
    provider_ref         text,
    status               text        NOT NULL DEFAULT 'active' CHECK (status IN (
                             'trialing', 'active', 'past_due', 'paused',
                             'canceled', 'expired')),
    current_period_start timestamptz NOT NULL DEFAULT now(),
    current_period_end   timestamptz,
    cancel_at_period_end boolean     NOT NULL DEFAULT false,
    canceled_at          timestamptz,
    created_at           timestamptz NOT NULL DEFAULT now(),
    updated_at           timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS subscriptions_org_idx ON subscriptions (organization_id, status);
CREATE UNIQUE INDEX IF NOT EXISTS subscriptions_provider_ref_key
    ON subscriptions (provider, provider_ref) WHERE provider_ref IS NOT NULL;

CREATE TABLE IF NOT EXISTS entitlements (
    id              uuid        PRIMARY KEY,
    organization_id uuid        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- e.g. `module.sniper`, `module.polymarket`, `limit.max_bots`,
    -- `limit.max_members`, `feature.api_keys`.
    feature         text        NOT NULL,
    -- NULL = pure boolean grant; a number = the limit.
    limit_value     double precision,
    -- Where the grant came from: the plan, a manual override, or a trial.
    source          text        NOT NULL DEFAULT 'plan'
                                  CHECK (source IN ('plan', 'override', 'trial')),
    enabled         boolean     NOT NULL DEFAULT true,
    starts_at       timestamptz NOT NULL DEFAULT now(),
    ends_at         timestamptz,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, feature, source)
);
CREATE INDEX IF NOT EXISTS entitlements_org_idx ON entitlements (organization_id, feature);

CREATE TABLE IF NOT EXISTS usage_events (
    id              uuid        PRIMARY KEY,
    organization_id uuid        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Closed metric vocabulary (see `bot_core::billing::usage`).
    metric          text        NOT NULL,
    quantity        double precision NOT NULL DEFAULT 0 CHECK (quantity >= 0),
    -- Who reported it: `sniper` | `copy` | `polymarket` | `api` | `system`.
    source          text        NOT NULL DEFAULT 'system',
    -- The ONE metering idempotency identity: the same reported event cannot
    -- be counted twice, in this process, a later one, or another replica.
    idempotency_key text        NOT NULL,
    detail          text        NOT NULL DEFAULT '',
    occurred_at     timestamptz NOT NULL DEFAULT now(),
    recorded_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, idempotency_key)
);
CREATE INDEX IF NOT EXISTS usage_events_org_metric_idx
    ON usage_events (organization_id, metric, occurred_at DESC);

CREATE TABLE IF NOT EXISTS provisioning_jobs (
    id              uuid        PRIMARY KEY,
    -- Set as soon as the step that creates them succeeds; NULL before that.
    organization_id uuid        REFERENCES organizations(id) ON DELETE CASCADE,
    user_id         uuid        REFERENCES users(id) ON DELETE CASCADE,
    -- Deterministic request identity (signup email + nonce): re-submitting
    -- the same signup resumes the SAME job instead of starting a second one.
    request_key     text        NOT NULL UNIQUE,
    state           text        NOT NULL DEFAULT 'requested' CHECK (state IN (
                        'requested', 'running', 'waiting', 'completed',
                        'failed', 'retrying', 'cancelled')),
    -- Last completed lifecycle step.
    step            text        NOT NULL DEFAULT 'signup' CHECK (step IN (
                        'signup', 'user_created', 'organization_created',
                        'membership_created', 'plan_assigned',
                        'default_configuration', 'ready')),
    attempts        integer     NOT NULL DEFAULT 0,
    last_error      text        NOT NULL DEFAULT '',
    -- Requested plan code, resolved when the plan step runs.
    plan_code       text        NOT NULL DEFAULT '',
    payload         jsonb       NOT NULL DEFAULT '{}'::jsonb,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    completed_at    timestamptz
);
CREATE INDEX IF NOT EXISTS provisioning_jobs_open_idx ON provisioning_jobs (state, updated_at)
    WHERE state IN ('requested', 'running', 'waiting', 'retrying');

CREATE TABLE IF NOT EXISTS organization_audit_events (
    id              bigserial   PRIMARY KEY,
    organization_id uuid        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Who acted: a user, an API key, or the platform itself.
    actor_user_id   uuid        REFERENCES users(id) ON DELETE SET NULL,
    actor_key_id    uuid        REFERENCES saas_api_keys(id) ON DELETE SET NULL,
    actor_label     text        NOT NULL DEFAULT '',
    -- Dotted control-plane action (`saas.org.created`, `saas.key.revoked`, …).
    action          text        NOT NULL,
    target          text        NOT NULL DEFAULT '',
    outcome         text        NOT NULL DEFAULT 'success'
                                  CHECK (outcome IN ('success', 'failure', 'denied')),
    detail          text        NOT NULL DEFAULT '',
    ts              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS organization_audit_events_org_idx
    ON organization_audit_events (organization_id, ts DESC);
CREATE INDEX IF NOT EXISTS organization_audit_events_action_idx
    ON organization_audit_events (action, ts DESC);
