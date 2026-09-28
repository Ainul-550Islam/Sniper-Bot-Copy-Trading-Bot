-- 0007_intent_journal.sql — crash point C closure (§I/§F): a durable
-- write-ahead intent is recorded BEFORE a money-moving broadcast, so a crash
-- between broadcast and any other durable trace still leaves evidence.
--
-- Lifecycle: 'pending' (recorded pre-broadcast) → 'submitted' (linked to the
-- signature that left the process) or 'abandoned' (the attempt provably never
-- broadcast: terminal error / no signature). Orphaned 'pending' rows after a
-- restart are AMBIGUOUS by definition — startup reconciliation enqueues them
-- as 'intent' claims and blocks their symbol; they are never auto-resubmitted.
--
-- Additive and restart-safe: IF NOT EXISTS only, no rewrites of existing data.

CREATE TABLE IF NOT EXISTS execution_intents (
    intent_id  text        PRIMARY KEY,
    module     text        NOT NULL,
    symbol     text        NOT NULL DEFAULT '',
    wallet     text        NOT NULL DEFAULT '',
    side       text        NOT NULL DEFAULT '',
    qty        text        NOT NULL DEFAULT '',
    status     text        NOT NULL DEFAULT 'pending',
    signature  text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

-- Orphan scan on startup / maintenance: pending rows ordered by age.
CREATE INDEX IF NOT EXISTS execution_intents_pending_idx
    ON execution_intents (created_at)
    WHERE status = 'pending';
