-- 0014_polymarket_trading.sql — TASK 4 (Polymarket trading engine): durable
-- signal journal, CLOB order lifecycle, fills and reconciliation findings.
--
-- The generic OMS tables from 0002 (`orders`, `order_status_history`,
-- `executions`) remain the ONE authoritative order record and idempotency
-- boundary: every Polymarket order is an `orders` row whose
-- `idempotency_key` is the deterministic intent id. The tables below add the
-- venue-specific detail the OMS deliberately does not model.
--
--   poly_signals
--       One row per strategy decision the pipeline finished with (accepted
--       OR rejected), keyed by the deterministic signal id. Records the last
--       stage reached and the explicit reject reason, so "why did the bot
--       (not) trade" is answerable after the fact and restart recovery can
--       see which intents already reached the venue.
--
--   poly_orders
--       CLOB-level lifecycle of one order: the venue order id (the EIP-712
--       struct hash, derivable before the POST), the OMS order id it belongs
--       to, price/size/matched quantity and the venue status string. Upserted
--       on every observed transition; `submitted_at` keeps the first time the
--       order reached the venue. Restart recovery re-adopts every non-terminal
--       row and asks the venue for the truth.
--
--   poly_fills
--       One row per fill event (poll delta, user-channel trade or paper fill),
--       keyed by the venue trade id when there is one and by a deterministic
--       digest otherwise, so a replayed websocket event or a repeated poll
--       never double-books a fill.
--
--   poly_recon_findings
--       Append-only findings from local-vs-venue reconciliation (orphan venue
--       order, local order missing on the venue, matched-quantity mismatch,
--       ambiguous submit, position without order) with the action taken.
--
-- Additive and restart-safe: IF NOT EXISTS only, no rewrites of existing data.

CREATE TABLE IF NOT EXISTS poly_signals (
    signal_id       text        PRIMARY KEY,
    condition_id    text        NOT NULL,
    token_id        text        NOT NULL,
    outcome         text        NOT NULL DEFAULT '',
    side            text        NOT NULL CHECK (side IN ('buy', 'sell')),
    strategy        text        NOT NULL DEFAULT '',
    limit_price     double precision NOT NULL,
    size_tokens     double precision NOT NULL,
    stake_usd       double precision NOT NULL,
    mode            text        NOT NULL CHECK (mode IN ('paper', 'simulate', 'live')),
    -- Last pipeline stage reached (RECEIVED … FILLED | RESTING | AMBIGUOUS |
    -- REJECTED | FAILED).
    stage           text        NOT NULL,
    reject_reason   text,
    detail          text        NOT NULL DEFAULT '',
    -- OMS `orders.id` once the intent passed the idempotency gate.
    order_id        text,
    -- Venue order id (EIP-712 struct hash) once signed.
    venue_order_id  text,
    position_id     text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS poly_signals_market_idx
    ON poly_signals (condition_id, token_id);
CREATE INDEX IF NOT EXISTS poly_signals_updated_idx
    ON poly_signals (updated_at DESC);

CREATE TABLE IF NOT EXISTS poly_orders (
    -- Venue order id: `0x` + 64 hex (the EIP-712 struct hash). Paper orders
    -- carry a deterministic `paper:` id.
    venue_order_id  text        PRIMARY KEY,
    -- The OMS order (0002 `orders.id`) this venue order belongs to.
    order_id        text        NOT NULL,
    signal_id       text        NOT NULL DEFAULT '',
    condition_id    text        NOT NULL,
    token_id        text        NOT NULL,
    outcome         text        NOT NULL DEFAULT '',
    side            text        NOT NULL CHECK (side IN ('buy', 'sell')),
    order_type      text        NOT NULL DEFAULT 'GTC',
    limit_price     double precision NOT NULL,
    size_tokens     double precision NOT NULL,
    size_matched    double precision NOT NULL DEFAULT 0,
    mode            text        NOT NULL CHECK (mode IN ('paper', 'simulate', 'live')),
    -- Local lifecycle state: submitted | resting | partially_filled | filled
    -- | cancelled | expired | unknown | failed.
    state           text        NOT NULL,
    -- Raw venue status string last observed (live, matched, cancelled, …).
    venue_status    text        NOT NULL DEFAULT '',
    -- Unix seconds; 0 = GTC.
    expiration      bigint      NOT NULL DEFAULT 0,
    position_id     text,
    replica_id      text        NOT NULL DEFAULT '',
    submitted_at    timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    closed_at       timestamptz
);
CREATE INDEX IF NOT EXISTS poly_orders_open_idx
    ON poly_orders (token_id)
    WHERE closed_at IS NULL;
CREATE INDEX IF NOT EXISTS poly_orders_order_idx
    ON poly_orders (order_id);

CREATE TABLE IF NOT EXISTS poly_fills (
    -- Venue trade id when known, else a deterministic digest of
    -- (venue_order_id, cumulative matched size, source).
    fill_id         text        PRIMARY KEY,
    venue_order_id  text        NOT NULL,
    order_id        text        NOT NULL,
    token_id        text        NOT NULL,
    side            text        NOT NULL CHECK (side IN ('buy', 'sell')),
    price           double precision NOT NULL,
    size_tokens     double precision NOT NULL,
    quote_usd       double precision NOT NULL,
    -- poll | user_ws | paper | recon
    source          text        NOT NULL,
    position_id     text,
    ts              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS poly_fills_order_idx
    ON poly_fills (venue_order_id, ts);

CREATE TABLE IF NOT EXISTS poly_recon_findings (
    id              bigserial   PRIMARY KEY,
    kind            text        NOT NULL,
    venue_order_id  text,
    order_id        text,
    token_id        text,
    detail          text        NOT NULL DEFAULT '',
    action          text        NOT NULL DEFAULT 'reported',
    replica_id      text        NOT NULL DEFAULT '',
    ts              timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS poly_recon_findings_ts_idx
    ON poly_recon_findings (ts DESC);
