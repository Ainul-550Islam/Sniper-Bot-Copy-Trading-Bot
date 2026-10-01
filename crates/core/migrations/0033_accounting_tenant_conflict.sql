-- 0033_accounting_tenant_conflict.sql — PROMPT 3/10 (STEP 10 atomic swap #7):
-- the global-risk/accounting surfaces and the reconciliation queue become
-- TENANT-LOCAL.
--
-- WHAT THIS MIGRATION DOES (one transaction):
--   1. `ledger_events`:          PK (event_id)     → (organization_id, event_id);
--   2. `ledger_postings`:        the single-row FK on event_id is rebuilt as a
--                                composite FK (organization_id, event_id) →
--                                ledger_events(organization_id, event_id);
--   3. `global_positions`:       PK (position_key) → (organization_id, position_key);
--   4. `global_risk_decisions`:  PK (decision_id)  → (organization_id, decision_id);
--   5. `kill_switches`:          PK (scope)        → (organization_id, scope);
--   6. `reconciliation_state`:   PK (kind, subject) → (organization_id, kind, subject).
--
-- WHY (business identity):
--   * `ledger_events.event_id` — `led_` + digest of (kind, module, venue,
--     wallet, reference_id). The wallet belongs to a tenant; two tenants
--     recording the same class of fact against the same reference derive
--     colliding ids only in the single-tenant world. Identity is
--     tenant-local. `ledger_postings` keeps its GLOBAL UNIQUE
--     (event_id, seq) chain-signature rule (0023's documented set:
--     "(event_id, seq) stays as they are") — only its FK follows the
--     events table to the composite key, which is why the FK is rebuilt
--     here, in the same transaction, BEFORE the PK swap.
--   * `global_positions.position_key` —
--     `module|venue|wallet|strategy|asset|quote_asset|mode`. The wallet
--     is tenant-owned, so the aggregated exposure of a wallet is the
--     tenant's exposure. Identity is tenant-local.
--   * `global_risk_decisions.decision_id` — per-decision digest; the
--     decision belongs to the tenant whose risk gate produced it.
--   * `kill_switches.scope` — each tenant's trading can (and must) be
--     killed independently; `scope` names the switch ('all', 'module:x',
--     'symbol:y'), the OWNER is the organization. The deployment-wide
--     process control plane stays in `runtime_flags` (0010, global by
--     design — 0023 header).
--   * `reconciliation_state (kind, subject)` — the recon work queue.
--     Two tenants can hold the same subject id (same logical order id
--     after the swaps above, same venue order id): their verification
--     work must not collide or be claimed across tenants. Identity is
--     tenant-local.
--
-- WRITER COUPLING (same implementation batch):
--   * legacy deployment writer `AccountingRepo::{record_event, upsert_position,
--     record_decision, set_kill_switch}` (crates/core/src/db/accounting.rs)
--     now binds `organization_id = public.deployment_organization_id()`
--     (including the ledger_postings INSERT, which satisfies the rebuilt
--     FK) and uses the composite arbiters;
--   * `ReconRepo` (crates/core/src/db/repo.rs) uses the composite
--     (organization_id, kind, subject) arbiter;
--   * the tenant-scoped repository `trading_repository::polymarket::
--     reconciliation` and `trading_repository::intent::recovery` bind the
--     acting tenant with the same arbiters.
--
-- FK SAFETY (audited): `ledger_postings.event_id` is the ONLY foreign key
-- referencing any swapped table (0015). It is dropped and rebuilt against
-- the composite PRIMARY KEY inside this same transaction — there is no
-- interval where the FK or the PK is missing.
--
-- Forward-only; idempotent; no data rewrite.

-- ── 1. ledger_postings FK → composite, then ledger_events PK swap ──────
ALTER TABLE ledger_postings
    DROP CONSTRAINT IF EXISTS ledger_postings_event_id_fkey;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'ledger_events_pkey'
          AND  conrelid = 'public.ledger_events'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.ledger_events DROP CONSTRAINT ledger_events_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.ledger_events
             ADD PRIMARY KEY (organization_id, event_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'ledger_postings_event_tenant_fkey'
          AND  conrelid = 'public.ledger_postings'::regclass
    ) THEN
        ALTER TABLE public.ledger_postings
            ADD CONSTRAINT ledger_postings_event_tenant_fkey
            FOREIGN KEY (organization_id, event_id)
            REFERENCES public.ledger_events (organization_id, event_id)
            ON DELETE RESTRICT;
    END IF;
END
$$;

-- ── 2. global_positions: PK (position_key) → composite ─────────────────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'global_positions_pkey'
          AND  conrelid = 'public.global_positions'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.global_positions DROP CONSTRAINT global_positions_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.global_positions
             ADD PRIMARY KEY (organization_id, position_key)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 3. global_risk_decisions: PK (decision_id) → composite ─────────────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'global_risk_decisions_pkey'
          AND  conrelid = 'public.global_risk_decisions'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.global_risk_decisions DROP CONSTRAINT global_risk_decisions_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.global_risk_decisions
             ADD PRIMARY KEY (organization_id, decision_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 4. kill_switches: PK (scope) → composite ───────────────────────────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'kill_switches_pkey'
          AND  conrelid = 'public.kill_switches'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.kill_switches DROP CONSTRAINT kill_switches_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.kill_switches
             ADD PRIMARY KEY (organization_id, scope)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 5. reconciliation_state: PK (kind, subject) → composite ────────────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'reconciliation_state_pkey'
          AND  conrelid = 'public.reconciliation_state'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.reconciliation_state DROP CONSTRAINT reconciliation_state_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.reconciliation_state
             ADD PRIMARY KEY (organization_id, kind, subject)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- Verification (operator): composite PKs + the rebuilt FK; the global
-- chain-signature UNIQUE on ledger_postings (event_id, seq) is untouched.
--    SELECT conrelid::regclass, conname, pg_get_constraintdef(oid)
--    FROM   pg_constraint
--    WHERE  conrelid IN ('public.ledger_events'::regclass,
--                        'public.ledger_postings'::regclass,
--                        'public.global_positions'::regclass,
--                        'public.global_risk_decisions'::regclass,
--                        'public.kill_switches'::regclass,
--                        'public.reconciliation_state'::regclass)
--    AND    contype IN ('p', 'f', 'u')
--    ORDER  BY conrelid::regclass::text, contype;
