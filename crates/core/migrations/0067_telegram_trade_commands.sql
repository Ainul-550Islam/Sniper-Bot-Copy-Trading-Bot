-- 0067_telegram_trade_commands.sql — BATCH 04.
--
-- Two tables:
--
--   telegram_trade_commands
--       One row per `/buy` or `/sell` request that passed validation and was
--       turned into a pending command. The row is the durable half of the
--       nonce handshake: it stores `nonce_sha256` only (32 bytes, never the
--       nonce the chat saw), and it is bound to (chat_id, user_id, params) so a
--       leaked nonce is useless from anywhere else.
--
--   telegram_trade_audit
--       Append-only trail: exactly one row per state transition
--       (requested / denied / confirmed / executed / failed / expired /
--       cancelled / revoked). Never carries a nonce, a bot token or key
--       material — `detail` is a whitelisted JSON object built by
--       `commands::audit::AuditDetail`.
--
-- Invariants enforced here (the Rust state machine re-checks all of them):
--   * `status` is one of the seven known states;
--   * a buy carries an amount and no percent; a sell carries a percent and no
--     amount — checked by the `action_shape` constraint;
--   * `percent_bps` is in (0, 10000];
--   * `slippage_bps` is in [1, 500];
--   * `expires_at > created_at`;
--   * at most ONE pending command per (organization, chat) — the partial unique
--     index. A chat can only ever have one nonce outstanding, which is what
--     makes `/cancel` (which takes no argument in the single-pending case)
--     unambiguous;
--   * `idempotency_key` is UNIQUE and always equals `command_id`, so a duplicate
--     `/confirm` cannot insert a second command or execute twice.
--
-- ROLLBACK (manual):
--   DROP TABLE IF EXISTS public.telegram_trade_audit;
--   DROP TABLE IF EXISTS public.telegram_trade_commands;

CREATE TABLE IF NOT EXISTS public.telegram_trade_commands (
    command_id          uuid        PRIMARY KEY,
    organization_id     uuid        NOT NULL,
    chat_id             bigint      NOT NULL,
    user_id             bigint      NOT NULL,
    consent_id          uuid        REFERENCES public.telegram_trade_consents (consent_id)
                                    ON DELETE RESTRICT,
    action              text        NOT NULL,
    mint                text        NOT NULL,
    amount_lamports     bigint,
    percent_bps         integer,
    slippage_bps        smallint    NOT NULL,
    nonce_sha256        bytea       NOT NULL,
    -- The port idempotency key. Always equal to `command_id` (the Rust side
    -- sets it in `TradeCommand::open`), but stored explicitly so the executing
    -- side can deduplicate on a column it owns, and UNIQUE so a duplicate
    -- `/confirm` cannot insert a second row for the same order.
    idempotency_key     uuid        NOT NULL UNIQUE,
    status              text        NOT NULL DEFAULT 'pending',
    failure_reason      text,
    execution_ref       text,
    created_at          timestamptz NOT NULL DEFAULT now(),
    expires_at          timestamptz NOT NULL,
    confirmed_at        timestamptz,
    settled_at          timestamptz,
    CONSTRAINT telegram_trade_commands_action_known
        CHECK (action IN ('buy', 'sell')),
    CONSTRAINT telegram_trade_commands_status_known
        CHECK (status IN ('pending', 'confirmed', 'executed', 'failed', 'expired',
                          'cancelled', 'denied')),
    CONSTRAINT telegram_trade_commands_action_shape
        CHECK (
            (action = 'buy' AND amount_lamports IS NOT NULL AND amount_lamports > 0
                            AND percent_bps IS NULL)
            OR
            (action = 'sell' AND percent_bps IS NOT NULL AND percent_bps > 0
                             AND percent_bps <= 10000
                             AND amount_lamports IS NULL)
        ),
    CONSTRAINT telegram_trade_commands_mint_shape
        CHECK (length(mint) BETWEEN 32 AND 44 AND mint ~ '^[1-9A-HJ-NP-Za-km-z]+$'),
    CONSTRAINT telegram_trade_commands_slippage_range
        CHECK (slippage_bps BETWEEN 1 AND 500),
    CONSTRAINT telegram_trade_commands_nonce_digest_len
        CHECK (octet_length(nonce_sha256) = 32),
    CONSTRAINT telegram_trade_commands_expiry_after_creation
        CHECK (expires_at > created_at)
);

