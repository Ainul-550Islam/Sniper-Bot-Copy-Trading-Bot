-- 0024_trading_tenant_backfill_constraints.sql — STEP 2 (enterprise tenant
-- isolation): transition the trading-truth tenant columns introduced by
-- 0023 from "introduced" to "enforced".
--
-- WHAT THIS MIGRATION DOES
-- ------------------------
--   1. Resolves the deployment organization — the ONE organization a
--      legacy single-operator deployment maps to — reusing the EXISTING
--      mechanism (slug `deployment`, `DEPLOYMENT_ORG_SLUG` in
--      crates/server/src/saas/middleware.rs, provisioned by
--      `ensure_deployment_organization` in crates/server/src/saas/mod.rs).
--      No second tenant is invented.
--   2. Backfills every pre-0023 row to that organization,
--      deterministically. No random organization is ever assigned, no row
--      is duplicated, no row is deleted.
--   3. Fails CLOSED when ownership is ambiguous: if unmapped rows exist
--      alongside other organizations and no deployment organization can
--      be resolved, the migration raises with per-table counts instead of
--      guessing (rows silently becoming cross-tenant data is the one
--      outcome this migration must never produce).
--   4. Sets `organization_id NOT NULL` on all 32 trading-truth tables,
--      together with a `deployment_organization_id()` column DEFAULT that
--      keeps every EXISTING writer working (see THE DEFAULT BRIDGE below).
--   5. Documents the tenant-composite UNIQUE constraints the tenant-
--      scoped repositories (STEP 10–14) will use with
--      `ON CONFLICT (organization_id, ...)` — and deliberately defers
--      creating them to the STEP 10–14 per-table atomic swap (see the
--      CONSTRAINT STRATEGY section: a composite added alongside its
--      still-implying global constraint breaks the designed concurrent
--      upsert grace of this HA deployment).
--   6. Adds tenant-leading indexes for the high-frequency query paths
--      (recovery scans, tenant listings, reconciliation and ledger joins).
--
-- THE DEPLOYMENT-ORGANIZATION MAPPING (reusing the existing mechanism)
-- --------------------------------------------------------------------
--   * The deployment organization is the organization whose slug is
--     `deployment` — exactly what `ensure_deployment_organization`
--     provisions at startup (name `Deployment`, active, no creator).
--   * If it already exists (an install that has run the SaaS control
--     plane at least once), it is reused; its id backfills the legacy rows.
--   * If it does NOT exist and the database is a pure legacy
--     single-operator deployment (unmapped rows exist, no other
--     organization does), this migration creates it with the same field
--     values the Rust function uses, so both creators converge on the
--     same row and `ensure_deployment_organization` reuses it (and now
--     also ensures its Business plan — see the matching change in
--     crates/server/src/saas/mod.rs).
--   * If it does NOT exist, unmapped rows exist AND other organizations
--     exist, ownership of those rows is genuinely ambiguous: they could
--     belong to any tenant. The migration REFUSES (RAISE EXCEPTION) with
--     the exact per-table unmapped counts. Fix the data (set
--     organization_id explicitly, or provide the deployment organization)
--     and restart: sqlx re-runs the unapplied migration and completes.
--   * If no unmapped rows exist (a fresh database, or a fully attributed
--     one), the backfill is a no-op and no organization is created from
--     here — fresh installs keep getting their deployment organization
--     from the application exactly as before.
--
-- THE DEFAULT BRIDGE (why NOT NULL is safe the same day it lands)
-- ----------------------------------------------------------------
-- The legacy single-operator writers (OrderManager, the copy/Polymarket
-- engines, the recovery and reconciliation workers — the code that exists
-- TODAY, before the STEP 10–14 tenant repositories arrive) insert into
-- these tables WITHOUT an organization_id column. Plain `SET NOT NULL`
-- would break every one of them, including the CI database tests, the
-- night the migration deploys. Instead every column gets
--     DEFAULT deployment_organization_id()
-- — the SQL twin of `ensure_deployment_organization`: resolve the
-- deployment organization by slug, create it once if missing, return its
-- id. Consequences:
--   * Legacy writers keep working unchanged; their rows are attributed
--     to the deployment organization, which is the documented mapping
--     for everything the deployment-global code path writes.
--   * NULL becomes impossible at the schema level: omitting the column
--     yields the deployment organization, and an explicit NULL is
--     rejected by NOT NULL (fail closed).
--   * Tenant-scoped writers (STEP 10+) ALWAYS bind organization_id
--     explicitly from a typed TradingTenantScope — for them the default
--     never fires, and the runtime/broadcast guards (STEP 18/20)
--     re-verify attribution. The default is a compatibility bridge for
--     the deployment-global code path, not a tenant fallback.
--
-- CONSTRAINT STRATEGY (why the old constraints stay, for now)
-- ----------------------------------------------------------------
-- Every identity constraint on these tables is an ACTIVE `ON CONFLICT`
-- conflict target in the current Rust code (verified against the tree):
--   orders (id) and (idempotency_key); transactions (signature);
--   positions (id); trades (id); idempotency_keys (scope, key);
--   dedup_keys (namespace, key); reconciliation_state (kind, subject);
--   execution_intents (intent_id); execution_claims (execution_id);
--   execution_lifecycle (intent_id); copy_leaders (address);
--   copy_events (event_id); copy_links (position_id);
--   poly_signals (signal_id); poly_orders (venue_order_id);
--   poly_fills (fill_id); ledger_events (event_id);
--   ledger_postings (event_id, seq); global_positions (position_key);
--   global_risk_decisions (decision_id); kill_switches (scope).
-- Dropping or replacing any of them TODAY would make PostgreSQL reject
-- those `ON CONFLICT` specifications ("no unique or exclusion constraint
-- matching the ON CONFLICT specification") and break the working
-- deployment — which the program forbids. Therefore this migration:
--   * KEEPS every existing PRIMARY KEY / UNIQUE constraint untouched.
--     Transitional effect: a cross-tenant identifier collision is still
--     REJECTED by the old global constraint — fail-closed availability
--     limiting (e.g. two tenants following the same leader address),
--     never data mixing.
--
-- WHY NO ADDITIVE tenant-composite UNIQUE constraints LAND HERE (the
-- concurrent-upsert hazard, found by running the real test suite against
-- this migration on PostgreSQL 17):
--   A second UNIQUE index over a SUPERSET of an existing conflict
--   target's columns (e.g. (organization_id, execution_id) next to the
--   execution_claims primary key (execution_id)) is logically redundant
--   today — the old global constraint already implies it — and
--   operationally HAZARDOUS: PostgreSQL's ON CONFLICT DO UPDATE
--   guarantees the atomic insert-or-update path only for the named
--   ARBITER index. When two replicas race an upsert on the same key
--   (exactly what execution_claims, dedup_keys, copy_events,
--   idempotency_keys, kill_switches... are DESIGNED for in this HA
--   deployment), the loser can hit the conflict on the NON-arbiter
--   composite index and surface a raw unique-violation error instead of
--   taking the graceful arbiter path. That broke
--   two_contexts_pg_claim_race_single_owner; removing the composite
--   restored it. The tenant-composite constraints therefore arrive in
--   the STEP 10–14 migrations as an ATOMIC per-table swap — drop the
--   old global constraint, add the tenant composite, and rewrite the
--   writer's ON CONFLICT arbiter to (organization_id, ...) in the SAME
--   deployment (migrations run inside the new binary's startup, so the
--   swap and the new writer code ship together). The interim guarantee
--   is the still-active global constraint: globally unique ⇒ unique
--   per tenant; cross-tenant collisions fail closed.
--
-- Idempotency: every block re-applies cleanly (CREATE OR REPLACE,
-- guarded constraint adds, IF-NOT-EXISTS indexes, no-op UPDATEs, SET
-- DEFAULT / SET NOT NULL which are stable on re-run). Safe on a fresh
-- database (nothing to backfill, no organization created here) and on an
-- existing deployment (backfill + enforcement in one transaction).
--
-- Ordering: 0023 introduced the columns; this file runs strictly after
-- it. Nothing is dropped, renamed or rewritten; existing primary keys
-- and business uniqueness rules are preserved.

