-- 0016_ha_workers_leases_cursors.sql — TASK 6 (HA / crash recovery /
-- distributed reliability): worker registry + heartbeats, singleton role
-- leases with fencing generations, durable feed cursors, detected feed gaps
-- and the recovery record journal.
--
-- Nothing here replaces an existing table. `execution_claims` (0009) +
-- `execution_claim_events` (0011) remain the PER-EXECUTION ownership record
-- (one claim per logical intent: `snipe:<mint>`, `copy:<wallet>:<mint>`,
-- `poly:entry:<token>`); this migration adds the other scope: SINGLETON
-- ROLE leases (one active reconciliation worker, one recovery worker, one
-- accounting maintenance loop, one consumer per feed) plus the durability
-- that makes a restart or a takeover deterministic. `orders` /
-- `executions` (0002) stay the order truth, `ledger_events` (0015) stays
-- the financial truth, `reconciliation_state` (0005) stays the venue-truth
-- queue.
--
--   ha_workers
--       One row per worker IDENTITY (the process `replica_id`). A restart
--       keeps the identity and takes a NEW `generation`; the previous
--       generation can no longer heartbeat (the heartbeat CAS includes the
--       generation), which is how a zombie process is detected. `last_seen_at`
--       is compared against the SHARED database clock, never a worker clock.
--
--   ha_worker_events
--       Append-only worker lifecycle log (registered / state / heartbeat
--       failure / stale detected), so a post-mortem can see the order in
--       which workers came and went.
--
--   ha_leases
--       One row per singleton role. `generation` is the FENCING TOKEN: it
--       increases on every acquisition (fresh, takeover, re-acquisition) and
--       is never reused, so a delayed write from a stale holder is
--       recognisable. Acquisition is `INSERT … ON CONFLICT DO UPDATE …
--       WHERE` the current lease is expired/released/self-held — one
--       statement, so two workers racing cannot both win. Renew / release /
--       verify are compare-and-set on `(holder, generation)`.
--
--   ha_lease_events
--       Append-only lease lifecycle log (acquired / renewed / lost /
--       released / takeover / fenced) — the audit of who owned what, when.
--
--   ha_cursors
--       Durable feed positions (`feed[:scope]`): last sequence for
--       sequenced feeds, last opaque token for signature-style feeds, plus
--       lifetime counters (processed / duplicates / gaps). A restart resumes
--       here instead of at "now" (which loses events) or at the beginning
--       (which replays work).
--
--   ha_feed_gaps
--       Detected discontinuities in a sequenced feed. A gap is NEVER
--       silently skipped: the row stays `detected` until a backfill
--       re-delivers the range or an operator accepts it explicitly.
--
--   ha_recovery_records
--       Append-only journal of deterministic recovery actions (which worker,
--       which trigger, which scope, which subject, which action, why).
--
-- Additive and restart-safe: IF NOT EXISTS only, no rewrites of existing data.

