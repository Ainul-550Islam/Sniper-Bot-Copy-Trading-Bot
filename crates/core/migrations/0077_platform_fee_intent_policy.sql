-- Immutable per-tenant intent fee policies. This is a pre-execution policy
-- snapshot, not a fee charge, ledger posting, or proof of settlement.
CREATE TABLE public.platform_fee_intent_policy (
    organization_id uuid NOT NULL REFERENCES public.organizations(id) ON DELETE CASCADE,
    intent_id text NOT NULL CHECK (length(btrim(intent_id)) > 0),
    surface text NOT NULL CHECK (surface IN ('sniper', 'copy', 'telegram_swap', 'polymarket')),
    mode text NOT NULL CHECK (mode IN ('paper', 'simulate', 'live')),
    asset text NOT NULL CHECK (length(btrim(asset)) > 0),
    fee_bps integer NOT NULL CHECK (fee_bps BETWEEN 0 AND 500),
    cap_micros bigint CHECK (cap_micros IS NULL OR cap_micros >= 0),
    minimum_micros bigint NOT NULL CHECK (minimum_micros >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, intent_id),
    CHECK (cap_micros IS NULL OR minimum_micros <= cap_micros)
);
ALTER TABLE public.platform_fee_intent_policy ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.platform_fee_intent_policy FORCE ROW LEVEL SECURITY;
CREATE POLICY tenant_org_isolation ON public.platform_fee_intent_policy
    USING (organization_id = app.current_org_id())
    WITH CHECK (organization_id = app.current_org_id());
REVOKE UPDATE, DELETE ON public.platform_fee_intent_policy FROM bot_app;
GRANT SELECT, INSERT ON public.platform_fee_intent_policy TO bot_app;
GRANT SELECT, INSERT ON public.platform_fee_intent_policy TO bot_system;
