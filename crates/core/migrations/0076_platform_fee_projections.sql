-- Immutable tenant-scoped fee projections. One row atomically records the
-- assessed charge and both non-cash/cash projections. This table is not
-- proof of on-chain collection or an operator cash ledger.
CREATE TABLE IF NOT EXISTS public.platform_fee_projections (
    organization_id uuid NOT NULL REFERENCES public.organizations(id) ON DELETE CASCADE,
    charge_id text NOT NULL,
    surface text NOT NULL CHECK (surface IN ('sniper','copy','telegram_swap','polymarket')),
    mode text NOT NULL CHECK (mode IN ('paper','simulate','live')),
    asset text NOT NULL,
    fee_micros bigint NOT NULL CHECK (fee_micros >= 0),
    charge jsonb NOT NULL,
    tenant_entry jsonb NOT NULL,
    platform_projection jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, charge_id)
);
CREATE INDEX IF NOT EXISTS platform_fee_projections_surface_idx
    ON public.platform_fee_projections (organization_id, surface, mode, asset);
ALTER TABLE public.platform_fee_projections ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.platform_fee_projections FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_org_isolation ON public.platform_fee_projections;
CREATE POLICY tenant_org_isolation ON public.platform_fee_projections
    USING (organization_id = app.current_org_id())
    WITH CHECK (organization_id = app.current_org_id());
REVOKE UPDATE, DELETE ON public.platform_fee_projections FROM bot_app;
GRANT SELECT, INSERT ON public.platform_fee_projections TO bot_app;
GRANT SELECT, INSERT ON public.platform_fee_projections TO bot_system;
