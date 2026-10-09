-- 0049_referrals_platform_fees.sql — referral attribution and the per-trade
-- platform-fee book.
--
-- GAP-MAP v2 (P2). Numbering: the GAP MAP listed these as 0045; 0044–0048
-- were taken by P0/P1 work, and migration 0048's header explicitly reserved
-- 0049 for this file and 0050 for wallet pools (the demo-tenant flag moved
-- to 0051 to honour that reservation).
--
-- Two independent concerns live here because the GAP MAP groups them:
--
-- 1. PLATFORM FEES. Until `crates/core/src/billing/platform_fee.rs` (P1) the
--    product billed ONLY subscriptions — no per-trade revenue existed. That
--    module computes and journals fees in-memory; this migration gives the
--    operator side durable storage:
--      * `platform_fee_policies` — the durable per-plan override of the
--        compiled-in `DEFAULT_FEE_BPS` schedule (platform_fee.rs reads the
--        compiled default when no row exists);
--      * `platform_fee_ledger`   — one row per assessed charge, columns
--        matched field-for-field to `PlatformFeeCharge` so journaling is a
--        straight projection. `charge_id` is the idempotency key: the module
--        guarantees "the same id never posts twice", enforced here with a
--        UNIQUE constraint so a replayed worker can never double-journal.
--    Money never moves through these tables: venue transfers are separate,
--    and the ledger records the operator's SERVICE fee only (see the module
--    docs). All amounts are integer micro-units — no numeric/float columns.
--
-- 2. REFERRALS. Codes belong to an organization (the referrer); attribution
--    links a referred organization/user to the code exactly once. No payout
--    math lives in the schema — attribution is the durable fact; payout
--    policy is application logic (and future ledger entries) so it can change
--    without a migration.

-- ---------------------------------------------------------------------------
-- Platform fee policies (durable override of the compiled-in defaults)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS platform_fee_policies (
    -- One row per plan; `plan` uses the same codes as `plans.code`
    -- ('starter', 'pro', 'business', 'enterprise') but is kept free-text +
    -- CHECK so a renamed plan fails loudly here instead of silently falling
    -- back to the compiled default.
    plan            text PRIMARY KEY CHECK (plan IN (
                        'starter', 'pro', 'business', 'enterprise'
                    )),
    -- Basis points of trade notional. The application layer enforces the
    -- 500 bps hard cap (platform_fee.rs::MAX_PLATFORM_FEE_BPS); the DB check
    -- is defence in depth and must never exceed it.
    fee_bps         integer NOT NULL CHECK (fee_bps BETWEEN 0 AND 500),
    -- Absolute per-trade cap in micro-units of the quote currency;
    -- NULL = uncapped (mirrors Option<i64> in PlatformFeePolicy).
    cap_micros      bigint CHECK (cap_micros IS NULL OR cap_micros >= 0),
    -- Minimum charge per trade in micro-units; 0 = no minimum. Must not
    -- exceed the cap when a cap is set (PlatformFeePolicy::new rejects it
    -- too — the DB check keeps hand-edited rows honest).
    minimum_micros  bigint NOT NULL DEFAULT 0 CHECK (minimum_micros >= 0),
    CHECK (cap_micros IS NULL OR minimum_micros <= cap_micros),
    updated_by      uuid REFERENCES users(id) ON DELETE SET NULL,
    updated_at      timestamptz NOT NULL DEFAULT now()
);

COMMENT ON TABLE platform_fee_policies IS
    'Durable per-plan platform-fee overrides. Absent row = compiled default '
    'from crates/core/src/billing/platform_fee.rs (DEFAULT_FEE_BPS).';

