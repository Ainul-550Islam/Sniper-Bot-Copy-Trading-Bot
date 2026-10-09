-- 0052_limit_orders_dca.sql — durable storage for the module-sniper
-- trading-execution features: client limit orders and scheduled DCA.
--
-- GAP-MAP v2 (P2). Numbering: 0044–0051 were consumed by earlier P0/P1/P2
-- batches (see their headers); `crates/module-sniper/src/limit_orders.rs`
-- explicitly reserved 0052 for these two tables in its module doc.
--
-- Scope. Until now module-sniper had NO durable trading state: an operator
-- restarting the process lost every open limit order and every DCA schedule.
-- These tables give that state a durable home, org-scoped like every other
-- tenant table. Both features already exist as pure logic in module-sniper
-- (`limit_orders.rs`, `dca.rs`) with in-memory stores for tests; this
-- migration is the durable store their Postgres-backed implementations plug
-- into. No money moves through these tables — they record INTENT (an order
-- to place, a budget to spend), never executed fills or ledger balances.
--
-- Design rules carried over from the models:
--   * every row is org-scoped; the server enforces the tenant_query gate;
--   * `limit_orders.expires_at` NULL = good-til-cancelled;
--   * `dca_schedules.budget_sol` is a HARD ceiling — the store refuses to
--     overspend it (mirrors InMemoryDcaStore::record_run);
--   * `dca_schedules.spent_sol` is persisted so a restart cannot re-spend;
--   * status columns are text + CHECK so a renamed variant fails loudly
--     instead of silently mapping to a wrong bucket.

-- ---------------------------------------------------------------------------
-- Limit orders
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS limit_orders (
    id             text PRIMARY KEY,
    -- Tenant scope. Every query MUST filter on this column.
    organization_id uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Mint to trade (base58, validated by the module before insert).
    mint           text NOT NULL,
    -- OrderSide: 'buy' | 'sell'.
    side           text NOT NULL CHECK (side IN ('buy', 'sell')),
    -- TriggerKind: 'at_or_below' | 'at_or_above'.
    trigger_kind   text NOT NULL CHECK (trigger_kind IN ('at_or_below', 'at_or_above')),
    -- Trigger price, SOL per whole token. Stored as numeric (not float) so a
    -- round-trip through Postgres is exact; the module converts to f64 at the
    -- boundary, where it already validates finiteness and positivity.
    price_sol      numeric(24, 9) NOT NULL CHECK (price_sol > 0),
    -- Buy: SOL to spend (>0). Sell: fraction of the open position, [0,1].
    amount         numeric(24, 9) NOT NULL CHECK (amount > 0),
    -- OrderStatus: 'active' | 'triggered' | 'cancelled' | 'expired'.
    status         text NOT NULL DEFAULT 'active'
                   CHECK (status IN ('active', 'triggered', 'cancelled', 'expired')),
    created_at     timestamptz NOT NULL DEFAULT now(),
    -- Optional expiry; NULL = good-til-cancelled.
    expires_at     timestamptz,
    -- When an order leaves 'active', record why and when (audit trail; the
    -- module's TriggeredOrder carries the fill context to the pipeline).
    resolved_at    timestamptz,
    CONSTRAINT limit_orders_sell_fraction CHECK (
        side <> 'sell' OR amount <= 1
    )
);

COMMENT ON TABLE limit_orders IS
    'Client limit orders for module-sniper. Records intent only — execution '
    'and fills live in the trading pipeline. Org-scoped; money never moves here.';

CREATE INDEX IF NOT EXISTS limit_orders_active_by_org_idx
    ON limit_orders (organization_id, trigger_kind, price_sol)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS limit_orders_org_created_idx
    ON limit_orders (organization_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- DCA schedules
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS dca_schedules (
    id                 text PRIMARY KEY,
    -- Tenant scope. Every query MUST filter on this column.
    organization_id    uuid NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    -- Mint being accumulated (base58, validated by the module before insert).
    mint               text NOT NULL,
    -- SOL spent per run (> 0).
    amount_per_run_sol numeric(24, 9) NOT NULL CHECK (amount_per_run_sol > 0),
    -- Seconds between runs (> 0). BIGINT so large intervals are legal.
    interval_secs      bigint NOT NULL CHECK (interval_secs > 0),
    -- TOTAL SOL budget across all runs. Hard ceiling — see module doc.
    -- Unlimited schedules are deliberately unsupported (operational hazard).
    budget_sol         numeric(24, 9) NOT NULL CHECK (budget_sol > 0),
    -- SOL spent so far. Persisted so a restart cannot re-spend. The store
    -- refuses to let spent_sol exceed budget_sol.
    spent_sol          numeric(24, 9) NOT NULL DEFAULT 0 CHECK (spent_sol >= 0),
    -- DcaStatus: 'active' | 'completed' | 'paused' | 'cancelled'.
    status             text NOT NULL DEFAULT 'active'
                       CHECK (status IN ('active', 'completed', 'paused', 'cancelled')),
    created_at         timestamptz NOT NULL DEFAULT now(),
    -- When the next run is due. Advanced by WHOLE intervals past now on each
    -- run (no catch-up bursts — see dca.rs).
    next_run_at        timestamptz NOT NULL,
    CONSTRAINT dca_budget_covers_one_run CHECK (budget_sol >= amount_per_run_sol),
    CONSTRAINT dca_spent_within_budget CHECK (spent_sol <= budget_sol)
);

COMMENT ON TABLE dca_schedules IS
    'Scheduled DCA into a mint for module-sniper. budget_sol is a hard '
    'ceiling; spent_sol is persisted for restart safety. Org-scoped; records '
    'budget intent only, never executed fills.';

CREATE INDEX IF NOT EXISTS dca_schedules_due_idx
    ON dca_schedules (next_run_at)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS dca_schedules_org_idx
    ON dca_schedules (organization_id, created_at DESC);
