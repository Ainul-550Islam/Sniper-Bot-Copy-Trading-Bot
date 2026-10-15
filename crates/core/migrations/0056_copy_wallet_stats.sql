-- 0056_copy_wallet_stats.sql — per-leader statistics for copy-trading.
--
-- Additive only. Rows are computed from observed trades; they are never
-- hand-entered. The primary key scopes each figure to one tenant, one wallet
-- and one window, so a re-computation is an UPSERT.
--
-- ROLLBACK (manual): DROP TABLE IF EXISTS copy_wallet_stats;

CREATE TABLE IF NOT EXISTS copy_wallet_stats (
    organization_id  uuid NOT NULL,
    wallet           text NOT NULL,
    window_days      integer NOT NULL,
    trades           integer NOT NULL DEFAULT 0,
    wins             integer NOT NULL DEFAULT 0,
    pnl_lamports     bigint NOT NULL DEFAULT 0,
    avg_hold_secs    integer,
    last_seen_at     timestamptz,
    computed_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (organization_id, wallet, window_days)
);
