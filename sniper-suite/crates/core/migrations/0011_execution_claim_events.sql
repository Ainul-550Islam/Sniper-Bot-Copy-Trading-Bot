-- 0011_execution_claim_events.sql — Prompt 3 gap closure (§M/§S): append-only
-- transition HISTORY for execution claims. The `execution_claims` row is
-- overwritten by design (that is what makes acquisition atomic) and therefore
-- carries lineage only one generation deep (previous_owner/takeover_count);
-- this table keeps EVERY transition forever, so the full ownership history of
-- any logical execution — every acquisition, takeover, release, hand-off,
-- fencing rejection and renewal rejection — is reconstructable from the
-- database alone, not just from logs.
--
-- Written best-effort by the Postgres claim store (an event-write failure
-- never changes the outcome of the claim operation itself).
--
-- Additive and restart-safe: IF NOT EXISTS only.

CREATE TABLE IF NOT EXISTS execution_claim_events (
    id           bigserial   PRIMARY KEY,
    execution_id text        NOT NULL,
    -- 'acquired' | 'reacquired' | 'takeover' | 'released' | 'handed_off'
    -- | 'fenced' | 'renew_rejected'
    event        text        NOT NULL,
    owner_id     text        NOT NULL,
    claim_epoch  bigint      NOT NULL,
    previous_owner text,
    detail       text        NOT NULL DEFAULT '',
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS execution_claim_events_id_idx
    ON execution_claim_events (execution_id, created_at);

CREATE INDEX IF NOT EXISTS execution_claim_events_time_idx
    ON execution_claim_events (created_at);
