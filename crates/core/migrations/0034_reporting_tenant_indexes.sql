-- 0034_reporting_tenant_indexes.sql — PROMPT 3/10: tenant-leading indexes
-- for the reporting/aggregation query paths the 0024 set does not cover.
--
-- AUDIT FIRST (per the prompt's migration rule — no manufactured indexes):
--   0024 already created the tenant-leading forms for the hot operational
--   paths: orders (org, status, updated_at) / (org, module, created_at) /
--   (org, symbol, created_at), positions (org, status[open|closing]) /
--   (org, symbol, opened_at) / (org, updated_at), trades (org, ts) /
--   (org, position_id, ts), executions (org, order_id, ts) / (org, ts),
--   poly_fills (org, venue_order_id, ts), ledger_events (org, ts) …
--
--   The NEW tenant-scoped reporting repositories (PROMPT 3/10 §I) run
--   three aggregation shapes that no existing index covers:
--
--   1. realized-PnL windows:
--        SELECT SUM(realized_quote - cost_basis) FROM positions
--        WHERE organization_id = $1 AND closed_at >= $2 AND closed_at < $3
--      → needs (organization_id, closed_at). The 0024 set indexes
--        updated_at and the open-status partial — neither serves a
--        closed_at range scan.
--
--   2. per-symbol volume / fee breakdowns:
--        SELECT symbol, SUM(amount_in), SUM(fee) FROM trades
--        WHERE organization_id = $1 AND ts >= $2 GROUP BY symbol
--      → needs (organization_id, symbol, ts). 0024 has (org, ts) only.
--
--   3. Polymarket fill metrics over time (independent of one venue
--      order):
--        SELECT SUM(quote_usd), SUM(size_tokens) FROM poly_fills
--        WHERE organization_id = $1 AND ts >= $2
--      → needs (organization_id, ts). 0024 has (org, venue_order_id,
--        ts) — an equality-first index that cannot serve a pure time
--        window without the venue_order_id prefix.
--
-- No uniqueness changes in this file — only these three covering
-- indexes. Forward-only; IF NOT EXISTS (re-applies cleanly).

-- 1. Realized-PnL windows (positions closed in a time range).
CREATE INDEX IF NOT EXISTS positions_org_closed_idx
    ON positions (organization_id, closed_at)
    WHERE closed_at IS NOT NULL;

-- 2. Per-symbol volume / fee reporting over time.
CREATE INDEX IF NOT EXISTS trades_org_symbol_ts_idx
    ON trades (organization_id, symbol, ts);

-- 3. Polymarket fill metrics over time.
CREATE INDEX IF NOT EXISTS poly_fills_org_ts_idx
    ON poly_fills (organization_id, ts);
