-- Optional provenance for new projections. Historical projection rows remain
-- unlinked; a link is not evidence of a fill, transfer, or cash settlement.
ALTER TABLE public.platform_fee_projections
    ADD COLUMN intent_id text;
ALTER TABLE public.platform_fee_projections
    ADD CONSTRAINT platform_fee_projection_frozen_policy_fk
    FOREIGN KEY (organization_id, intent_id)
    REFERENCES public.platform_fee_intent_policy (organization_id, intent_id);
CREATE INDEX platform_fee_projection_intent_idx
    ON public.platform_fee_projections (organization_id, intent_id)
    WHERE intent_id IS NOT NULL;
