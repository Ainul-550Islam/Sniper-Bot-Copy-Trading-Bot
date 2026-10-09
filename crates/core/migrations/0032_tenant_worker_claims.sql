-- 0032 — tenant worker claim lanes (PROMPT 3/10 §H48–H51).
--
-- AUDIT NOTE (revision of the earlier "skip 0032" decision): the
-- global `ha_leases` plane (0016) STAYS GLOBAL — worker registration,
-- feed cursors and the operator-plane leases are deployment
-- infrastructure, not tenant data, and are untouched here. What the
-- tenant data plane additionally needs is PER-TENANT work lanes:
-- "which replica leads tenant X's copy pipeline?" That question has
-- no home in `ha_leases` (role PK is a global enum vocabulary) and no
-- other tenant table carries a lease (TTL + holder + fencing token).
-- Per the only-if-required migration rule this table is required, so
-- the reserved 0032 slot is filled with exactly this and nothing
-- else. No fake swaps, no `organization_id` bolted onto ha_*.
--
-- Lane identity: (organization_id, purpose). Two tenants may run the
-- same purpose independently; two workers of ONE tenant race on the
-- same lane through the single-statement CAS in
-- trading_repository::worker_claim::acquire (the exact legacy
-- ha_leases acquisition shape, tenant-composite arbiter).
--
-- `generation` is the fencing token: strictly increasing per lane,
-- never reused — a stale leader's writes are rejected downstream by
-- token comparison.

CREATE TABLE IF NOT EXISTS worker_claims (
    organization_id uuid        NOT NULL,
    -- 'copy_pipeline' | 'poly_recon' | 'gc' | 'reporting' | 'recovery_sweep'
    -- | custom lane names (free text, 1..=64).
    purpose         text        NOT NULL CHECK (purpose <> ''),
    leader_name     text        NOT NULL,
    -- FENCING TOKEN for this lane. Strictly increasing, never reused.
    generation      bigint      NOT NULL DEFAULT 1,
    acquired_at     timestamptz NOT NULL DEFAULT now(),
    heartbeat_at    timestamptz NOT NULL DEFAULT now(),
    lease_until     timestamptz NOT NULL,
    takeover_count  bigint      NOT NULL DEFAULT 0,
    previous_leader text,
    released        boolean     NOT NULL DEFAULT false,
    PRIMARY KEY (organization_id, purpose)
);

-- Who currently leads anything (operator triage).
CREATE INDEX IF NOT EXISTS worker_claims_leader_idx
    ON worker_claims (leader_name);
-- Lapse sweep: live lanes whose lease has expired.
CREATE INDEX IF NOT EXISTS worker_claims_lapse_idx
    ON worker_claims (lease_until)
    WHERE released = false;