-- ══════════════════════════════════════════════════════════════════════
-- 1. The deployment-organization resolver (SQL twin of
--    ensure_deployment_organization; used as the column DEFAULT below)
-- ══════════════════════════════════════════════════════════════════════
-- VOLATILE: it may INSERT the organization on first use. The fixed
-- search_path makes it safe to invoke from column DEFAULTs under any
-- role. Field values match the Rust creator exactly (slug `deployment`,
-- name `Deployment`, active); plan assignment stays in Rust
-- (ensure_deployment_organization now ensures the Business plan for a
-- pre-existing deployment organization too).

CREATE OR REPLACE FUNCTION public.deployment_organization_id()
RETURNS uuid
LANGUAGE plpgsql
VOLATILE
SET search_path = public
AS $fn$
DECLARE
    v_id uuid;
BEGIN
    SELECT id INTO v_id FROM organizations
    WHERE lower(slug) = 'deployment'
    LIMIT 1;

    IF v_id IS NULL THEN
        INSERT INTO organizations (id, slug, name, status)
        VALUES (gen_random_uuid(), 'deployment', 'Deployment', 'active')
        ON CONFLICT DO NOTHING;

        SELECT id INTO v_id FROM organizations
        WHERE lower(slug) = 'deployment'
        LIMIT 1;
    END IF;

    IF v_id IS NULL THEN
        RAISE EXCEPTION 'deployment organization (slug ''deployment'') could not be resolved or created'
            USING ERRCODE = 'P0001',
                  HINT = 'Create the ''deployment'' organization (or fix the conflicting row), then retry.';
    END IF;

    RETURN v_id;
