-- Migration 0037: Commercial strategy library, backtests, and webhook delivery infrastructure.
-- Forward-only migration with tenant-scoped indexes.

CREATE TABLE IF NOT EXISTS tenant_strategies (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name VARCHAR(128) NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    module VARCHAR(32) NOT NULL,
    mode VARCHAR(16) NOT NULL DEFAULT 'paper',
    status VARCHAR(32) NOT NULL DEFAULT 'active',
    version INTEGER NOT NULL DEFAULT 1,
    config_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tenant_strategies_org_created
    ON tenant_strategies (organization_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_tenant_strategies_org_module
    ON tenant_strategies (organization_id, module, status);

CREATE TABLE IF NOT EXISTS backtest_runs (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    strategy_id UUID NOT NULL REFERENCES tenant_strategies(id) ON DELETE CASCADE,
    venue VARCHAR(32) NOT NULL,
    period_start TIMESTAMPTZ NOT NULL,
    period_end TIMESTAMPTZ NOT NULL,
    initial_balance_usd_cents BIGINT NOT NULL,
    fee_rate_bps INTEGER NOT NULL DEFAULT 25,
    slippage_bps INTEGER NOT NULL DEFAULT 50,
    status VARCHAR(32) NOT NULL DEFAULT 'queued',
    result_json JSONB,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_backtest_runs_org_created
    ON backtest_runs (organization_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_backtest_runs_strategy
    ON backtest_runs (strategy_id, created_at DESC);

CREATE TABLE IF NOT EXISTS webhook_endpoints (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    secret VARCHAR(128) NOT NULL,
    event_types TEXT[] NOT NULL DEFAULT '{}',
    status VARCHAR(32) NOT NULL DEFAULT 'active',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_webhook_endpoints_org
    ON webhook_endpoints (organization_id, status);

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    endpoint_id UUID NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
    event_id VARCHAR(64) NOT NULL,
    event_type VARCHAR(64) NOT NULL,
    status VARCHAR(32) NOT NULL DEFAULT 'pending',
    response_status INTEGER,
    attempt_count INTEGER NOT NULL DEFAULT 1,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_org_created
    ON webhook_deliveries (organization_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_endpoint
    ON webhook_deliveries (endpoint_id, created_at DESC);