-- ---------------------------------------------------------------------------
-- Platform fee ledger (the operator's per-trade revenue journal)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS platform_fee_ledger (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Idempotency key from PlatformFeeCharge::charge_id. A replayed journal
    -- attempt hits the UNIQUE constraint instead of double-posting.
    charge_id        text NOT NULL,
    -- Tenant the fee was assessed to. Every query in this codebase must be
    -- organization-scoped (tenant_query gate), hence NOT NULL + index.
    organization_id  uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Execution venue of the underlying trade ('pumpfun', 'pumpswap',
    -- 'raydium', 'polymarket', ...). Free text: venues are an open set.
    venue            text NOT NULL,
    -- Quote asset the fee is denominated in ('SOL', 'USDC', ...).
    asset            text NOT NULL,
    -- Trade notional and assessed fee in micro-units (1 unit = 1e6 micros).
    -- Integers end-to-end: the fee pipeline never stores floats.
    notional_micros  bigint NOT NULL CHECK (notional_micros >= 0),
    fee_micros       bigint NOT NULL CHECK (fee_micros >= 0),
    -- The exact policy that produced the fee (PlatformFeePolicy as JSON),
    -- kept so a later policy change never rewrites history.
    policy           jsonb NOT NULL,
    -- Tenant-side book the fee was charged against, and the operator book
    -- that recognises the revenue (both are ledger wallet labels, not keys).
    tenant_wallet    text NOT NULL,
    platform_wallet  text NOT NULL,
    -- assessed -> posted | reversed. `assessed` = journaled; `posted` =
    -- reflected in the accounting ledger; `reversed` = corrected entry with
    -- a companion positive row (the ledger is append-only, never mutated).
    status           text NOT NULL DEFAULT 'assessed' CHECK (status IN (
                         'assessed', 'posted', 'reversed'
                     )),
    -- Set when the charge is reversed; references the original charge_id.
    reverses_charge  text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    posted_at        timestamptz,
    CONSTRAINT platform_fee_ledger_charge_id_unique UNIQUE (charge_id)
);

CREATE INDEX IF NOT EXISTS platform_fee_ledger_org_idx
    ON platform_fee_ledger (organization_id, created_at);
CREATE INDEX IF NOT EXISTS platform_fee_ledger_status_idx
    ON platform_fee_ledger (status) WHERE status = 'assessed';

COMMENT ON TABLE platform_fee_ledger IS
    'Append-only journal of operator service fees per trade (P2). Source of '
    'truth for per-trade revenue; subscription revenue stays in the billing '
    'tables. Amounts are integer micro-units of `asset`.';

-- ---------------------------------------------------------------------------
-- Referrals
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS referral_codes (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    -- Stored upper-case; lookups upper-case the input. The CHECK rejects
    -- anything outside a conservative code alphabet at the DB boundary too.
    code             text NOT NULL CHECK (code ~ '^[A-Z0-9_-]{4,32}$'),
    -- The organization that OWNS the code and receives attribution.
    organization_id  uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    created_by       uuid REFERENCES users(id) ON DELETE SET NULL,
    status           text NOT NULL DEFAULT 'active' CHECK (status IN (
                         'active', 'disabled'
                     )),
    disabled_at      timestamptz,
    created_at       timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT referral_codes_code_unique UNIQUE (code)
);

CREATE INDEX IF NOT EXISTS referral_codes_org_idx
    ON referral_codes (organization_id) WHERE status = 'active';

CREATE TABLE IF NOT EXISTS referral_attributions (
    id                     uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    referral_code_id       uuid NOT NULL REFERENCES referral_codes(id) ON DELETE CASCADE,
    -- The organization that signed up through the code. One attribution per
    -- referred org, forever: re-signup cannot re-attribute (anti-churn).
    referred_organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- The user who actually redeemed the code, when known.
    referred_user_id       uuid REFERENCES users(id) ON DELETE SET NULL,
    attributed_at          timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT referral_attributions_referred_org_unique
        UNIQUE (referred_organization_id)
);

CREATE INDEX IF NOT EXISTS referral_attributions_code_idx
    ON referral_attributions (referral_code_id);

COMMENT ON TABLE referral_attributions IS
    'Durable fact: which referred organization came from which code. Payout '
    'policy is deliberately NOT encoded here — it is application logic over '
    'this table plus the billing/fee ledgers.';