END
$fn$;

-- ══════════════════════════════════════════════════════════════════════
-- 2. Resolve / create the deployment organization for the backfill and
--    fail closed on ambiguous ownership
-- ══════════════════════════════════════════════════════════════════════

DO $$
DECLARE
    v_tables constant text[] := ARRAY[
        'orders',
        'order_status_history',
        'executions',
        'transactions',
        'idempotency_keys',
        'positions',
        'trades',
        'balance_snapshots',
        'dedup_keys',
        'risk_events',
        'audit_events',
        'reconciliation_state',
        'execution_intents',
        'execution_claims',
        'execution_claim_events',
        'execution_lifecycle',
        'execution_lifecycle_events',
        'copy_leaders',
        'copy_leader_events',
        'copy_events',
        'copy_links',
        'poly_signals',
        'poly_orders',
        'poly_fills',
        'poly_recon_findings',
        'ledger_events',
        'ledger_postings',
        'global_positions',
        'global_risk_decisions',
        'kill_switches',
        'kill_switch_events',
        'accounting_recon_findings'
    ];
    t text;
    v_unmapped bigint := 0;
    v_detail text := '';
    v_rows bigint;
    v_deployment uuid;
    v_org_count bigint;
BEGIN
    -- Count unmapped rows per table (drives both the refusal report and
    -- the operator-visible backfill notices).
    FOREACH t IN ARRAY v_tables LOOP
        EXECUTE format('SELECT count(*) FROM %I WHERE organization_id IS NULL', t) INTO v_rows;
        IF v_rows > 0 THEN
            v_unmapped := v_unmapped + v_rows;
            v_detail := v_detail || format(' %s=%s', t, v_rows);
        END IF;
    END LOOP;

    -- Resolve the deployment organization (the single legacy owner).
    SELECT id INTO v_deployment FROM organizations
    WHERE lower(slug) = 'deployment'
    LIMIT 1;

    IF v_deployment IS NULL AND v_unmapped > 0 THEN
        SELECT count(*) INTO v_org_count FROM organizations;
        IF v_org_count > 0 THEN
            RAISE EXCEPTION USING
                ERRCODE = 'P0001',
                MESSAGE = format(
                    '0024_trading_tenant_backfill_constraints: %s trading-truth row(s) have no organization_id while %s other organization(s) exist and the deployment organization (slug ''deployment'') does not',
                    v_unmapped, v_org_count),
                DETAIL = 'unmapped rows:' || v_detail,
                HINT = 'Ownership of pre-0024 rows is ambiguous on a multi-organization database. Set organization_id explicitly on these rows (or provide the ''deployment'' organization), then restart: the migration re-runs and completes.';
        ELSE
            -- Pure legacy single-operator deployment: create the
            -- deployment organization exactly as ensure_deployment_organization
            -- does (same slug, name and status), so both creators converge
            -- on one row.
            INSERT INTO organizations (id, slug, name, status)
            VALUES (gen_random_uuid(), 'deployment', 'Deployment', 'active');
        END IF;
    END IF;
