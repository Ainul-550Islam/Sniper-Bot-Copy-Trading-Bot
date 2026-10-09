-- 0051_demo_tenant_flag.sql — flag demo tenants so operators can query and
-- exclude them.
--
-- GAP-MAP v2 (P1, deploy): the demo stack (`deploy/compose/docker-compose.demo.yml`
-- + `deploy/seed-demo-tenant.sh`) provisions a throwaway tenant for buyer
-- evaluations. Operators need a durable, queryable marker for such tenants —
-- for billing reports, support queues, and cleanup jobs — that survives
-- independent of naming conventions.
--
-- Design note (deliberate): this flag lives ONLY in SQL. The SaaS document
-- store is the authoritative source for `Organization` records and the
-- Postgres projection (`saas/postgres.rs`) upserts an explicit column list
-- that does NOT include `is_demo`, so projection replays can never clobber a
-- flag set here. The seed script sets it right after provisioning; nothing in
-- the application ever clears it — removal of demo tenants is an operator
-- action.
--
-- The control-plane demo banner keys off the seeded slug (`demo`) because the
-- flag is intentionally not part of the wire model (keeping it out of the
-- document store avoids the projection-clobber hazard above).

ALTER TABLE organizations
    ADD COLUMN IF NOT EXISTS is_demo boolean NOT NULL DEFAULT false;

COMMENT ON COLUMN organizations.is_demo IS
    'True for demo/evaluation tenants seeded by deploy/seed-demo-tenant.sh. '
    'Operational flag only — excluded from the SaaS wire model by design.';

CREATE INDEX IF NOT EXISTS organizations_is_demo_idx
    ON organizations (is_demo) WHERE is_demo;
