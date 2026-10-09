-- 0028_execution_claims_tenant_conflict.sql — PROMPT 3/10 (STEP 10 atomic
-- swap #3): execution-claim identity becomes TENANT-LOCAL while the
-- distributed claim RACE SEMANTICS are preserved exactly.
--
-- WHAT THIS MIGRATION DOES (one transaction):
--   1. drops `execution_claims_pkey` (PRIMARY KEY (execution_id), 0009);
--   2. creates PRIMARY KEY (organization_id, execution_id).
--
-- WHY (business identity):
--   `execution_id` is the LOGICAL execution identity (`snipe:<mint>`,
--   `copy:<wallet>:<mint>`, `poly:entry:<token_id>`, …). Those strings
--   are only globally unique while there is exactly one tenant (0023
--   header): two tenants sniping the same mint derive the SAME
--   `execution_id` and must hold INDEPENDENT claims — one owner per
--   (tenant, execution), never one owner globally. Identity is
--   tenant-local.
--
-- RACE SEMANTICS — PRESERVED, NOT SIMPLIFIED:
--   The acquisition statement in `PostgresClaimStore::claim`
--   (crates/core/src/db/claims.rs) keeps its exact shape:
--     WITH prev AS (…), ins AS (
--       INSERT … ON CONFLICT (…) DO UPDATE SET …
--       WHERE (released | lease expired | handoff grace elapsed)
--       RETURNING …)
--     SELECT ins.*, …
--   Only the arbiter changes: `ON CONFLICT (execution_id)` →
--   `ON CONFLICT (organization_id, execution_id)` with
--   organization_id bound (deployment writers via
--   public.deployment_organization_id(), tenant writers via the acting
--   tenant). Postgres still serialises conflicting upserts on the SAME
--   (organization_id, execution_id): two replicas of one tenant racing
--   the same execution still get exactly one winner — the
--   two_contexts_pg_claim_race_single_owner invariant, unchanged.
--   Cross-tenant rows never conflict (different arbiter values), which
--   is precisely the independence the enterprise product requires.
--
--   The 0024 hazard does NOT apply here: the composite is created in
--   the SAME transaction that drops the global PK, so there is never a
--   second unique index over the live arbiter's columns alongside it.
--
-- PRESERVED:
--   * `execution_claims_lease_idx` (takeover scan) and
--     `execution_claims_owner_idx` stay — operator scans remain global;
--   * `execution_claim_events` (0011) has NO foreign key to
--     execution_claims (audited) — the PK swap is dependency-free;
--   * rows are never deleted (auditability) — unchanged.
--
-- Forward-only; idempotent; no data rewrite (existing rows already
-- carry their backfilled organization_id and were unique under the
-- stricter global PK).

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'execution_claims_pkey'
          AND  conrelid = 'public.execution_claims'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.execution_claims DROP CONSTRAINT execution_claims_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.execution_claims
             ADD PRIMARY KEY (organization_id, execution_id)';
EXCEPTION
    WHEN duplicate_object THEN NULL; -- constraint already present (re-run)
END
$$;

-- Verification (operator): exactly one PK, composite.
--    SELECT conname, pg_get_constraintdef(oid) FROM pg_constraint
--    WHERE  conrelid = 'public.execution_claims'::regclass AND contype = 'p';
