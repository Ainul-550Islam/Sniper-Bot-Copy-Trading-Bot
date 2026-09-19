-- 0009_execution_claims.sql — Prompt 3 §T: durable distributed execution
-- ownership. One row per LOGICAL execution id (never deleted — the claim
-- history answers "which replica believed it owned this execution, when did
-- it acquire/lose it, who took over"). Postgres is the AUTHORITATIVE claim
-- store when [database] is enabled: acquisition is a single atomic
-- INSERT ... ON CONFLICT ... WHERE (expired lease | released | handoff grace
-- elapsed) — two replicas racing on the same id can never both win.
--
-- Fencing: claim_epoch is the fencing token. renew/verify/release are CAS on
-- (execution_id, owner_id, claim_epoch, status='claimed'); a replica whose
-- lease expired and was taken over holds a stale epoch and is rejected
-- BEFORE any money-moving continuation.
--
-- Additive and restart-safe: IF NOT EXISTS only; no rewrites of existing data.

CREATE TABLE IF NOT EXISTS execution_claims (
    execution_id    text        PRIMARY KEY,
    kind            text        NOT NULL DEFAULT '',
    module          text        NOT NULL DEFAULT '',
    strategy        text        NOT NULL DEFAULT '',
    symbol          text        NOT NULL DEFAULT '',
    owner_id        text        NOT NULL,
    claim_epoch     bigint      NOT NULL DEFAULT 1,
    -- 'claimed' | 'released' | 'handed_off' (exact state machine in
    -- crates/core/src/ownership.rs).
    status          text        NOT NULL DEFAULT 'claimed'
                    CHECK (status IN ('claimed', 'released', 'handed_off')),
    claimed_at      timestamptz NOT NULL DEFAULT now(),
    lease_until     timestamptz NOT NULL,
    last_heartbeat  timestamptz NOT NULL DEFAULT now(),
    takeover_count  bigint      NOT NULL DEFAULT 0,
    previous_owner  text,
    updated_at      timestamptz NOT NULL DEFAULT now()
);

-- Takeover scan / operator introspection: leases that lapsed.
CREATE INDEX IF NOT EXISTS execution_claims_lease_idx
    ON execution_claims (lease_until)
    WHERE status = 'claimed';

CREATE INDEX IF NOT EXISTS execution_claims_owner_idx
    ON execution_claims (owner_id, updated_at);
