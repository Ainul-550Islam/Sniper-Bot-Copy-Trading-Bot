-- 0005_reconciliation.sql — reconciliation work queue and worker
-- checkpoints (resume points that survive restarts).

-- One row per entity that needs external-truth verification. Workers claim
-- rows whose next_attempt_at has passed, with exponential backoff recorded
-- in attempts/next_attempt_at. `subject` is the entity id (order id,
-- signature, polymarket order id, position id).
CREATE TABLE IF NOT EXISTS reconciliation_state (
    subject         text NOT NULL,
    kind            text NOT NULL CHECK (kind IN (
                        'order', 'position', 'transaction',
                        'polymarket_order', 'balance')),
    status          text NOT NULL DEFAULT 'pending' CHECK (status IN (
                        'pending', 'in_progress', 'resolved', 'failed')),
    attempts        integer NOT NULL DEFAULT 0,
    max_attempts    integer NOT NULL DEFAULT 10,
    last_error      text,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, subject)
);
-- The worker poll query: pending/in_progress rows that are due.
CREATE INDEX IF NOT EXISTS reconciliation_due_idx
    ON reconciliation_state (status, next_attempt_at);

-- Generic worker cursors (feed slots, journal offsets, sampler ticks).
CREATE TABLE IF NOT EXISTS recovery_checkpoints (
    worker     text PRIMARY KEY,
    position   jsonb NOT NULL DEFAULT '{}'::jsonb,
    updated_at timestamptz NOT NULL DEFAULT now()
);
