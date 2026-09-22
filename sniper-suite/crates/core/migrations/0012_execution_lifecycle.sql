-- 0012_execution_lifecycle.sql — TASK 1 (execution reliability layer): durable
-- execution lifecycle rows so a crash at ANY point of build → simulate →
-- submit → confirm leaves enough state to recover deterministically.
--
-- One row per deterministic intent id (`bot_core::execution::intent_id`);
-- the row is upserted on every lifecycle transition and `Submitted` is
-- written BEFORE the broadcast call (write-ahead, like execution_intents).
-- `execution_lifecycle_events` is the append-only transition history that
-- backs audit/forensics; the application exposes no update/delete path.
--
-- Restart policy (see ExecutionLedger::resolve_after_restart):
--   created/validated  → failed   (the send never happened — nothing left)
--   submitted/pending  → ambiguous: kept, signature enqueued for
--                        reconciliation, duplicates stay blocked
--   confirmed/failed/expired/reconciled → settled, informational only
--
-- Additive and restart-safe: IF NOT EXISTS only, no rewrites of existing data.

CREATE TABLE IF NOT EXISTS execution_lifecycle (
    intent_id                   text        PRIMARY KEY,
    module                      text        NOT NULL DEFAULT '',
    label                       text        NOT NULL DEFAULT '',
    wallet                      text        NOT NULL DEFAULT '',
    symbol                      text        NOT NULL DEFAULT '',
    state                       text        NOT NULL CHECK (state IN (
                                    'created', 'validated', 'submitted', 'pending',
                                    'confirmed', 'failed', 'expired', 'reconciled')),
    attempts                    integer     NOT NULL DEFAULT 1,
    signature                   text,
    blockhash                   text,
    last_valid_block_height     bigint,
    priority_fee_micro_lamports bigint      NOT NULL DEFAULT 0,
    failure_class               text,
    error                       text,
    created_at                  timestamptz NOT NULL DEFAULT now(),
    updated_at                  timestamptz NOT NULL DEFAULT now()
);

-- Startup hydration + dashboards: open attempts ordered by age.
CREATE INDEX IF NOT EXISTS execution_lifecycle_open_idx
    ON execution_lifecycle (updated_at)
    WHERE state IN ('created', 'validated', 'submitted', 'pending');
-- Reconciliation joins by signature (transactions.signature).
CREATE INDEX IF NOT EXISTS execution_lifecycle_signature_idx
    ON execution_lifecycle (signature)
    WHERE signature IS NOT NULL;

CREATE TABLE IF NOT EXISTS execution_lifecycle_events (
    id            bigserial   PRIMARY KEY,
    intent_id     text        NOT NULL,
    attempt       integer     NOT NULL DEFAULT 1,
    from_state    text,
    to_state      text        NOT NULL,
    signature     text,
    failure_class text,
    reason        text,
    ts            timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS execution_lifecycle_events_intent_idx
    ON execution_lifecycle_events (intent_id, ts);
CREATE INDEX IF NOT EXISTS execution_lifecycle_events_ts_idx
    ON execution_lifecycle_events (ts);