END$$;

-- ══════════════════════════════════════════════════════════════════════
-- 3. Deterministic backfill — every legacy row maps to the deployment
--    organization (single-deployment semantics; no row duplicated,
--    no row dropped, re-runs are no-ops)
-- ══════════════════════════════════════════════════════════════════════

DO $$
DECLARE
    v_tables constant text[] := ARRAY[
        'orders',
        'order_status_history',
        'executions',
        'transactions',
        'idempotency_keys',
        'positions',
        'trades',
        'balance_snapshots',
        'dedup_keys',
        'risk_events',
        'audit_events',
        'reconciliation_state',
        'execution_intents',
        'execution_claims',
        'execution_claim_events',
        'execution_lifecycle',
        'execution_lifecycle_events',
        'copy_leaders',
        'copy_leader_events',
        'copy_events',
        'copy_links',
        'poly_signals',
        'poly_orders',
        'poly_fills',
        'poly_recon_findings',
        'ledger_events',
        'ledger_postings',
        'global_positions',
        'global_risk_decisions',
        'kill_switches',
        'kill_switch_events',
        'accounting_recon_findings'
    ];
    t text;
    v_updated bigint;
    v_total bigint := 0;
BEGIN
    FOREACH t IN ARRAY v_tables LOOP
        EXECUTE format(
            'UPDATE %I SET organization_id = (SELECT id FROM organizations WHERE lower(slug) = ''deployment'' LIMIT 1) WHERE organization_id IS NULL',
            t
        );
        GET DIAGNOSTICS v_updated = ROW_COUNT;
        v_total := v_total + v_updated;
        IF v_updated > 0 THEN
            RAISE NOTICE '0024 backfill %: % row(s) -> deployment organization', t, v_updated;
        END IF;
    END LOOP;
    RAISE NOTICE '0024 backfill total: % row(s)', v_total;
END$$;

-- ══════════════════════════════════════════════════════════════════════
-- 4. Enforce: DEFAULT bridge + NOT NULL on all 32 tables
-- ══════════════════════════════════════════════════════════════════════
-- The DEFAULT is what makes NOT NULL safe for the existing writers (see
-- THE DEFAULT BRIDGE above). Both statements are stable on re-run.

DO $$
DECLARE
    v_tables constant text[] := ARRAY[
        'orders',
        'order_status_history',
        'executions',
        'transactions',
        'idempotency_keys',
        'positions',
        'trades',
        'balance_snapshots',
        'dedup_keys',
        'risk_events',
        'audit_events',
        'reconciliation_state',
        'execution_intents',
        'execution_claims',
        'execution_claim_events',
        'execution_lifecycle',
        'execution_lifecycle_events',
        'copy_leaders',
        'copy_leader_events',
        'copy_events',
        'copy_links',
        'poly_signals',
        'poly_orders',
        'poly_fills',
        'poly_recon_findings',
        'ledger_events',
        'ledger_postings',
        'global_positions',
        'global_risk_decisions',
        'kill_switches',
        'kill_switch_events',
        'accounting_recon_findings'
    ];
    t text;
BEGIN
    FOREACH t IN ARRAY v_tables LOOP
        EXECUTE format(
            'ALTER TABLE %I ALTER COLUMN organization_id SET DEFAULT public.deployment_organization_id()',
            t
        );
        EXECUTE format(
            'ALTER TABLE %I ALTER COLUMN organization_id SET NOT NULL',
            t
        );
    END LOOP;
END$$;

