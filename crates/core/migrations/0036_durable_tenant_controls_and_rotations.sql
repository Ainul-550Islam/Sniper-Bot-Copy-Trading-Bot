-- 0036_durable_tenant_controls_and_rotations.sql — P0 §S-2 / §S-4 / WS replay:
-- move three pieces of SECURITY-RELEVANT state out of process memory and into
-- the database.
--
-- WHY THIS MIGRATION EXISTS
-- -------------------------
-- Three surfaces kept their authoritative state in a process-global
-- `OnceLock<Mutex<HashMap<..>>>`:
--
--   1. `trading_data_plane::module_controls` — the TENANT KILL-SWITCH.
--      A tenant pausing a module paused it on exactly ONE replica; every
--      other replica kept trading, and a restart silently re-enabled the
--      module. For a system that moves money this is the most dangerous
--      of the three.
--   2. `saas::custody_rotation` — custody ROTATION state. A rotation
--      created on replica A could not be activated on replica B, a
--      restart stranded a profile mid-rotation, and no durable record of
--      a key-rotation event existed for an auditor to read.
--   3. `saas::websocket_auth` — WebSocket ticket REPLAY protection. The
--      seen-token set was per-process, so a captured ticket replayed
--      simply by reaching a different replica.
--
-- Consequence stated plainly: the deployment could not safely run more
-- than one replica. This migration is the schema half of the fix; the
-- Rust half is `trading_data_plane::module_control_store`,
-- `saas::custody_rotation_store` and `saas::websocket_replay_store`.
--
-- DESIGN RULES FOLLOWED (same as 0023–0035)
-- -----------------------------------------
--   * Forward-only, additive, idempotent (`IF NOT EXISTS` everywhere) —
--     re-applies cleanly on a fresh database and on a live one.
--   * Tenant ownership is a COLUMN and part of the KEY, never an
--     application-side filter: `organization_id` leads every primary key
--     and every operational index, exactly like 0026–0033.
--   * `ON DELETE RESTRICT` to `organizations(id)`: control and custody
--     truth is never silently cascade-deleted. Tenant removal stays
--     governed by the retention workflow (0021).
--   * Optimistic concurrency: every mutable row carries a monotonically
--     increasing `version`. Two replicas racing the same row cannot
--     interleave a lost update — the writer compares-and-swaps on it.
--   * Audit metadata on every row: who (`updated_by` actor label, never a
--     secret), when (`created_at` / `updated_at`), and a
--     `correlation_id` so an action can be joined to the request that
--     caused it.
--   * No DEFAULT deployment_organization_id() bridge here. These are NEW
--     tables with NEW writers that always bind the acting tenant
--     explicitly, so the compatibility default that 0024 needed for the
--     legacy writers would only be able to hide a bug.

-- ─────────────────────────────────────────────────────────────────────────
-- 1. tenant_module_controls — the durable tenant kill-switch (§S-2)
-- ─────────────────────────────────────────────────────────────────────────
--
-- Identity is (organization_id, module): one override per tenant per
-- module. ABSENCE of a row is the honest "no tenant override" — the
-- effective state is then the entitlement-governed default. That keeps
-- the exact semantics the in-memory map had, so the API surface does not
-- change.
--
-- `enabled = false` is the pause. `reason` is operator-supplied text
-- (bounded at the same 280 chars the HTTP layer validates, enforced here
-- too so a non-HTTP writer cannot bypass it).
CREATE TABLE IF NOT EXISTS tenant_module_controls (
    organization_id uuid        NOT NULL
        REFERENCES organizations(id) ON DELETE RESTRICT,
    -- 'sniper' | 'copy' | 'polymarket' | 'telegram' | 'contract'
    -- (bot_core::models::BotModule::as_str). Free text with a non-empty
    -- check rather than an enum: the module vocabulary lives in Rust and
    -- a CHECK list here would need a migration every time it grows.
    module          text        NOT NULL CHECK (module <> ''),
    -- false = the tenant has PAUSED this module.
    enabled         boolean     NOT NULL,
    -- Why, for disables. Empty for enables.
    reason          text        NOT NULL DEFAULT '' CHECK (length(reason) <= 280),
    -- Optimistic-concurrency token. Strictly increasing, never reused.
    version         bigint      NOT NULL DEFAULT 1 CHECK (version > 0),
    -- Non-secret actor label from the authenticated context.
    updated_by      text        NOT NULL DEFAULT '',
    -- Request correlation, for joining this change to its audit event.
    correlation_id  text        NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, module)
);

-- Operational read: "which modules has ANY tenant paused right now?"
-- Partial, so the index only carries the (rare) disabled rows.
CREATE INDEX IF NOT EXISTS tenant_module_controls_disabled_idx
    ON tenant_module_controls (organization_id, module)
    WHERE enabled = false;

-- Operator triage: most recent control changes first.
CREATE INDEX IF NOT EXISTS tenant_module_controls_updated_idx
    ON tenant_module_controls (updated_at DESC);

