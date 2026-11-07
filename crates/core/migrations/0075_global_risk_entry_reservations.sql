-- Cross-replica leases for accepted but not-yet-projected entry risk.
-- The deployment organization scopes the operator plane; tenant_id scopes
-- tenant-runtime reservations without mixing tenants.
CREATE TABLE IF NOT EXISTS public.global_risk_entry_reservations (
    organization_id uuid NOT NULL REFERENCES public.organizations(id) ON DELETE CASCADE,
    reservation_id text NOT NULL,
    tenant_id uuid REFERENCES public.organizations(id) ON DELETE CASCADE,
    module text NOT NULL,
    venue text NOT NULL,
    wallet text NOT NULL,
    strategy text NOT NULL,
    asset text NOT NULL,
    quote_asset text NOT NULL,
    requested_quote double precision NOT NULL
        CHECK (requested_quote > 0 AND requested_quote < 'Infinity'::double precision),
    requested_ref double precision NOT NULL
        CHECK (requested_ref >= 0 AND requested_ref < 'Infinity'::double precision),
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (organization_id, reservation_id)
);

CREATE INDEX IF NOT EXISTS global_risk_entry_reservations_active_idx
    ON public.global_risk_entry_reservations (organization_id, expires_at);
CREATE INDEX IF NOT EXISTS global_risk_entry_reservations_tenant_idx
    ON public.global_risk_entry_reservations (organization_id, tenant_id, expires_at);

-- A short database lease serializes the read/check/insert critical section
-- without holding a pooled connection while risk projections are queried.
CREATE TABLE IF NOT EXISTS public.global_risk_entry_admission_locks (
    organization_id uuid PRIMARY KEY REFERENCES public.organizations(id) ON DELETE CASCADE,
    holder_id uuid,
    lease_until timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE public.global_risk_entry_reservations ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.global_risk_entry_reservations FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_org_isolation ON public.global_risk_entry_reservations;
CREATE POLICY tenant_org_isolation ON public.global_risk_entry_reservations
    USING (organization_id = app.current_org_id())
    WITH CHECK (organization_id = app.current_org_id());

ALTER TABLE public.global_risk_entry_admission_locks ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.global_risk_entry_admission_locks FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_org_isolation ON public.global_risk_entry_admission_locks;
CREATE POLICY tenant_org_isolation ON public.global_risk_entry_admission_locks
    USING (organization_id = app.current_org_id())
    WITH CHECK (organization_id = app.current_org_id());
