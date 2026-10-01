-- 0031_execution_intents_tenant_conflict.sql — PROMPT 3/10 (STEP 10 atomic
-- swap #6): the write-ahead intent journal and the execution-lifecycle
-- rows become TENANT-LOCAL.
--
-- WHAT THIS MIGRATION DOES (one transaction):
--   1. `execution_intents`:   PRIMARY KEY (intent_id) →
--                             (organization_id, intent_id);
--   2. `execution_lifecycle`: PRIMARY KEY (intent_id) →
--                             (organization_id, intent_id).
--
-- WHY (business identity):
--   * `execution_intents.intent_id` — the deterministic write-ahead
--     intent identity recorded BEFORE a money-moving broadcast. Like
--     the claim ids (0028), these strings (`snipe:<mint>`,
--     `copy:<wallet>:<mint>`, …) are only globally unique while there
--     is exactly one tenant: two tenants sniping the same mint derive
--     the SAME intent_id, and each must journal its own broadcast
--     evidence independently. A global PK would make the second
--     tenant's `record` a no-op (ON CONFLICT DO NOTHING) — its crash
--     evidence would silently not exist. Identity is tenant-local.
--   * `execution_lifecycle.intent_id` — the durable lifecycle row per
--     deterministic intent (0012). Same derivation, same reasoning:
--     each tenant's build→submit→confirm recovery state is its own.
--     Identity is tenant-local.
--
-- WRITER COUPLING (same implementation batch):
--   * legacy deployment writers `IntentRepo::record`
--     (crates/core/src/db/repo.rs) and `ExecutionRepo::upsert`
--     (crates/core/src/db/execution.rs) now bind
--     `organization_id = public.deployment_organization_id()` and use
--     the composite arbiters;
--   * the tenant-scoped repository `trading_repository::intent::*` and
--     `trading_repository::executions::lifecycle` bind the acting
--     tenant with the same arbiters.
--
-- PRESERVED:
--   * `execution_lifecycle_events` (bigserial PK, append-only) needs
--     no swap; its reads are tenant-scoped in the new repositories;
--   * no foreign keys reference these tables (audited);
--   * restart policy and orphan scan semantics are unchanged — only
--     the key that scopes them gains the tenant prefix.
--
-- Forward-only; idempotent; no data rewrite.

-- ── 1. execution_intents: PK (intent_id) → (organization_id, intent_id) ──
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'execution_intents_pkey'
          AND  conrelid = 'public.execution_intents'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.execution_intents DROP CONSTRAINT execution_intents_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.execution_intents
             ADD PRIMARY KEY (organization_id, intent_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- ── 2. execution_lifecycle: PK (intent_id) → (organization_id, intent_id) ──
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'execution_lifecycle_pkey'
          AND  conrelid = 'public.execution_lifecycle'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.execution_lifecycle DROP CONSTRAINT execution_lifecycle_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.execution_lifecycle
             ADD PRIMARY KEY (organization_id, intent_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

-- Verification (operator): two composite PKs.
--    SELECT conrelid::regclass, conname, pg_get_constraintdef(oid)
--    FROM   pg_constraint
--    WHERE  conrelid IN ('public.execution_intents'::regclass,
--                        'public.execution_lifecycle'::regclass)
--    AND    contype = 'p';
