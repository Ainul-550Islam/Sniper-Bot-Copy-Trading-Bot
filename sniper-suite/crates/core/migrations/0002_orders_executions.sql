-- 0002_orders_executions.sql — the order management system: normalized order
-- lifecycle, per-order execution log, on-chain transaction tracking and the
-- generic idempotency-key table.

CREATE TABLE IF NOT EXISTS orders (
    -- Stable internal id (assigned by the app, e.g. "ord_<uuid>").
    id              text PRIMARY KEY,
    -- Deterministic key proving "this intent was already submitted"
    -- (e.g. sha256 of module + feed signature + wallet + side). UNIQUE:
    -- duplicate intents collapse instead of double-executing.
    idempotency_key text UNIQUE,
    module          text NOT NULL CHECK (module IN
                        ('sniper', 'copy', 'polymarket', 'contract', 'telegram', 'system')),
    strategy_id     uuid REFERENCES strategies(id) ON DELETE SET NULL,
    side            text NOT NULL CHECK (side IN ('buy', 'sell', 'stake', 'unstake', 'claim', 'other')),
    symbol          text NOT NULL,
    venue           text NOT NULL,
    mode            text NOT NULL CHECK (mode IN ('paper', 'simulate', 'live')),
    status          text NOT NULL DEFAULT 'created' CHECK (status IN (
                        'created', 'validated', 'queued', 'submitted', 'accepted',
                        'partially_filled', 'filled', 'failed', 'cancelled',
                        'expired', 'unknown', 'reconciled')),
    qty             double precision NOT NULL DEFAULT 0,
    price           double precision,
    -- Provider-side identifiers, filled in when known.
    external_id     text,
    signature       text,
    error           text,
    meta            jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    submitted_at    timestamptz,
    finished_at     timestamptz
);
-- Recovery scans non-terminal orders; ops looks orders up by chain id,
-- external id, module or time window.
CREATE INDEX IF NOT EXISTS orders_status_updated_idx ON orders (status, updated_at);
CREATE INDEX IF NOT EXISTS orders_signature_idx ON orders (signature) WHERE signature IS NOT NULL;
CREATE INDEX IF NOT EXISTS orders_external_id_idx ON orders (external_id) WHERE external_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS orders_module_created_idx ON orders (module, created_at DESC);
CREATE INDEX IF NOT EXISTS orders_symbol_idx ON orders (symbol, created_at DESC);

CREATE TABLE IF NOT EXISTS order_status_history (
    id          bigserial PRIMARY KEY,
    order_id    text NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    from_status text,
    to_status   text NOT NULL,
    reason      text,
    ts          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS order_status_history_order_idx ON order_status_history (order_id, ts);

-- One row per attempt/observation against an order: send, simulate,
-- confirm poll outcome, cancel, reconciliation verdict.
CREATE TABLE IF NOT EXISTS executions (
    id         bigserial PRIMARY KEY,
    order_id   text NOT NULL REFERENCES orders(id) ON DELETE CASCADE,
    ts         timestamptz NOT NULL DEFAULT now(),
    kind       text NOT NULL CHECK (kind IN (
                   'validate', 'simulate', 'send', 'confirm', 'cancel',
                   'reconcile', 'recover', 'note')),
    endpoint   text,
    latency_ms bigint,
    ok         boolean NOT NULL,
    detail     text
);
CREATE INDEX IF NOT EXISTS executions_order_idx ON executions (order_id, ts);
CREATE INDEX IF NOT EXISTS executions_ts_idx ON executions (ts);

-- On-chain transaction bookkeeping, independent of orders so non-trading
-- transactions (governance, maintenance) can be tracked too.
CREATE TABLE IF NOT EXISTS transactions (
    signature    text PRIMARY KEY,
    chain        text NOT NULL DEFAULT 'solana',
    order_id     text REFERENCES orders(id) ON DELETE SET NULL,
    slot         bigint,
    status       text NOT NULL DEFAULT 'submitted' CHECK (status IN (
                     'submitted', 'confirmed', 'finalized', 'failed', 'not_found')),
    landed       boolean NOT NULL DEFAULT false,
    error        text,
    submitted_at timestamptz NOT NULL DEFAULT now(),
    confirmed_at timestamptz
);
CREATE INDEX IF NOT EXISTS transactions_order_idx ON transactions (order_id);
CREATE INDEX IF NOT EXISTS transactions_status_idx ON transactions (status, submitted_at);

-- Generic idempotency for non-order operations (webhook processing,
-- telegram commands, recovery actions). Response replay is optional.
CREATE TABLE IF NOT EXISTS idempotency_keys (
    key         text NOT NULL,
    scope       text NOT NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    consumed_at timestamptz,
    response    jsonb,
    PRIMARY KEY (scope, key)
);
CREATE INDEX IF NOT EXISTS idempotency_keys_created_idx ON idempotency_keys (created_at);
