-- 0030_polymarket_orders_tenant_conflict.sql — PROMPT 3/10 (STEP 10 atomic
-- swap #5): the Polymarket trading surfaces become TENANT-LOCAL.
--
-- WHAT THIS MIGRATION DOES (one transaction):
--   1. `poly_signals`: PRIMARY KEY (signal_id)      → (organization_id, signal_id);
--   2. `poly_orders`:  PRIMARY KEY (venue_order_id) → (organization_id, venue_order_id);
--   3. `poly_fills`:   PRIMARY KEY (fill_id)        → (organization_id, fill_id).
--
-- WHY (business identity):
--   * `poly_signals.signal_id` — the deterministic signal id
--     (`psig_` + digest of the intent identity). Two tenants running
--     the same strategy against the same market derive the SAME
--     signal id; each must journal its OWN decision row (one tenant
--     paper, the other live). Identity is tenant-local.
--   * `poly_orders.venue_order_id` — the venue order id (EIP-712
--     struct hash, or `paper:<digest>` for paper orders). The MARKET
--     is globally known, but a customer's order STATE is not globally
--     owned: paper orders of two tenants on the same market derive
--     identical `paper:` ids, and even for live venue ids the row's
--     owner is the tenant that placed it — one tenant must never
--     upsert-overwrite another tenant's order lifecycle through the
--     conflict path. Identity is tenant-local.
--   * `poly_fills.fill_id` — venue trade id or a deterministic digest
--     of (venue_order_id, cumulative matched size, source). Same
--     reasoning as poly_orders: paper fills of two tenants collide on
--     the digest. Identity is tenant-local.
--
-- WRITER COUPLING (same implementation batch):
--   * legacy deployment writer `PolyRepo::{upsert_signal, upsert_order,
--     record_fill}` (crates/core/src/db/polymarket.rs) now binds
--     `organization_id = public.deployment_organization_id()` and uses
--     the composite arbiters;
--   * the tenant-scoped repository `trading_repository::polymarket::*`
--     binds the acting tenant with the same arbiters.
--
-- PRESERVED:
--   * `poly_recon_findings` (bigserial PK, append-only) needs no swap;
--   * no foreign keys reference these three tables (audited);
--   * the OMS `orders` idempotency boundary stays authoritative (0026).
--
-- Forward-only; idempotent; no data rewrite.

-- ── 1. poly_signals: PK (signal_id) → (organization_id, signal_id) ──────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'poly_signals_pkey'
          AND  conrelid = 'public.poly_signals'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.poly_signals DROP CONSTRAINT poly_signals_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.poly_signals
             ADD PRIMARY KEY (organization_id, signal_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 2. poly_orders: PK (venue_order_id) → (organization_id, venue_order_id) ──
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'poly_orders_pkey'
          AND  conrelid = 'public.poly_orders'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.poly_orders DROP CONSTRAINT poly_orders_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.poly_orders
             ADD PRIMARY KEY (organization_id, venue_order_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 3. poly_fills: PK (fill_id) → (organization_id, fill_id) ────────────
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'poly_fills_pkey'
          AND  conrelid = 'public.poly_fills'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.poly_fills DROP CONSTRAINT poly_fills_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.poly_fills
             ADD PRIMARY KEY (organization_id, fill_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- Verification (operator): three composite PKs.
--    SELECT conrelid::regclass, conname, pg_get_constraintdef(oid)
--    FROM   pg_constraint
--    WHERE  conrelid IN ('public.poly_signals'::regclass,
--                        'public.poly_orders'::regclass,
--                        'public.poly_fills'::regclass)
--    AND    contype = 'p';