-- At most one live pending command per organization + chat.
CREATE UNIQUE INDEX IF NOT EXISTS telegram_trade_commands_one_pending
    ON public.telegram_trade_commands (organization_id, chat_id)
    WHERE status = 'pending';

-- `/pending` and `/cancel` lookups.
CREATE INDEX IF NOT EXISTS telegram_trade_commands_chat_recent
    ON public.telegram_trade_commands (organization_id, chat_id, created_at DESC);

-- Rolling 24h spend query: only executed buys contribute.
CREATE INDEX IF NOT EXISTS telegram_trade_commands_spent
    ON public.telegram_trade_commands (organization_id, confirmed_at)
    WHERE status = 'executed' AND amount_lamports IS NOT NULL;

-- Expiry sweeper.
CREATE INDEX IF NOT EXISTS telegram_trade_commands_expiry
    ON public.telegram_trade_commands (expires_at)
    WHERE status = 'pending';

CREATE TABLE IF NOT EXISTS public.telegram_trade_audit (
    id              bigserial   PRIMARY KEY,
    organization_id uuid        NOT NULL,
    chat_id         bigint      NOT NULL,
    user_id         bigint,
    command_id      uuid,
    consent_id      uuid,
    event           text        NOT NULL,
    detail          jsonb       NOT NULL DEFAULT '{}'::jsonb,
    ts              timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT telegram_trade_audit_event_known
        CHECK (event IN (
            'requested', 'denied', 'confirmed', 'executed',
            'failed', 'expired', 'cancelled', 'revoked',
            'consent_granted'
        )),
    CONSTRAINT telegram_trade_audit_detail_is_object
        CHECK (jsonb_typeof(detail) = 'object')
);

CREATE INDEX IF NOT EXISTS telegram_trade_audit_org_ts
    ON public.telegram_trade_audit (organization_id, ts DESC);
CREATE INDEX IF NOT EXISTS telegram_trade_audit_command
    ON public.telegram_trade_audit (command_id)
    WHERE command_id IS NOT NULL;

-- Tenant isolation. Migration 0062 installs the policy loop, but it ran before
-- these tables existed, so the policy is installed here — guarded so the
-- migration still applies on a database without the Batch 01 RLS foundation.
DO $$
BEGIN
    IF to_regprocedure('app.current_org_id()') IS NOT NULL THEN
        ALTER TABLE public.telegram_trade_commands ENABLE ROW LEVEL SECURITY;
        ALTER TABLE public.telegram_trade_commands FORCE ROW LEVEL SECURITY;
        DROP POLICY IF EXISTS tenant_org_isolation ON public.telegram_trade_commands;
        CREATE POLICY tenant_org_isolation ON public.telegram_trade_commands
            USING (organization_id = app.current_org_id())
            WITH CHECK (organization_id = app.current_org_id());

        ALTER TABLE public.telegram_trade_audit ENABLE ROW LEVEL SECURITY;
        ALTER TABLE public.telegram_trade_audit FORCE ROW LEVEL SECURITY;
        DROP POLICY IF EXISTS tenant_org_isolation ON public.telegram_trade_audit;
        CREATE POLICY tenant_org_isolation ON public.telegram_trade_audit
            USING (organization_id = app.current_org_id())
            WITH CHECK (organization_id = app.current_org_id());
    END IF;
END $$;

-- Runtime grants, guarded on role existence (the roles are created by 0060).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_app') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.telegram_trade_commands TO bot_app;
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.telegram_trade_audit TO bot_app;
        GRANT USAGE, SELECT ON SEQUENCE public.telegram_trade_audit_id_seq TO bot_app;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bot_system') THEN
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.telegram_trade_commands TO bot_system;
        GRANT SELECT, INSERT, UPDATE, DELETE ON public.telegram_trade_audit TO bot_system;
        GRANT USAGE, SELECT ON SEQUENCE public.telegram_trade_audit_id_seq TO bot_system;
    END IF;
END $$;
