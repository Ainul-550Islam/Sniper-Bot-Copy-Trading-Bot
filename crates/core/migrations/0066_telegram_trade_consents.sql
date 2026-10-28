-- 0066_telegram_trade_consents.sql — BATCH 04.
--
-- One row per granted Telegram trading consent. A consent is a capability: it
-- carries the caps and the expiry that bound every order issued under it, and
-- it can only ever TIGHTEN the organization's own risk limits.
--
-- Invariants enforced here (defence in depth — the Rust side re-checks them):
--   * `scope` is a non-empty subset of ('buy','sell');
--   * `max_order_lamports > 0`;
--   * `daily_cap_lamports >= max_order_lamports`;
--   * `expires_at > granted_at`;
--   * at most ONE non-revoked consent per (organization, chat) — the partial
--     unique index. Expiry is a policy question (an expired row is not
--     usable), so the index keys on `revoked_at IS NULL`: re-granting after a
--     lapse revokes the predecessor first, which keeps the "one live row"
--     shape unambiguous for both readers and the constraint.
--   * only SHA-256 of the confirmation phrase is stored (never the phrase).
--
-- ROLLBACK (manual): DROP TABLE IF EXISTS telegram_trade_consents;

CREATE TABLE IF NOT EXISTS public.telegram_trade_consents (
    consent_id          uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL,
    user_id             bigint      NOT NULL,
    chat_id             bigint      NOT NULL,
    scope               text[]      NOT NULL,
    max_order_lamports  bigint      NOT NULL,
    daily_cap_lamports  bigint      NOT NULL,
    granted_at          timestamptz NOT NULL DEFAULT now(),
    expires_at          timestamptz NOT NULL,
    revoked_at          timestamptz,
    phrase_sha256       bytea       NOT NULL,
    CONSTRAINT telegram_trade_consents_scope_subset
        CHECK (cardinality(scope) > 0 AND scope <@ ARRAY['buy', 'sell']::text[]),
    CONSTRAINT telegram_trade_consents_order_cap_positive
        CHECK (max_order_lamports > 0),
    CONSTRAINT telegram_trade_consents_daily_covers_order
        CHECK (daily_cap_lamports >= max_order_lamports),
    CONSTRAINT telegram_trade_consents_expiry_after_grant
        CHECK (expires_at > granted_at),
    CONSTRAINT telegram_trade_consents_phrase_digest_len
        CHECK (octet_length(phrase_sha256) = 32)
);

-- At most one live (non-revoked) consent per organization + chat.
CREATE UNIQUE INDEX IF NOT EXISTS telegram_trade_consents_one_active
    ON public.telegram_trade_consents (organization_id, chat_id)
    WHERE revoked_at IS NULL;

-- Lookup path used by every command: newest consent for a chat.
CREATE INDEX IF NOT EXISTS telegram_trade_consents_chat_lookup
    ON public.telegram_trade_consents (organization_id, chat_id, granted_at DESC);

-- Revocation / expiry sweeps.
CREATE INDEX IF NOT EXISTS telegram_trade_consents_expiry
    ON public.telegram_trade_consents (expires_at)
    WHERE revoked_at IS NULL;

-- Tenant isolation. Migration 0062 installs the RLS policy loop, but it runs
-- BEFORE this table exists, so the policy is installed here — guarded, so the
-- migration still applies on a database without the Batch 01 RLS foundation.
DO $$
BEGIN
    IF to_regprocedure('app.current_org_id()') IS NOT NULL THEN
        ALTER TABLE public.telegram_trade_consents ENABLE ROW LEVEL SECURITY;
        ALTER TABLE public.telegram_trade_consents FORCE ROW LEVEL SECURITY;
        DROP POLICY IF EXISTS tenant_org_isolation ON public.telegram_trade_consents;
        CREATE POLICY tenant_org_isolation ON public.telegram_trade_consents
            USING (organization_id = app.current_org_id())
            WITH CHECK (organization_id = app.current_org_id());
    END IF;
END $$;

-- Runtime grants, guarded on role existence (the roles are created by 0060).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_app') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.telegram_trade_consents TO bot_app;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_system') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.telegram_trade_consents TO bot_system;
    END IF;
END $$;