-- ─────────────────────────────────────────────────────────────────────────
-- 2. custody_rotations — durable custody rotation state (§S-4)
-- ─────────────────────────────────────────────────────────────────────────
--
-- One row per rotation workflow instance. The state machine mirrors
-- `bot_core::custody::rotation::RotationState` exactly; the CHECK is the
-- schema-level copy of that closed vocabulary, so an out-of-domain state
-- can never be persisted (the 0035 lesson: a status CHECK that is
-- narrower than the Rust enum fails every INSERT, silently, if the
-- write's Result is discarded).
--
-- `profile_id` / `old_signer_id` / `new_signer_id` are the custody
-- identifiers; NO key material, no provider credential and no signing
-- material is stored here or anywhere else in this schema.
CREATE TABLE IF NOT EXISTS custody_rotations (
    id              uuid        PRIMARY KEY,
    organization_id uuid        NOT NULL
        REFERENCES organizations(id) ON DELETE RESTRICT,
    profile_id      text        NOT NULL CHECK (profile_id <> ''),
    old_signer_id   text        NOT NULL CHECK (old_signer_id <> ''),
    new_signer_id   text        NOT NULL CHECK (new_signer_id <> ''),
    -- Closed vocabulary — the schema-level copy of
    -- bot_core::custody::rotation::RotationState::ALL. All five, exactly:
    -- a CHECK narrower than the Rust enum rejects legitimate writes.
    state           text        NOT NULL
        CHECK (state IN ('pending', 'active', 'draining', 'revoked', 'failed')),
    -- Emergency revoke of the OLD signer before the new one is proven.
    force_revoked   boolean     NOT NULL DEFAULT false,
    provider_type   text        NOT NULL DEFAULT '',
    -- Why a rotation failed / was revoked. Never carries secret detail.
    -- Mirrors RotationRecord::failure_reason. Never carries secret detail.
    failure_reason  text        NOT NULL DEFAULT '' CHECK (length(failure_reason) <= 500),
    activated_at    timestamptz,
    revoked_at      timestamptz,
    version         bigint      NOT NULL DEFAULT 1 CHECK (version > 0),
    created_by      text        NOT NULL DEFAULT '',
    updated_by      text        NOT NULL DEFAULT '',
    correlation_id  text        NOT NULL DEFAULT '',
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    -- The old/new pair must differ: rotating a signer onto itself is a
    -- no-op that would otherwise look like a completed rotation.
    CONSTRAINT custody_rotations_distinct_signers
        CHECK (old_signer_id <> new_signer_id)
);

-- Tenant-scoped lookup: every read path is "this tenant's rotations".
-- organization_id LEADS the index so a cross-tenant scan is not even
-- cheap, let alone correct.
CREATE INDEX IF NOT EXISTS custody_rotations_tenant_idx
    ON custody_rotations (organization_id, created_at DESC);

-- Profile drill-down within a tenant.
CREATE INDEX IF NOT EXISTS custody_rotations_profile_idx
    ON custody_rotations (organization_id, profile_id, created_at DESC);

-- AT MOST ONE rotation in flight per (tenant, profile). Two concurrent
-- in-flight rotations on one custody profile is the exact race that can
-- revoke a signer the other rotation is relying on. Enforced as a partial
-- unique index so terminal rows (revoked/failed) are unlimited.
--
-- BEHAVIOUR CHANGE, stated plainly: the in-memory store allowed
-- unlimited concurrent rotations per profile. Under 0036 the second
-- concurrent one is refused with 409. That is the point of the fix.
CREATE UNIQUE INDEX IF NOT EXISTS custody_rotations_one_inflight_per_profile
    ON custody_rotations (organization_id, profile_id)
    WHERE state IN ('pending', 'active', 'draining');

-- ─────────────────────────────────────────────────────────────────────────
-- 3. ws_replay_tokens — shared WebSocket replay protection
-- ─────────────────────────────────────────────────────────────────────────
--
-- The PRIMARY KEY is the token HASH, and the hash alone: replay
-- protection must be global across tenants and replicas, because the
-- question "has this exact credential already been presented?" is not a
-- tenant-scoped question. `organization_id` is recorded for audit (it is
-- nullable: a ticket can be refused before its tenant is resolved) and is
-- deliberately NOT part of the key — making it part of the key would let
-- the same ticket be replayed once per tenant.
--
-- Only the SHA-256 hash is stored. The plaintext token never reaches
-- this table, these indexes, or the write-ahead log.
CREATE TABLE IF NOT EXISTS ws_replay_tokens (
    token_hash      text        PRIMARY KEY CHECK (length(token_hash) = 64),
    organization_id uuid        REFERENCES organizations(id) ON DELETE SET NULL,
    first_seen_at   timestamptz NOT NULL DEFAULT now(),
    -- The end of the replay window. A row past this instant is garbage,
    -- not a valid "already seen" answer: the reader filters on it so an
    -- un-swept table can never refuse a legitimate fresh ticket.
    expires_at      timestamptz NOT NULL,
    correlation_id  text        NOT NULL DEFAULT ''
);

-- TTL sweep. Maintenance is global by design (it is not tenant data),
-- which is why this index does not lead with organization_id.
CREATE INDEX IF NOT EXISTS ws_replay_tokens_expiry_idx
    ON ws_replay_tokens (expires_at);

-- Audit: which tickets did this tenant present?
CREATE INDEX IF NOT EXISTS ws_replay_tokens_tenant_idx
    ON ws_replay_tokens (organization_id, first_seen_at DESC)
    WHERE organization_id IS NOT NULL;

-- ─────────────────────────────────────────────────────────────────────────
-- 4. Operator verification
-- ─────────────────────────────────────────────────────────────────────────
--   SELECT table_name FROM information_schema.tables
--    WHERE table_name IN ('tenant_module_controls','custody_rotations','ws_replay_tokens');
--
--   SELECT conname, pg_get_constraintdef(oid) FROM pg_constraint
--    WHERE conrelid IN ('public.tenant_module_controls'::regclass,
--                       'public.custody_rotations'::regclass,
--                       'public.ws_replay_tokens'::regclass);
--
-- ROLLBACK LIMITATION (stated honestly): this migration is additive, so
-- rolling the SCHEMA back is a `DROP TABLE` of three new tables. Rolling
-- the APPLICATION back to a build that reads the in-memory maps is safe
-- but LOSES the durable state — a tenant pause recorded here would not be
-- honoured by the older binary. Roll the application forward, not back,
-- once a tenant has used the durable kill-switch.
