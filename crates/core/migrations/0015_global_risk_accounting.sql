-- 0015_global_risk_accounting.sql — TASK 5 (global risk + accounting /
-- ledger): the append-only double-entry ledger over every trading module,
-- the derived position snapshots, the global risk decision journal, the
-- venue / strategy kill switches and the accounting reconciliation findings.
--
-- Nothing here replaces an existing table. `orders` / `executions` (0002)
-- stay the order truth, `positions` / `trades` (0003) stay each module's
-- operational record, `poly_fills` (0014) stays the Polymarket fill journal.
-- The ledger is the ONE financial idempotency boundary ACROSS modules: every
-- fill / fee / settlement / deposit / withdrawal / transfer / funding /
-- correction becomes exactly one `ledger_events` row keyed by the
-- deterministic event id (a digest of kind + module + venue + wallet +
-- reference id), and its balanced postings live in `ledger_postings`.
--
--   ledger_events
--       One row per financial event. `INSERT … ON CONFLICT DO NOTHING` on the
--       event id is the durable half of the idempotency mechanism: a replayed
--       fill, a repeated settlement, a recovery re-submission or a duplicate
--       fee event never books twice, in this process, a later one or another
--       replica.
--
--   ledger_postings
--       The balanced double-entry lines of one event (per quote asset the
--       debits equal the credits): cash / inventory / fees / realized_pnl /
--       equity / funding / adjustments, per wallet.
--
--   global_positions
--       Derived snapshot of the aggregated book per
--       (module, venue, wallet, strategy, asset, quote asset, mode). Rebuilt
--       from `ledger_events` on every start — informational for operators and
--       dashboards, never an input to a decision.
--
--   global_risk_decisions
--       Every global risk decision (accept AND reject) with the exposure
--       snapshot it was taken against, so a verdict is reproducible later.
--
--   kill_switches / kill_switch_events
--       Current state and append-only history of the per-venue / per-strategy
--       kill switches an operator engaged at runtime (configuration-pinned
--       switches are not stored — the config file is their record).
--
--   accounting_recon_findings
--       Append-only accounting reconciliation findings (missing / duplicate
--       ledger entry, position / quantity / fee / PnL mismatch, orphan event,
--       unresolved financial event) across the four record layers — OMS
--       orders, module trades (fills), the ledger and the positions. Each row
--       names the subject it is about (`order_id` / `trade_id` / `event_id` /
--       `position_id`). The engine reports and never repairs.
--
-- Additive and restart-safe: IF NOT EXISTS only, no rewrites of existing data.

