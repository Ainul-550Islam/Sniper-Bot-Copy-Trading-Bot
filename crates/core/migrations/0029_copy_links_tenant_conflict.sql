-- 0029_copy_links_tenant_conflict.sql — PROMPT 3/10 (STEP 10 atomic swap #4):
-- the copy-trading surfaces become TENANT-LOCAL.
--
-- WHAT THIS MIGRATION DOES (one transaction):
--   1. `copy_leaders`:  PRIMARY KEY (address)     → (organization_id, address);
--   2. `copy_events`:   PRIMARY KEY (event_id)    → (organization_id, event_id);
--   3. `copy_links`:    PRIMARY KEY (position_id) → (organization_id, position_id).
--
-- WHY (business identity):
--   * `copy_leaders.address` — the EXTERNAL Solana leader address. The
--     same leader wallet MAY be configured by MULTIPLE tenants: the
--     address is a globally-known on-chain identity, but FOLLOWING it
--     (label, status, sizing counters) is each tenant's own
--     configuration. A global PK would let the second tenant's
--     upsert OVERWRITE the first tenant's leader row (its counters,
--     its pause state) — cross-tenant state corruption through the
--     conflict path. Identity is tenant-local.
--   * `copy_events.event_id` — the deterministic leader-trade event id.
--     Two tenants following the same leader observe the SAME leader
--     trade and derive the SAME event id; each must record its OWN
--     outcome row (one tenant rejects by sizing, the other mirrors).
--     A global PK collapses the second tenant's record onto the
--     first's. Identity is tenant-local.
--   * `copy_links.position_id` — the follower's mirrored position.
--     Position ids are app-assigned per tenant, but the link's
--     business owner is the tenant that mirrors the trade; keying the
--     link tenant-locally keeps every link read/write scoped to the
--     owning organization and lets the same position-id scheme exist
--     independently per tenant. Identity is tenant-local.
--
-- WRITER COUPLING (same implementation batch):
--   * legacy deployment writer `CopyRepo::{upsert_leader, record_event,
--     upsert_link}` (crates/core/src/db/copy.rs) now binds
--     `organization_id = public.deployment_organization_id()` and uses
--     the composite arbiters;
--   * the tenant-scoped repository `trading_repository::copy::*` binds
--     the acting tenant with the same arbiters.
--
-- PRESERVED:
--   * `copy_leader_events` (bigserial PK, append-only) needs no swap;
--     its reads are tenant-scoped in the new repositories;
--   * no foreign keys reference these three tables (audited);
--   * every existing index stays (copy_links_leader_mint_open_idx etc.)
--     plus the 0024 tenant-leading forms.
--
-- Forward-only; idempotent; no data rewrite.

-- ── 1. copy_leaders: PK (address) → (organization_id, address) ──────────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'copy_leaders_pkey'
          AND  conrelid = 'public.copy_leaders'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.copy_leaders DROP CONSTRAINT copy_leaders_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.copy_leaders
             ADD PRIMARY KEY (organization_id, address)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 2. copy_events: PK (event_id) → (organization_id, event_id) ─────────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'copy_events_pkey'
          AND  conrelid = 'public.copy_events'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.copy_events DROP CONSTRAINT copy_events_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.copy_events
             ADD PRIMARY KEY (organization_id, event_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 3. copy_links: PK (position_id) → (organization_id, position_id) ────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'copy_links_pkey'
          AND  conrelid = 'public.copy_links'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.copy_links DROP CONSTRAINT copy_links_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.copy_links
             ADD PRIMARY KEY (organization_id, position_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- Verification (operator): three composite PKs.
--    SELECT conrelid::regclass, conname, pg_get_constraintdef(oid)
--    FROM   pg_constraint
--    WHERE  conrelid IN ('public.copy_leaders'::regclass,
--                        'public.copy_events'::regclass,
--                        'public.copy_links'::regclass)
--    AND    contype = 'p';
