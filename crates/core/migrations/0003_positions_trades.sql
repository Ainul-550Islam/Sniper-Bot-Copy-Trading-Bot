-- 0003_positions_trades.sql — durable position and trade ledger plus
-- periodic balance snapshots (reconciliation inputs).
-- Columns mirror bot_core::models::{Position, Trade} exactly.

CREATE TABLE IF NOT EXISTS positions (
    id                    text PRIMARY KEY,
    source                text NOT NULL CHECK (source IN ('sniper', 'copy', 'polymarket', 'manual', 'risk')),
    venue                 text NOT NULL,
    mode                  text NOT NULL,
    status                text NOT NULL DEFAULT 'open' CHECK (status IN
                              ('open', 'closing', 'closed', 'stopped_out', 'failed')),
    symbol                text NOT NULL,
    symbol_display        text NOT NULL DEFAULT '',
    quote_symbol          text NOT NULL DEFAULT '',
    qty                   double precision NOT NULL DEFAULT 0,
    avg_entry             double precision NOT NULL DEFAULT 0,
    cost_basis            double precision NOT NULL DEFAULT 0,
    realized_quote        double precision NOT NULL DEFAULT 0,
    last_mark             double precision NOT NULL DEFAULT 0,
    stop_loss             double precision,
    take_profit           double precision,
    trailing_stop         double precision,
    trailing_high_water   double precision,
    max_hold_secs         bigint,
    entry_signature       text,
    exit_signature        text,
    entry_latency_ms      bigint,
    copied_wallet         text,
    market_id             text,
    outcome               text,
    reason_closed         text,
    opened_at             timestamptz NOT NULL,
    updated_at            timestamptz NOT NULL DEFAULT now(),
    closed_at             timestamptz
);
-- Restart recovery loads live positions; ops queries by symbol/source.
CREATE INDEX IF NOT EXISTS positions_status_idx ON positions (status) WHERE status IN ('open', 'closing');
CREATE INDEX IF NOT EXISTS positions_symbol_idx ON positions (symbol, opened_at DESC);
CREATE INDEX IF NOT EXISTS positions_updated_idx ON positions (updated_at);
CREATE INDEX IF NOT EXISTS positions_market_idx ON positions (market_id) WHERE market_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS trades (
    id             text PRIMARY KEY,
    ts             timestamptz NOT NULL DEFAULT now(),
    source         text NOT NULL,
    venue          text NOT NULL,
    mode           text NOT NULL,
    side           text NOT NULL CHECK (side IN ('long', 'short')),
    symbol         text NOT NULL,
    symbol_display text NOT NULL DEFAULT '',
    amount_in      double precision NOT NULL DEFAULT 0,
    amount_out     double precision NOT NULL DEFAULT 0,
    quote_symbol   text NOT NULL DEFAULT '',
    price          double precision NOT NULL DEFAULT 0,
    fee            double precision NOT NULL DEFAULT 0,
    slippage_bps   bigint NOT NULL DEFAULT 0,
    signature      text,
    position_id    text REFERENCES positions(id) ON DELETE SET NULL,
    note           text,
    latency_ms     bigint
);
CREATE INDEX IF NOT EXISTS trades_position_idx ON trades (position_id, ts);
CREATE INDEX IF NOT EXISTS trades_ts_idx ON trades (ts);
CREATE INDEX IF NOT EXISTS trades_signature_idx ON trades (signature) WHERE signature IS NOT NULL;

-- Periodic truth snapshots used by the balance reconciler.
CREATE TABLE IF NOT EXISTS balance_snapshots (
    id         bigserial PRIMARY KEY,
    ts         timestamptz NOT NULL DEFAULT now(),
    chain      text NOT NULL DEFAULT 'solana',
    address    text NOT NULL,
    asset      text NOT NULL,
    amount     double precision NOT NULL,
    usd_value  double precision,
    source     text NOT NULL DEFAULT 'rpc'
);
CREATE INDEX IF NOT EXISTS balance_snapshots_addr_ts_idx ON balance_snapshots (address, ts DESC);
CREATE INDEX IF NOT EXISTS balance_snapshots_ts_idx ON balance_snapshots (ts);