-- ══════════════════════════════════════════════════════════════════════
-- 5. Tenant-composite UNIQUE constraints — NOT added here (deferred to
--    the STEP 10–14 atomic swap; see the CONSTRAINT STRATEGY header)
-- ══════════════════════════════════════════════════════════════════════
-- These are the conflict targets the tenant-scoped repositories WILL use
-- (ON CONFLICT (organization_id, ...)) and the guarantee that the same
-- external identifier may exist independently per tenant once the legacy
-- global constraint is retired. They are deliberately NOT created in
-- this migration:
--   * Today every one of them is IMPLIED by the still-active global
--     constraint (UNIQUE(x) ⇒ UNIQUE(organization_id, x)), so they add
--     zero enforcement while the global constraint exists.
--   * A second unique index over a superset of a live ON CONFLICT
--     arbiter's columns turns the designed concurrent-upsert grace of
--     this deployment (HA claim races, cross-replica dedup, order
--     idempotency) into raw unique-violation errors when the loser's
--     conflict surfaces on the non-arbiter index. Verified live:
--     adding (organization_id, execution_id) to execution_claims broke
--     two_contexts_pg_claim_race_single_owner; removing it restored the
--     exactly-one-owner invariant.
-- The STEP 10–14 per-table sequence (one deployment, atomic in effect
-- because migrations run inside the new binary's startup): drop the old
-- global constraint → add the tenant composite → ship the writer whose
-- ON CONFLICT names the composite as arbiter. Planned composites:
--   orders (organization_id, idempotency_key)
--   idempotency_keys (organization_id, scope, key)
--   dedup_keys (organization_id, namespace, key)
--   reconciliation_state (organization_id, kind, subject)
--   execution_intents (organization_id, intent_id)
--   execution_claims (organization_id, execution_id)
--   execution_lifecycle (organization_id, intent_id)
--   copy_leaders (organization_id, address)
--   copy_events (organization_id, event_id)
--   copy_links (organization_id, position_id)
--   poly_signals (organization_id, signal_id)
--   poly_orders (organization_id, venue_order_id)
--   poly_fills (organization_id, fill_id)
--   ledger_events (organization_id, event_id)
--   global_positions (organization_id, position_key)
--   global_risk_decisions (organization_id, decision_id)
--   kill_switches (organization_id, scope)

-- ══════════════════════════════════════════════════════════════════════
-- 6. Tenant-leading indexes (every high-frequency path gets its
--    organization_id-first form; all existing indexes are preserved for
--    the deployment-global worker scans that legitimately remain global)
-- ══════════════════════════════════════════════════════════════════════

-- ── OMS (0002) ─────────────────────────────────────────────────────────
-- Tenant order listings (newest first).
CREATE INDEX IF NOT EXISTS orders_org_created_idx
    ON orders (organization_id, created_at DESC);
-- Recovery: non-terminal orders per tenant.
CREATE INDEX IF NOT EXISTS orders_org_status_updated_idx
    ON orders (organization_id, status, updated_at);
-- Per-tenant module dashboards.
CREATE INDEX IF NOT EXISTS orders_org_module_created_idx
    ON orders (organization_id, module, created_at DESC);
-- Reconciliation by chain signature / provider id, tenant-scoped.
CREATE INDEX IF NOT EXISTS orders_org_signature_idx
    ON orders (organization_id, signature) WHERE signature IS NOT NULL;
CREATE INDEX IF NOT EXISTS orders_org_external_id_idx
    ON orders (organization_id, external_id) WHERE external_id IS NOT NULL;
-- Per-tenant market views.
CREATE INDEX IF NOT EXISTS orders_org_symbol_idx
    ON orders (organization_id, symbol, created_at DESC);

CREATE INDEX IF NOT EXISTS order_status_history_org_order_idx
    ON order_status_history (organization_id, order_id, ts);

CREATE INDEX IF NOT EXISTS executions_org_order_idx
    ON executions (organization_id, order_id, ts);
CREATE INDEX IF NOT EXISTS executions_org_ts_idx
    ON executions (organization_id, ts);

CREATE INDEX IF NOT EXISTS transactions_org_order_idx
    ON transactions (organization_id, order_id);
CREATE INDEX IF NOT EXISTS transactions_org_status_idx
    ON transactions (organization_id, status, submitted_at);
-- Incident queries by signer, tenant-scoped.
CREATE INDEX IF NOT EXISTS transactions_org_signer_idx
    ON transactions (organization_id, signer);

-- Housekeeping of expired keys per tenant.
CREATE INDEX IF NOT EXISTS idempotency_keys_org_created_idx
    ON idempotency_keys (organization_id, created_at);

-- ── Positions / trades / balances (0003) ───────────────────────────────
CREATE INDEX IF NOT EXISTS positions_org_status_idx
    ON positions (organization_id, status) WHERE status IN ('open', 'closing');
CREATE INDEX IF NOT EXISTS positions_org_symbol_idx
    ON positions (organization_id, symbol, opened_at DESC);
CREATE INDEX IF NOT EXISTS positions_org_updated_idx
    ON positions (organization_id, updated_at);
CREATE INDEX IF NOT EXISTS positions_org_market_idx
    ON positions (organization_id, market_id) WHERE market_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS trades_org_ts_idx
    ON trades (organization_id, ts);
CREATE INDEX IF NOT EXISTS trades_org_position_idx
    ON trades (organization_id, position_id, ts);
CREATE INDEX IF NOT EXISTS trades_org_signature_idx
    ON trades (organization_id, signature) WHERE signature IS NOT NULL;

CREATE INDEX IF NOT EXISTS balance_snapshots_org_addr_ts_idx
    ON balance_snapshots (organization_id, address, ts DESC);
CREATE INDEX IF NOT EXISTS balance_snapshots_org_ts_idx
    ON balance_snapshots (organization_id, ts);

-- ── Dedup / risk / audit (0004) ────────────────────────────────────────
CREATE INDEX IF NOT EXISTS dedup_keys_org_expires_idx
    ON dedup_keys (organization_id, expires_at) WHERE expires_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS risk_events_org_ts_idx
    ON risk_events (organization_id, ts);
CREATE INDEX IF NOT EXISTS risk_events_org_module_ts_idx
    ON risk_events (organization_id, module, ts DESC);

CREATE INDEX IF NOT EXISTS audit_events_org_ts_idx
    ON audit_events (organization_id, ts);
CREATE INDEX IF NOT EXISTS audit_events_org_action_idx
    ON audit_events (organization_id, action, ts DESC);
CREATE INDEX IF NOT EXISTS audit_events_org_actor_idx
    ON audit_events (organization_id, actor, ts DESC);

-- ── Reconciliation queue (0005) ────────────────────────────────────────
-- Per-tenant due-work scans (the global (status, next_attempt_at) index
-- from 0005 keeps serving the deployment-global worker).
CREATE INDEX IF NOT EXISTS reconciliation_state_org_due_idx
    ON reconciliation_state (organization_id, status, next_attempt_at);

-- ── Intents / claims / lifecycle (0007, 0009, 0011, 0012) ─────────────
CREATE INDEX IF NOT EXISTS execution_intents_org_pending_idx
    ON execution_intents (organization_id, created_at) WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS execution_claims_org_owner_idx
    ON execution_claims (organization_id, owner_id, updated_at);

CREATE INDEX IF NOT EXISTS execution_claim_events_org_id_idx
    ON execution_claim_events (organization_id, execution_id, created_at);

CREATE INDEX IF NOT EXISTS execution_lifecycle_org_open_idx
    ON execution_lifecycle (organization_id, updated_at)
    WHERE state IN ('created', 'validated', 'submitted', 'pending');
CREATE INDEX IF NOT EXISTS execution_lifecycle_org_signature_idx
    ON execution_lifecycle (organization_id, signature) WHERE signature IS NOT NULL;

CREATE INDEX IF NOT EXISTS execution_lifecycle_events_org_intent_idx
    ON execution_lifecycle_events (organization_id, intent_id, ts);

-- ── Copy trading (0013) ────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS copy_leaders_org_status_idx
    ON copy_leaders (organization_id, status);
CREATE INDEX IF NOT EXISTS copy_leader_events_org_address_idx
    ON copy_leader_events (organization_id, address, ts);
CREATE INDEX IF NOT EXISTS copy_events_org_leader_idx
    ON copy_events (organization_id, leader, observed_at);
CREATE INDEX IF NOT EXISTS copy_events_org_signature_idx
    ON copy_events (organization_id, signature);
CREATE INDEX IF NOT EXISTS copy_events_org_observed_idx
    ON copy_events (organization_id, observed_at);
CREATE INDEX IF NOT EXISTS copy_links_org_leader_mint_open_idx
    ON copy_links (organization_id, leader, mint) WHERE status = 'open';

-- ── Polymarket (0014) ──────────────────────────────────────────────────
CREATE INDEX IF NOT EXISTS poly_signals_org_market_idx
    ON poly_signals (organization_id, condition_id, token_id);
CREATE INDEX IF NOT EXISTS poly_signals_org_updated_idx
    ON poly_signals (organization_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS poly_orders_org_open_idx
    ON poly_orders (organization_id, token_id) WHERE closed_at IS NULL;
CREATE INDEX IF NOT EXISTS poly_orders_org_order_idx
    ON poly_orders (organization_id, order_id);
CREATE INDEX IF NOT EXISTS poly_fills_org_order_idx
    ON poly_fills (organization_id, venue_order_id, ts);
CREATE INDEX IF NOT EXISTS poly_recon_findings_org_ts_idx
    ON poly_recon_findings (organization_id, ts DESC);

-- ── Global risk + accounting (0015) ────────────────────────────────────
CREATE INDEX IF NOT EXISTS ledger_events_org_ts_idx
    ON ledger_events (organization_id, ts, recorded_at);
CREATE INDEX IF NOT EXISTS ledger_events_org_reference_idx
    ON ledger_events (organization_id, reference_id);
CREATE INDEX IF NOT EXISTS ledger_events_org_position_idx
    ON ledger_events (organization_id, position_id) WHERE position_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ledger_events_org_trade_idx
    ON ledger_events (organization_id, trade_id) WHERE trade_id IS NOT NULL;
-- Per-tenant exposure aggregation (the leading columns of the 0015 scope
-- index, tenant-first).
CREATE INDEX IF NOT EXISTS ledger_events_org_scope_idx
    ON ledger_events (organization_id, module, venue, wallet);
CREATE INDEX IF NOT EXISTS ledger_postings_org_event_idx
    ON ledger_postings (organization_id, event_id);
CREATE INDEX IF NOT EXISTS ledger_postings_org_account_idx
    ON ledger_postings (organization_id, account, wallet, asset);
CREATE INDEX IF NOT EXISTS global_positions_org_open_idx
    ON global_positions (organization_id, module, venue) WHERE qty > 0;
CREATE INDEX IF NOT EXISTS global_risk_decisions_org_ts_idx
    ON global_risk_decisions (organization_id, ts DESC);
CREATE INDEX IF NOT EXISTS global_risk_decisions_org_verdict_idx
    ON global_risk_decisions (organization_id, verdict, ts DESC);
CREATE INDEX IF NOT EXISTS kill_switch_events_org_scope_idx
    ON kill_switch_events (organization_id, scope, ts DESC);
CREATE INDEX IF NOT EXISTS accounting_recon_findings_org_ts_idx
    ON accounting_recon_findings (organization_id, ts DESC);
CREATE INDEX IF NOT EXISTS accounting_recon_findings_org_kind_idx
    ON accounting_recon_findings (organization_id, kind, ts DESC);
CREATE INDEX IF NOT EXISTS accounting_recon_findings_org_finding_idx
    ON accounting_recon_findings (organization_id, finding_id, ts DESC);

-- ══════════════════════════════════════════════════════════════════════
-- Post-apply verification (informational; run manually to audit the
-- result of this migration):
--   * all 32 columns NOT NULL with the bridge default:
--       SELECT table_name, is_nullable, column_default
--       FROM   information_schema.columns
--       WHERE  column_name = 'organization_id'
--       AND    table_schema = 'public'
--       AND    table_name IN ( ...the 32 trading-truth tables... );
--   * zero rows left unmapped (must return 0 on every table):
--       SELECT count(*) FROM orders WHERE organization_id IS NULL;  -- etc.
--   * NO tenant-composite uniques were added (must return 0; they are
--     deferred to the STEP 10–14 atomic swap — see the CONSTRAINT
--     STRATEGY header for the concurrent-upsert hazard):
--       SELECT count(*) FROM pg_constraint
--       WHERE  conname LIKE '%\_tenant\_%\_key';
--   * the deployment organization is the backfill target:
--       SELECT id, slug, name, status FROM organizations
--       WHERE  lower(slug) = 'deployment';
--   * the default bridge resolves (must return the deployment org id):
--       SELECT public.deployment_organization_id();
--   * a legacy-style insert (no organization_id) is attributed:
--       INSERT INTO orders (id, module, side, symbol, venue, mode)
--       VALUES ('ord_bridge_check', 'system', 'other', 'X', 'none', 'paper')
--       RETURNING id, organization_id;   -- then DELETE the check row.
