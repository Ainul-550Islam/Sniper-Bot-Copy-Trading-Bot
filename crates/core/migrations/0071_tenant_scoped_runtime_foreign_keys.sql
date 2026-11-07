-- 0071_tenant_scoped_runtime_foreign_keys.sql — bind tenant-owned runtime
-- rows to parent rows in the same organization.
--
-- Migration 0054 stored organization_id beside dca_runs.schedule_id and
-- sso_pending_auth.config_id, but their foreign keys referenced only the
-- globally unique parent IDs. That allowed a row owned by tenant B to point
-- at tenant A's DCA schedule or SSO configuration. In DCA this also let a
-- mismatched run occupy the global (schedule_id, slot_at) idempotency key.
-- The application normally supplies matching IDs, but the database must
-- enforce the tenant relationship as well.
--
-- Parent IDs remain globally primary-keyed for compatibility. The redundant
-- composite unique constraints provide valid targets for the tenant-aware
-- foreign keys. Existing mismatched data causes this migration to fail closed
-- for operator investigation rather than being silently reassigned/deleted.

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'dca_schedules_org_id_key'
           AND conrelid = 'public.dca_schedules'::regclass
    ) THEN
        ALTER TABLE public.dca_schedules
            ADD CONSTRAINT dca_schedules_org_id_key UNIQUE (organization_id, id);
    END IF;
END
$$;

ALTER TABLE public.dca_runs
    DROP CONSTRAINT IF EXISTS dca_runs_schedule_id_fkey;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'dca_runs_schedule_tenant_fkey'
           AND conrelid = 'public.dca_runs'::regclass
    ) THEN
        ALTER TABLE public.dca_runs
            ADD CONSTRAINT dca_runs_schedule_tenant_fkey
            FOREIGN KEY (organization_id, schedule_id)
            REFERENCES public.dca_schedules (organization_id, id)
            ON DELETE CASCADE;
    END IF;
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'tenant_sso_configs_org_id_key'
           AND conrelid = 'public.tenant_sso_configs'::regclass
    ) THEN
        ALTER TABLE public.tenant_sso_configs
            ADD CONSTRAINT tenant_sso_configs_org_id_key UNIQUE (organization_id, id);
    END IF;
END
$$;

ALTER TABLE public.sso_pending_auth
    DROP CONSTRAINT IF EXISTS sso_pending_auth_config_id_fkey;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'sso_pending_auth_config_tenant_fkey'
           AND conrelid = 'public.sso_pending_auth'::regclass
    ) THEN
        ALTER TABLE public.sso_pending_auth
            ADD CONSTRAINT sso_pending_auth_config_tenant_fkey
            FOREIGN KEY (organization_id, config_id)
            REFERENCES public.tenant_sso_configs (organization_id, id)
            ON DELETE CASCADE;
    END IF;
END
$$;
