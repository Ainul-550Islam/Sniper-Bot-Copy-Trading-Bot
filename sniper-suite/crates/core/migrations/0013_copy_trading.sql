-- 0013_copy_trading.sql — TASK 3 (copy-trading engine): durable leader
-- registry, processed leader-trade events and leader ↔ follower links.
--
-- Three concerns, three tables (+ one append-only history):
--
--   copy_leaders / copy_leader_events
--       The leaders (tracked wallets) the engine follows, their lifecycle
--       state (active | paused | removed) and running counters. Config is the
--       seed; the row survives restarts so a paused leader stays paused and
--       counters are not lost. `copy_leader_events` is the append-only
--       transition history (followed → paused → resumed → unfollowed, rule
--       changes) with the replica that made the change.
--
--   copy_events
--       One row per leader-trade event the pipeline finished with (accepted
--       or rejected), keyed by the deterministic event id. This is the
--       durable "already processed" record the restart recovery re-seeds the
--       dedup facade from, and the forensic trail that links a leader's
--       signature to our intent and position. Upserted on the final stage;
--       `created_at` keeps the first-seen time.
--
--   copy_links
--       Follower position ↔ the leader entry it mirrors. Reconciliation
--       compares the leader's observed activity with what we hold through
--       these rows: an open link whose leader has exited, a follower position
--       without a link (orphan), or a quantity mismatch.
--
-- Additive and restart-safe: IF NOT EXISTS only, no rewrites of existing data.

CREATE TABLE IF NOT EXISTS copy_leaders (
    address       text        PRIMARY KEY,
    label         text        NOT NULL DEFAULT '',
    status        text        NOT NULL CHECK (status IN ('active', 'paused', 'removed')),
    source        text        NOT NULL DEFAULT 'config',
    followed_at   timestamptz NOT NULL DEFAULT now(),
    status_since  timestamptz NOT NULL DEFAULT now(),
    events_seen   bigint      NOT NULL DEFAULT 0,
    mirrored      bigint      NOT NULL DEFAULT 0,
    rejected      bigint      NOT NULL DEFAULT 0,
    last_event_at timestamptz,
    last_slot     bigint,
    updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS copy_leader_events (
    id         bigserial   PRIMARY KEY,
    address    text        NOT NULL,
    event      text        NOT NULL,
    reason     text,
    replica_id text        NOT NULL DEFAULT '',
    ts         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS copy_leader_events_address_idx
    ON copy_leader_events (address, ts);

CREATE TABLE IF NOT EXISTS copy_events (
    event_id        text             PRIMARY KEY,
    leader          text             NOT NULL,
    signature       text             NOT NULL,
    slot            bigint           NOT NULL DEFAULT 0,
    mint            text             NOT NULL,
    side            text             NOT NULL CHECK (side IN ('buy', 'sell')),
    venue           text             NOT NULL DEFAULT '',
    token_amount    double precision NOT NULL DEFAULT 0,
    sol_amount      double precision NOT NULL DEFAULT 0,
    source          text             NOT NULL DEFAULT '',
    source_sequence bigint           NOT NULL DEFAULT 0,
    event_at        timestamptz,
    observed_at     timestamptz      NOT NULL,
    stage           text             NOT NULL,
    reject_reason   text,
    detail          text,
    intent_id       text,
    position_id     text,
    created_at      timestamptz      NOT NULL DEFAULT now(),
    updated_at      timestamptz      NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS copy_events_leader_idx
    ON copy_events (leader, observed_at);
CREATE INDEX IF NOT EXISTS copy_events_signature_idx
    ON copy_events (signature);
CREATE INDEX IF NOT EXISTS copy_events_observed_idx
    ON copy_events (observed_at);

CREATE TABLE IF NOT EXISTS copy_links (
    position_id         text             PRIMARY KEY,
    leader              text             NOT NULL,
    mint                text             NOT NULL,
    entry_event_id      text             NOT NULL,
    entry_signature     text             NOT NULL,
    intent_id           text,
    leader_token_amount double precision NOT NULL DEFAULT 0,
    follower_qty        double precision NOT NULL DEFAULT 0,
    status              text             NOT NULL CHECK (status IN ('open', 'closed', 'orphaned', 'mismatch')),
    opened_at           timestamptz      NOT NULL DEFAULT now(),
    closed_at           timestamptz,
    exit_event_id       text,
    last_reconciled_at  timestamptz,
    note                text,
    updated_at          timestamptz      NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS copy_links_leader_mint_open_idx
    ON copy_links (leader, mint)
    WHERE status = 'open';