CREATE TABLE IF NOT EXISTS ha_workers (
    -- Stable worker identity = the process `replica_id`.
    worker_id     text        PRIMARY KEY,
    -- Monotonic per-identity life counter; a restart increments it.
    generation    bigint      NOT NULL DEFAULT 1,
    mode          text        NOT NULL DEFAULT 'single'
                                CHECK (mode IN ('single', 'active_passive', 'active_active')),
    state         text        NOT NULL DEFAULT 'starting' CHECK (state IN (
                                'starting', 'recovering', 'ready', 'running', 'degraded',
                                'lease_lost', 'recovery_required', 'draining', 'stopped')),
    host          text        NOT NULL DEFAULT '',
    pid           bigint      NOT NULL DEFAULT 0,
    version       text        NOT NULL DEFAULT '',
    detail        text        NOT NULL DEFAULT '',
    started_at    timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ha_workers_last_seen_idx ON ha_workers (last_seen_at DESC);
CREATE INDEX IF NOT EXISTS ha_workers_state_idx ON ha_workers (state);

CREATE TABLE IF NOT EXISTS ha_worker_events (
    id          bigserial   PRIMARY KEY,
    worker_id   text        NOT NULL,
    generation  bigint      NOT NULL DEFAULT 0,
    event       text        NOT NULL CHECK (event IN (
                    'registered', 'state', 'heartbeat_failed', 'stale_detected', 'shutdown')),
    state       text,
    detail      text        NOT NULL DEFAULT '',
    ts          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ha_worker_events_worker_idx ON ha_worker_events (worker_id, ts DESC);

CREATE TABLE IF NOT EXISTS ha_leases (
    -- `reconciliation` | `recovery` | `accounting_maintenance` | `state_sync`
    -- | `feed:<name>`.
    role            text        PRIMARY KEY,
    holder          text        NOT NULL,
    -- FENCING TOKEN. Strictly increasing per role, never reused.
    generation      bigint      NOT NULL DEFAULT 1,
    acquired_at     timestamptz NOT NULL DEFAULT now(),
    renewed_at      timestamptz NOT NULL DEFAULT now(),
    expires_at      timestamptz NOT NULL,
    takeover_count  bigint      NOT NULL DEFAULT 0,
    previous_holder text,
    released        boolean     NOT NULL DEFAULT false
);
CREATE INDEX IF NOT EXISTS ha_leases_holder_idx ON ha_leases (holder);
CREATE INDEX IF NOT EXISTS ha_leases_expiry_idx ON ha_leases (expires_at)
    WHERE released = false;

CREATE TABLE IF NOT EXISTS ha_lease_events (
    id          bigserial   PRIMARY KEY,
    role        text        NOT NULL,
    holder      text        NOT NULL,
    generation  bigint      NOT NULL DEFAULT 0,
    event       text        NOT NULL CHECK (event IN (
                    'acquired', 'renewed', 'lost', 'released', 'takeover', 'fenced')),
    detail      text        NOT NULL DEFAULT '',
    ts          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ha_lease_events_role_idx ON ha_lease_events (role, ts DESC);

CREATE TABLE IF NOT EXISTS ha_cursors (
    -- `feed[:scope]`.
    cursor_key      text        PRIMARY KEY,
    feed            text        NOT NULL CHECK (feed IN (
                        'sniper_launches', 'copy_logs', 'copy_geyser',
                        'polymarket_market', 'polymarket_user')),
    scope           text        NOT NULL DEFAULT '',
    -- Last processed sequence (sequenced feeds).
    position        bigint,
    -- Last processed opaque token (signature-style feeds).
    token           text,
    last_event_at   timestamptz,
    processed_count bigint      NOT NULL DEFAULT 0,
    duplicate_count bigint      NOT NULL DEFAULT 0,
    gap_count       bigint      NOT NULL DEFAULT 0,
    worker_id       text        NOT NULL DEFAULT '',
    updated_at      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ha_cursors_feed_idx ON ha_cursors (feed, updated_at DESC);

CREATE TABLE IF NOT EXISTS ha_feed_gaps (
    id            bigserial   PRIMARY KEY,
    feed          text        NOT NULL,
    scope         text        NOT NULL DEFAULT '',
    from_position bigint      NOT NULL,
    to_position   bigint      NOT NULL,
    status        text        NOT NULL DEFAULT 'detected'
                                CHECK (status IN ('detected', 'backfilled', 'accepted')),
    worker_id     text        NOT NULL DEFAULT '',
    detected_at   timestamptz NOT NULL DEFAULT now(),
    resolved_at   timestamptz,
    UNIQUE (feed, scope, from_position, to_position)
);
CREATE INDEX IF NOT EXISTS ha_feed_gaps_open_idx ON ha_feed_gaps (feed, detected_at DESC)
    WHERE status = 'detected';

CREATE TABLE IF NOT EXISTS ha_recovery_records (
    id          bigserial   PRIMARY KEY,
    worker_id   text        NOT NULL,
    generation  bigint      NOT NULL DEFAULT 0,
    -- `startup` | `takeover` | `periodic`.
    trigger     text        NOT NULL DEFAULT 'startup',
    -- `orders` | `ledger` | `cursors` | `risk` | `positions`.
    scope       text        NOT NULL,
    subject     text        NOT NULL DEFAULT '',
    -- One of the deterministic order-recovery actions (§6).
    action      text        NOT NULL CHECK (action IN (
                    'close_unsent', 'hold_ambiguous', 'adopt_from_venue', 'resume_tracking',
                    'finalize_filled', 'finalize_cancelled', 'finalize_expired', 'no_action')),
    detail      text        NOT NULL DEFAULT '',
    ts          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS ha_recovery_records_ts_idx ON ha_recovery_records (ts DESC);
CREATE INDEX IF NOT EXISTS ha_recovery_records_scope_idx ON ha_recovery_records (scope, ts DESC);
CREATE INDEX IF NOT EXISTS ha_recovery_records_subject_idx ON ha_recovery_records (subject)
    WHERE subject <> '';
