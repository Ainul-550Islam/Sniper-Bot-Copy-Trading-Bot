-- 0027_idempotency_tenant_conflict.sql — PROMPT 3/10 (STEP 10 atomic swap #2):
-- the two generic idempotency surfaces become TENANT-LOCAL.
--
-- WHAT THIS MIGRATION DOES (one transaction):
--   1. `idempotency_keys`: PRIMARY KEY (scope, key)  →  PRIMARY KEY
--      (organization_id, scope, key);
--   2. `dedup_keys`:        PRIMARY KEY (namespace, key)  →  PRIMARY KEY
--      (organization_id, namespace, key).
--
-- WHY (business identity):
--   * `idempotency_keys (scope, key)` is the idempotency reservation for
--     non-order operations (webhooks, telegram commands, recovery
--     actions). `key` is caller-supplied: two tenants issuing the same
--     command key MUST NOT collapse into one — the second tenant's
--     operation would be silently swallowed as a "duplicate" of the
--     first tenant's. Identity is tenant-local.
--   * `dedup_keys (namespace, key)` is the durable L2 dedup window.
--     A leader signature watched by TWO tenants must be processed once
--     PER TENANT (each tenant mirrors the trade with its own sizing);
--     a global (namespace, key) would dedup the second tenant's
--     processing away entirely. Identity is tenant-local.
--
-- WRITER COUPLING (same implementation batch):
--   * legacy deployment writers `IdempotencyRepo::try_consume` /
--     `record_response` and `DedupRepo::mark` / `forget` / `exists`
--     (crates/core/src/db/repo.rs) now bind
--     `organization_id = public.deployment_organization_id()` and use
--     the composite arbiters;
--   * the tenant-scoped repositories
--     `trading_repository::executions::idempotency` and the copy/poly
--     event writers bind the acting tenant with the same arbiters.
--
-- WHY NOT EARLIER: see 0026 — the composite is created in the SAME
-- transaction that drops the global constraint, together with the
-- writer whose ON CONFLICT names it.
--
-- PRESERVED:
--   * `idempotency_keys_created_idx`, `dedup_keys_expires_idx` stay
--     (retention / TTL scans are global maintenance);
--   * no FK references these tables (audited: 0001–0025 contain no
--     REFERENCES to idempotency_keys / dedup_keys), so the PK swap is
--     dependency-free;
--   * existing rows: already organization-attributed (0024) and unique
--     under the stricter global key, hence unique under the composite.
--
-- Forward-only; idempotent; no data rewrite.

-- ── 1. idempotency_keys: PK (scope, key) → (organization_id, scope, key) ──
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'idempotency_keys_pkey'
          AND  conrelid = 'public.idempotency_keys'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.idempotency_keys DROP CONSTRAINT idempotency_keys_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.idempotency_keys
             ADD PRIMARY KEY (organization_id, scope, key)';
EXCEPTION
    WHEN duplicate_object THEN NULL; -- constraint already present (re-run)
END
$$;

-- ── 2. dedup_keys: PK (namespace, key) → (organization_id, namespace, key) ──
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE  conname = 'dedup_keys_pkey'
          AND  conrelid = 'public.dedup_keys'::regclass
    ) THEN
        EXECUTE 'ALTER TABLE public.dedup_keys DROP CONSTRAINT dedup_keys_pkey';
    END IF;
    EXECUTE 'ALTER TABLE public.dedup_keys
             ADD PRIMARY KEY (organization_id, namespace, key)';
EXCEPTION
    WHEN duplicate_object THEN NULL; -- constraint already present (re-run)
END
$$;

-- 3. Verification (operator): both composites exist, old PKs gone.
--    SELECT conname, pg_get_constraintdef(oid) FROM pg_constraint
--    WHERE  conrelid IN ('public.idempotency_keys'::regclass,
--                        'public.dedup_keys'::regclass)
--    AND    contype = 'p';
