-- 0038_authoritative_exact_accounting.sql — Authoritative exact integer & fixed-point accounting ledger.
-- Forward-only, idempotent migration: converts monetary, quantity, price, and fee fields
-- from legacy floating-point approximations to exact numeric(28, 8) and atomic integer units.

-- ── 1. Enhance positions table with exact financial columns ──────────
ALTER TABLE positions
    ADD COLUMN IF NOT EXISTS qty_atomic numeric(38, 0) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS qty_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS avg_entry_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS cost_basis_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS realized_quote_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS last_mark_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS stop_loss_exact numeric(28, 8),
    ADD COLUMN IF NOT EXISTS take_profit_exact numeric(28, 8),
    ADD COLUMN IF NOT EXISTS trailing_stop_exact numeric(28, 8),
    ADD COLUMN IF NOT EXISTS trailing_high_water_exact numeric(28, 8),
    ADD COLUMN IF NOT EXISTS fee_paid_exact numeric(28, 8) NOT NULL DEFAULT 0;

-- Backfill existing position records deterministically
UPDATE positions
SET qty_exact = round(qty::numeric, 8),
    avg_entry_exact = round(avg_entry::numeric, 8),
    cost_basis_exact = round(cost_basis::numeric, 8),
    realized_quote_exact = round(realized_quote::numeric, 8),
    last_mark_exact = round(last_mark::numeric, 8),
    stop_loss_exact = CASE WHEN stop_loss IS NOT NULL THEN round(stop_loss::numeric, 8) ELSE NULL END,
    take_profit_exact = CASE WHEN take_profit IS NOT NULL THEN round(take_profit::numeric, 8) ELSE NULL END,
    trailing_stop_exact = CASE WHEN trailing_stop IS NOT NULL THEN round(trailing_stop::numeric, 8) ELSE NULL END,
    trailing_high_water_exact = CASE WHEN trailing_high_water IS NOT NULL THEN round(trailing_high_water::numeric, 8) ELSE NULL END
WHERE qty_exact = 0 AND qty != 0;

-- ── 2. Enhance trades table with exact financial columns ──────────────
ALTER TABLE trades
    ADD COLUMN IF NOT EXISTS amount_in_atomic numeric(38, 0) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS amount_out_atomic numeric(38, 0) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS amount_in_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS amount_out_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS price_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS fee_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS fee_bps_exact integer NOT NULL DEFAULT 0;

-- Backfill existing trades deterministically
UPDATE trades
SET amount_in_exact = round(amount_in::numeric, 8),
    amount_out_exact = round(amount_out::numeric, 8),
    price_exact = round(price::numeric, 8),
    fee_exact = round(fee::numeric, 8),
    fee_bps_exact = CASE WHEN price > 0 AND amount_in > 0 THEN round((fee / (price * amount_in) * 10000)::numeric)::integer ELSE 0 END
WHERE price_exact = 0 AND price != 0;

-- ── 3. Enhance balance_snapshots table with exact columns ─────────────
ALTER TABLE balance_snapshots
    ADD COLUMN IF NOT EXISTS amount_atomic numeric(38, 0) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS amount_exact numeric(28, 8) NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS usd_value_exact numeric(28, 8);

-- Backfill balance snapshots deterministically
UPDATE balance_snapshots
SET amount_exact = round(amount::numeric, 8),
    usd_value_exact = CASE WHEN usd_value IS NOT NULL THEN round(usd_value::numeric, 8) ELSE NULL END
WHERE amount_exact = 0 AND amount != 0;

-- ── 4. Materialized hourly portfolio snapshots for exact read model ──
CREATE TABLE IF NOT EXISTS portfolio_snapshots_hourly (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    snapshot_at TIMESTAMPTZ NOT NULL,
    total_equity_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    total_exposure_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    realized_pnl_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    unrealized_pnl_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    open_positions_count integer NOT NULL DEFAULT 0,
    high_water_mark_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    drawdown_bps integer NOT NULL DEFAULT 0,
    status VARCHAR(32) NOT NULL DEFAULT 'finalized',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_portfolio_snapshots_org_time
    ON portfolio_snapshots_hourly (organization_id, snapshot_at);

CREATE INDEX IF NOT EXISTS idx_portfolio_snapshots_org_created
    ON portfolio_snapshots_hourly (organization_id, created_at DESC);

-- ── 5. Durable tenant daily accounting for risk caps & loss tracking ─
CREATE TABLE IF NOT EXISTS tenant_daily_accounting (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    trade_date DATE NOT NULL,
    realized_pnl_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    unrealized_pnl_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    total_volume_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    total_fees_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    peak_exposure_usd_exact numeric(28, 8) NOT NULL DEFAULT 0,
    trades_count integer NOT NULL DEFAULT 0,
    loss_cap_exceeded boolean NOT NULL DEFAULT false,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tenant_daily_accounting_org_date
    ON tenant_daily_accounting (organization_id, trade_date);

-- ── 6. Enterprise IAM: MFA challenges and SSO federation tables ────────
CREATE TABLE IF NOT EXISTS tenant_sso_configs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider_type VARCHAR(32) NOT NULL CHECK (provider_type IN ('saml2', 'oidc', 'google', 'okta', 'azure_ad')),
    issuer_url TEXT NOT NULL,
    client_id VARCHAR(256) NOT NULL,
    client_secret_encrypted TEXT,
    metadata_xml TEXT,
    enforce_sso boolean NOT NULL DEFAULT false,
    allowed_domains TEXT[] NOT NULL DEFAULT '{}',
    status VARCHAR(32) NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tenant_sso_org
    ON tenant_sso_configs (organization_id);

CREATE TABLE IF NOT EXISTS user_mfa_devices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    device_type VARCHAR(32) NOT NULL CHECK (device_type IN ('totp', 'webauthn', 'backup_code')),
    name VARCHAR(64) NOT NULL DEFAULT 'Primary Device',
    secret_encrypted TEXT NOT NULL,
    credential_id TEXT,
    public_key_pem TEXT,
    counter BIGINT NOT NULL DEFAULT 0,
    verified boolean NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_user_mfa_user
    ON user_mfa_devices (user_id, verified);

CREATE INDEX IF NOT EXISTS idx_user_mfa_org
    ON user_mfa_devices (organization_id);