CREATE TABLE IF NOT EXISTS ledger_events (
    -- `led_` + 40 hex: digest of (kind, module, venue, wallet, reference_id).
    event_id            text        PRIMARY KEY,
    kind                text        NOT NULL CHECK (kind IN (
                            'fill', 'fee', 'settlement', 'deposit', 'withdrawal',
                            'transfer', 'funding_adjustment', 'correction')),
    module              text        NOT NULL,
    venue               text        NOT NULL,
    wallet              text        NOT NULL,
    strategy            text        NOT NULL DEFAULT '',
    asset               text        NOT NULL,
    quote_asset         text        NOT NULL,
    side                text        CHECK (side IS NULL OR side IN ('buy', 'sell')),
    quantity            double precision NOT NULL DEFAULT 0,
    price               double precision,
    quote_amount        double precision NOT NULL DEFAULT 0,
    fee                 double precision NOT NULL DEFAULT 0,
    mode                text        NOT NULL CHECK (mode IN ('paper', 'simulate', 'live')),
    -- Source-defined identity of the fact (tx signature, venue trade id,
    -- paper reference, deposit reference).
    reference_id        text        NOT NULL,
    -- OMS order id / intent id / claim id.
    correlation_id      text,
    -- Module position id (`positions.id`).
    position_id         text,
    -- Module trade id (`trades.id`).
    trade_id            text,
    counterparty_wallet text,
    detail              text        NOT NULL DEFAULT '',
    -- When the fact happened (venue / chain time when known).
    ts                  timestamptz NOT NULL,
    recorded_at         timestamptz NOT NULL DEFAULT now(),
    replica_id          text        NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS ledger_events_ts_idx
    ON ledger_events (ts, recorded_at);
CREATE INDEX IF NOT EXISTS ledger_events_reference_idx
    ON ledger_events (reference_id);
CREATE INDEX IF NOT EXISTS ledger_events_position_idx
    ON ledger_events (position_id) WHERE position_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ledger_events_trade_idx
    ON ledger_events (trade_id) WHERE trade_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS ledger_events_scope_idx
    ON ledger_events (module, venue, wallet, strategy, asset);

CREATE TABLE IF NOT EXISTS ledger_postings (
    id          bigserial   PRIMARY KEY,
    event_id    text        NOT NULL REFERENCES ledger_events(event_id) ON DELETE RESTRICT,
    seq         integer     NOT NULL,
    account     text        NOT NULL CHECK (account IN (
                    'cash', 'inventory', 'fees', 'realized_pnl', 'equity',
                    'funding', 'adjustments')),
    wallet      text        NOT NULL,
    asset       text        NOT NULL,
    side        text        NOT NULL CHECK (side IN ('debit', 'credit')),
    amount      double precision NOT NULL CHECK (amount >= 0),
    quantity    double precision NOT NULL DEFAULT 0,
    base_asset  text,
    UNIQUE (event_id, seq)
);
CREATE INDEX IF NOT EXISTS ledger_postings_account_idx
    ON ledger_postings (account, wallet, asset);

CREATE TABLE IF NOT EXISTS global_positions (
    -- `module|venue|wallet|strategy|asset|quote_asset|mode`.
    position_key    text        PRIMARY KEY,
    module          text        NOT NULL,
    venue           text        NOT NULL,
    wallet          text        NOT NULL,
    strategy        text        NOT NULL DEFAULT '',
    asset           text        NOT NULL,
    quote_asset     text        NOT NULL,
    mode            text        NOT NULL,
    qty             double precision NOT NULL DEFAULT 0,
    cost_basis      double precision NOT NULL DEFAULT 0,
    realized        double precision NOT NULL DEFAULT 0,
    fees            double precision NOT NULL DEFAULT 0,
    bought_quote    double precision NOT NULL DEFAULT 0,
    sold_quote      double precision NOT NULL DEFAULT 0,
    bought_qty      double precision NOT NULL DEFAULT 0,
    sold_qty        double precision NOT NULL DEFAULT 0,
    last_price      double precision NOT NULL DEFAULT 0,
    event_count     bigint      NOT NULL DEFAULT 0,
    last_event_id   text        NOT NULL DEFAULT '',
    position_ids    text[]      NOT NULL DEFAULT '{}',
    opened_at       timestamptz NOT NULL,
    updated_at      timestamptz NOT NULL
);
CREATE INDEX IF NOT EXISTS global_positions_open_idx
    ON global_positions (module, venue) WHERE qty > 0;

CREATE TABLE IF NOT EXISTS global_risk_decisions (
    decision_id     text        PRIMARY KEY,
    ts              timestamptz NOT NULL,
    module          text        NOT NULL,
    venue           text        NOT NULL,
    wallet          text        NOT NULL,
    strategy        text        NOT NULL DEFAULT '',
    asset           text        NOT NULL,
    quote_asset     text        NOT NULL,
    requested_quote double precision NOT NULL,
    mode            text        NOT NULL,
    verdict         text        NOT NULL CHECK (verdict IN ('accept', 'reject')),
    reason          text,
    detail          text        NOT NULL DEFAULT '',
    -- DecisionSnapshot as JSON (requested_ref, rate, exposures, open
    -- positions, realized today, drawdown, capital base).
    snapshot        jsonb       NOT NULL DEFAULT '{}'::jsonb,
    replica_id      text        NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS global_risk_decisions_ts_idx
    ON global_risk_decisions (ts DESC);
CREATE INDEX IF NOT EXISTS global_risk_decisions_reject_idx
    ON global_risk_decisions (verdict, reason, ts DESC);

CREATE TABLE IF NOT EXISTS kill_switches (
    -- `venue:<venue>` / `strategy:<label>`.
    scope       text        PRIMARY KEY,
    engaged     boolean     NOT NULL DEFAULT false,
    reason      text        NOT NULL DEFAULT '',
    actor       text        NOT NULL DEFAULT '',
    updated_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS kill_switch_events (
    id          bigserial   PRIMARY KEY,
    scope       text        NOT NULL,
    action      text        NOT NULL CHECK (action IN ('engage', 'release')),
    reason      text        NOT NULL DEFAULT '',
    actor       text        NOT NULL DEFAULT '',
    replica_id  text        NOT NULL DEFAULT '',
    ts          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS kill_switch_events_scope_idx
    ON kill_switch_events (scope, ts DESC);

CREATE TABLE IF NOT EXISTS accounting_recon_findings (
    id          bigserial   PRIMARY KEY,
    -- `acf_` + digest of the finding identity (kind + subject), NOT of the
    -- numbers: a persisting discrepancy is journaled once per process life.
    finding_id  text        NOT NULL,
    kind        text        NOT NULL CHECK (kind IN (
                    'missing_ledger_entry', 'duplicate_ledger_entry',
                    'position_mismatch', 'quantity_mismatch', 'fee_mismatch',
                    'pnl_mismatch', 'orphan_accounting_event',
                    'unresolved_financial_event')),
    module      text,
    venue       text,
    asset       text,
    position_id text,
    event_id    text,
    trade_id    text,
    -- OMS `orders.id` when the finding is about the intent layer (a filled
    -- order whose money never reached the ledger).
    order_id    text,
    expected    double precision NOT NULL DEFAULT 0,
    actual      double precision NOT NULL DEFAULT 0,
    detail      text        NOT NULL DEFAULT '',
    action      text        NOT NULL DEFAULT 'reported',
    replica_id  text        NOT NULL DEFAULT '',
    ts          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS accounting_recon_findings_ts_idx
    ON accounting_recon_findings (ts DESC);
CREATE INDEX IF NOT EXISTS accounting_recon_findings_kind_idx
    ON accounting_recon_findings (kind, ts DESC);
CREATE INDEX IF NOT EXISTS accounting_recon_findings_finding_idx
    ON accounting_recon_findings (finding_id, ts DESC);
