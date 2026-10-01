-- 0026_orders_tenant_conflict.sql — PROMPT 3/10 (STEP 10 atomic swap #1):
-- order idempotency becomes TENANT-LOCAL.
--
-- WHAT THIS MIGRATION DOES (one transaction, per the STEP 2 rule):
--   1. drops the legacy GLOBAL UNIQUE on `orders.idempotency_key`
--      (0002: `idempotency_key text UNIQUE`, constraint
--      `orders_idempotency_key_key`);
--   2. creates the tenant-composite UNIQUE
--      `(organization_id, idempotency_key)`.
--
-- WHY (business identity):
--   `idempotency_key` is CLIENT-SUPPLIED (deterministic digest of the
--   intent). While there was exactly one deployment tenant a global
--   unique was correct; with multiple organizations the same logical
--   intent digest MUST be expressible by two different tenants (both
--   sniping the same mint, both placing the same strategy key) without
--   one replay-collapsing the other's order. Identity is tenant-local.
--   The column stays nullable: orders legitimately placed without a
--   deterministic key keep that (multiple NULLs remain allowed).
--
-- WRITER COUPLING (same implementation batch — no interval where schema
-- and writer disagree):
--   * legacy deployment writer `OrderRepo::insert_if_absent`
--     (crates/core/src/db/repo.rs) now binds
--     `organization_id = public.deployment_organization_id()` and uses
--     `ON CONFLICT (organization_id, idempotency_key)`;
--   * the tenant-scoped repository
--     `trading_repository::orders::write` binds the acting tenant and
--     uses the same composite arbiter.
--
-- WHY NOT EARLIER (0024 hazard note): adding the composite while the
-- global unique still existed would have turned the designed
-- concurrent-upsert grace into raw unique-violation errors for the
-- losing upserter. The swap (drop + add in ONE transaction) removes
-- that window; migrations run inside the new binary's startup together
-- with the new writer.
--
-- Global identities PRESERVED (unchanged, by design):
--   * `orders.id` stays PRIMARY KEY (app-assigned `ord_<uuid>` —
--     globally unique by construction, 0023's documented rule);
--   * `transactions.signature` stays globally unique (chain signature);
--   * every existing FK and index is preserved; the dropped constraint
--     is superseded by the composite above (same guarantee per tenant,
--     plus independence across tenants).
--
-- Forward-only; idempotent (guard on pg_constraint); no data rewrite:
-- every existing row already carries its (backfilled) organization_id
-- and was unique on idempotency_key globally, hence is unique per
-- (organization_id, idempotency_key) now.

-- 1. Retire the legacy global unique (0002).
ALTER TABLE orders DROP CONSTRAINT IF EXISTS orders_idempotency_key_key;

-- 2. Tenant-composite unique, guarded so the file re-applies cleanly.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'orders_tenant_idempotency_key'
          AND  conrelid = 'public.orders'::regclass
    ) THEN
        ALTER TABLE public.orders
            ADD CONSTRAINT orders_tenant_idempotency_key
            UNIQUE (organization_id, idempotency_key);
    END IF;
END
$$;

-- 3. Verification (operator): the arbiter the writers name.
--    SELECT conname, pg_get_constraintdef(oid)
--    FROM   pg_constraint
--    WHERE  conrelid = 'public.orders'::regclass
--    AND    contype = 'u';
