-- 0079_intent_broadcast_reservations.sql — durable PRE-BROADCAST reservation
-- for one logical trade (execution-ownership §E / crash point C).
--
-- The write-ahead journal (0007) records that an attempt exists. It does not
-- stop a SECOND replica, or a restarted process, from claiming the same
-- logical trade and broadcasting it again while the first attempt's outcome
-- is still ambiguous (send-unknown, crash between send and link). This table
-- makes the claim a single atomic INSERT: exactly one owner,
-- `ON CONFLICT DO NOTHING`, so a duplicate reservation is observable instead
-- of silently ignored.
--
-- Ownership is LEASED, not permanent: `lease_until` bounds how long an owner
-- may hold the claim without a heartbeat. A takeover bumps `claim_epoch`, so
-- every later `complete`/`extend` by the old owner is a CAS no-op — a stale
-- replica cannot mutate a reservation a newer one owns.
--
-- Additive and restart-safe: IF NOT EXISTS only, no rewrites of existing data.
-- Tenant key `organization_id` puts this table under the 0062 FORCE-RLS
-- tenant policy (`app.current_org_id()`), like the rest of the ledger.

CREATE TABLE IF NOT EXISTS intent_broadcast_reservations (
    organization_id uuid        NOT NULL,
    trade_key       text        NOT NULL CHECK (length(btrim(trade_key)) > 0),
    intent_id       text        NOT NULL DEFAULT '',
    module          text        NOT NULL DEFAULT '',
    symbol          text        NOT NULL DEFAULT '',
    side            text        NOT NULL DEFAULT '',
    owner_id        text        NOT NULL CHECK (length(btrim(owner_id)) > 0),
    claim_epoch     bigint      NOT NULL DEFAULT 1 CHECK (claim_epoch > 0),
    -- 'reserved' | 'linked' | 'abandoned'
    status          text        NOT NULL DEFAULT 'reserved'
                    CHECK (status IN ('reserved', 'linked', 'abandoned')),
    signature       text,
    lease_until     timestamptz NOT NULL,
    takeover_count  bigint      NOT NULL DEFAULT 0,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, trade_key)
);

-- Operator / reconciliation scans: live reservations whose lease lapsed are
-- candidates for takeover or for an ambiguous-outcome review.
CREATE INDEX IF NOT EXISTS intent_broadcast_reservations_live_idx
    ON intent_broadcast_reservations (organization_id, lease_until)
    WHERE status = 'reserved';

CREATE INDEX IF NOT EXISTS intent_broadcast_reservations_updated_idx
    ON intent_broadcast_reservations (organization_id, updated_at);

-- ── Fencing helper (0009 counterpart) ──────────────────────────────────
-- A reservation is only useful if a stale owner can be told apart from the
-- current one. `claim_epoch`/`owner_id` are returned by every write so the
-- caller can prove its own identity; the CAS predicates live in the
-- repository (`db::repo::BroadcastReservationRepo`).
COMMENT ON TABLE intent_broadcast_reservations IS
    'Durable leased pre-broadcast reservation per logical trade; claim_epoch is the fencing token.';

-- ── Tenant isolation ───────────────────────────────────────────────────
-- Migration 0062 installs the FORCE-RLS policy loop, but it ran before this
-- table existed, so the policy is installed here — guarded so the migration
-- still applies on a database without the Batch 01 RLS foundation.
DO $$
BEGIN
    IF to_regprocedure('app.current_org_id()') IS NOT NULL THEN
        ALTER TABLE public.intent_broadcast_reservations ENABLE ROW LEVEL SECURITY;
        ALTER TABLE public.intent_broadcast_reservations FORCE ROW LEVEL SECURITY;
        DROP POLICY IF EXISTS tenant_org_isolation ON public.intent_broadcast_reservations;
        CREATE POLICY tenant_org_isolation ON public.intent_broadcast_reservations
            USING (organization_id = app.current_org_id())
            WITH CHECK (organization_id = app.current_org_id());
    END IF;
END $$;

-- Runtime grants, guarded on role existence (the roles are created by 0060).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_app') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.intent_broadcast_reservations TO bot_app;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_system') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.intent_broadcast_reservations TO bot_system;
    END IF;
END $$;
