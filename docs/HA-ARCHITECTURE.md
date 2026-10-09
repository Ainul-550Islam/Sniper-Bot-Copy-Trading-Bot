# HA Architecture (TASK 6)

How one trading system runs as several workers against one durable state
without duplicate execution and without lost events. Companion documents:
[DISTRIBUTED-OPERATIONS.md](DISTRIBUTED-OPERATIONS.md) (runbook) and
[CRASH-RECOVERY.md](CRASH-RECOVERY.md) (crash boundaries, the order
recovery matrix, failover reconciliation).

Nothing here is a claim of safety, profitability or institutional
production readiness. This layer makes ownership, durability and recovery
*deterministic and observable*; it does not make trading safe.

---

## 1. Scope and design rules

TASK 6 adds a reliability substrate under the TASK 1–5 engines. It adds no
strategy, no venue and no second source of truth.

| Rule | Where it holds |
|---|---|
| TASK 1–5 idempotency stays authoritative | per-execution ownership = `bot_core::ownership` claims; order identity = OMS `idempotency_key`; financial identity = `AccountingEvent::event_id`. TASK 6 never replaces them — it makes them safe across workers and restarts. |
| No process-local mutex is ownership | every cross-worker decision goes through the durable [`HaStore`] (migration 0016). A `Mutex` only protects in-process caches. |
| Fail closed | a store error is never "you own it": acquisition returns `Err`, fencing returns `FenceError::StoreUnavailable`, readiness drops. |
| Detection never takes over | a stale worker is reported (audit + metric); its leases are taken over only when they EXPIRE. |
| Never blindly resubmit | the recovery matrix has no `Resubmit` action; an ambiguous send is `HoldAmbiguous` + reconciliation. |
| Nothing silently skipped | a feed gap is journaled and stays `detected` until backfilled or explicitly accepted. |

---

## 2. Component map

```text
                    ┌─────────────────────────┐
                    │      Control plane      │  crates/server: API, probes,
                    │  /ready /api/ha /metrics │  workers, graceful shutdown
                    └────────────┬────────────┘
                                 │
                 ┌───────────────┴───────────────┐
            Worker A                         Worker B          one HaRuntime each
         (replica_id w-a)                 (replica_id w-b)
                 └───────────────┬───────────────┘
                                 │
                  Durable state — HaStore (migration 0016)
                                 │
     ┌──────────────┬────────────┼────────────┬──────────────┐
  ha_workers     ha_leases    ha_cursors   ha_feed_gaps  ha_recovery_records
  heartbeats    generations    positions     detected        one action
  generations   (fencing)      + counters    /backfilled     per subject
```

| File | Concern |
|---|---|
| `crates/core/src/ha/worker.rs` | worker identity, registration, heartbeat, the 9-state machine, HA modes |
| `crates/core/src/ha/lease.rs` | singleton role leases, fencing tokens, `FenceError` |
| `crates/core/src/ha/cursor.rs` | durable feed cursors, duplicate suppression, gap detection, replay |
| `crates/core/src/ha/recovery_plan.rs` | the crash-boundary and order-recovery matrices (pure) |
| `crates/core/src/ha/store.rs` | `HaStore` contract + `MemoryHaStore` |
| `crates/core/src/ha/runtime.rs` | `HaRuntime`: leases, cursors, readiness, graceful shutdown |
| `crates/core/src/ha/{metrics,audit}.rs` | `ha_*` series, `ha.*` audit actions |
| `crates/core/src/db/ha.rs` | `HaRepo` over migration 0016 |
| `crates/server/src/ha.rs` | `DbHaStore`, registration + recovery journalling, heartbeat loop, `LeasedWorker`, shutdown |

---

## 3. Worker identity

A worker is one process. Its identity is the existing `replica_id`
(`[ha].replica_id`, else `{host}-{pid}-{rand}`), so logs, metrics,
execution claims, ledger rows and `ha_workers` all name the same thing.

Registration (`ha_workers`) keeps the identity and takes the **next
generation**. That is what makes a zombie detectable: the heartbeat is a
compare-and-set on `(worker_id, generation)`, so a process whose identity
was re-registered by a newer life can no longer write. Its own runtime sees
`heartbeat() == false` and moves to `LEASE_LOST`.

Liveness is judged against the **shared store clock** (`HaStore::now`,
`now()` in PostgreSQL), never a worker clock.

### Worker state machine

```text
          register
 Starting ─────────► Recovering ──ok──► Ready ⇄ Running
     │                   │                ▲  │
     │                   │ failed         │  └──► Degraded ──┐
     │                   ▼                │                  │
     │           RecoveryRequired ◄───────┴──────────────────┤
     │                   │ retry                             │
     │                   └────────► Recovering               │
     │                                                       │
     │   lease lost ──► LeaseLost ──► Recovering             │
     └──────────── shutdown ──► Draining ──► Stopped ◄───────┘
```

`Ready`/`Running` are the only states that accept work or report READY.
`Stopped` is terminal for the process life; nothing leaves it. Illegal
transitions are refused (`WorkerTransitionError`) and change nothing.

---

## 4. Two scopes of ownership

| Scope | Unit | Mechanism | Since |
|---|---|---|---|
| per execution | one logical intent (`snipe:<mint>`, `copy:<wallet>:<mint>`, `poly:entry:<token>`, `exit:<position>:<rule>`) | `bot_core::ownership` claims: `execution_claims` + epochs, handoff grace for ambiguous sends | TASK 1–4 |
| per role (singleton) | one cluster-wide job: `reconciliation`, `recovery`, `accounting_maintenance`, `state_sync`, `feed:<name>` | `ha_leases` with fencing `generation` | TASK 6 |

Singleton jobs actually running under a lease today
(`crates/server/src/main.rs`):

| Lease role | Job | Why it must be singular |
|---|---|---|
| `reconciliation` | the venue/chain `RecoveryWorker` sweep | two replicas would race on the same reconciliation claim |
| `recovery` | hourly retention / housekeeping | retention deletes must run once, not once per replica |
| `state_sync` | periodic position re-verification sweep | it enqueues reconciliation claims; N replicas would enqueue N times |
| `accounting_maintenance` | the TASK 5 flush + reconcile + gauges pass | one ledger reconciliation per interval |

Each is a `LeasedWorker`: acquire → renew on `ttl/3` → **fence immediately
before every tick** → step down on loss → release on exit.

Both are durable, both fence, neither is a process mutex. Per-execution
claims let several workers trade different symbols at the same time
(active/active); role leases keep the singletons singular.

### Lease model

* `acquire` — ONE SQL statement (`INSERT … ON CONFLICT DO UPDATE … WHERE
  expired OR released OR self`) decides the winner; the loser gets
  `LeaseDecision::Rejected` with the current holder.
* `generation` — the fencing token. Strictly increasing per role, never
  reused, bumped on every acquisition (fresh, re-acquisition, takeover).
* `renew` / `release` / `verify` — compare-and-set on
  `(role, holder, generation)`. A stale worker cannot renew, cannot release
  the new owner's lease, and cannot pass a fence.
* `fence` — called immediately before every guarded mutation
  (`HaRuntime::guarded`). On failure the work does not run.
* Renewal cadence is `ttl / 3` (floored at 1 s), so two consecutive renewal
  failures still leave time before expiry.

---

## 5. HA modes (§9)

| Mode | Meaning | Requirement |
|---|---|---|
| `single` | one worker owns everything; leases still make a restart safe | no database needed (memory store = process scope) |
| `active_passive` | standbys run, hold no singleton lease and report NOT READY until they take one over | `[database].enabled = true` |
| `active_active` | several workers process concurrently; per-execution claims keep them apart, role leases keep singletons singular | `[database].enabled = true` |

Configuration validation refuses a clustered mode without a database: the
in-memory store is process-local and would give every worker its own
"truth".

---

## 6. Durable cursors (§4)

| Feed | Shape | Key | Wired in |
|---|---|---|---|
| `copy_logs` | opaque, scoped per leader wallet | signature | `module-copy/src/feeds.rs::run_poll` — warm-up resumes from the durable token; each batch advances it |
| `polymarket_user` | sequenced (local delivery counter) | delivery sequence | `module-polymarket/src/ws.rs::run_user_feed` — the counter continues across lives; skipped deliveries become gaps |
| `sniper_launches` | opaque | signature | cursor available; the launch feed is fully covered by the TASK 2 dedup (`mark_launch_seen`) |
| `copy_geyser` | sequenced | slot/sequence | cursor available; the Geyser stream shares the `copy_event` dedup |
| `polymarket_market` | sequenced | stream sequence | cursor available; the book feed carries no money-bearing events |

The two feeds that can LOSE money-bearing events on a restart — the copy
poll loop (a leader trade that happened while the process was down) and the
authenticated Polymarket user channel (a fill delivery) — are wired. The
remaining three have cursors available but are protected by their existing
dedup and carry no unique financial truth, so they are not forced through a
durable position (that would add writes without removing a loss mode).

`offer(position)` is deterministic: `<= cursor` → `Duplicate` (suppressed,
counted); `== cursor+1` → `Advanced`; `> cursor+1` → `Advanced` **plus** a
`FeedGap` recorded in `ha_feed_gaps`, audited and metered. The current item
is always processed — dropping it would turn one gap into two. A restart
loads the row and resumes; `replay_from` rewinds deliberately for a
backfill and is audited.

---

## 7. Readiness (§11)

`GET /ready` is the existing probe; TASK 6 adds a `worker` component that
is NOT ready when any of these hold:

* the worker state is not `Ready`/`Running` (`Starting`, `Recovering`,
  `Degraded`, `LeaseLost`, `RecoveryRequired`, `Draining`, `Stopped`);
* durable recovery has not completed in this life;
* a role in `[ha].required_roles` is not held — **re-verified against the
  store on every refresh**, never from the local cache;
* a reported dependency (`database`, …) is unhealthy.

So a standby that is perfectly healthy but owns nothing reports NOT READY,
and a worker that lost its lease stops reporting READY within one refresh.

---

## 8. Graceful shutdown (§10)

`ha::shutdown` runs as its own time-bounded phase in the server's shutdown
sequence, between the module drain and the pump flush:

1. `Draining` — stop accepting new work, refresh readiness (now false).
2. Persist every cursor this worker advanced.
3. Release every held lease so a standby takes over immediately instead of
   waiting for expiry.
4. `Stopped` + audit (`ha.shutdown` with the phase).

Shutting down twice is a no-op. A crash (no clean shutdown) is equally
safe — leases simply expire.

---

## 9. Metrics and audit

Metrics (`ha_*`): `worker_state`, `worker_state_changes_total`,
`worker_heartbeats_total{outcome}`, `workers_seen{health}`,
`lease_operations_total{role,op,outcome}`, `leases_held{role}`,
`lease_takeovers_total{role}`, `fenced_mutations_total{role,reason}`,
`recovery_actions_total{scope,action}`, `recovery_failures_total`,
`replay_events_total{feed,outcome}`, `feed_gaps_total{feed}`,
`cursor_lag_secs{feed}`, `cursor_position{feed}`, `readiness`,
`readiness_changes_total`. Fenced mutations and suppressed duplicates also
feed the shared `bot_duplicate_execution_prevented_total{where}`.

Audit (actor `ha`): `ha.worker.registered|state|heartbeat_failed|stale_detected`,
`ha.lease.acquired|renewed|lost|released|takeover|fenced`,
`ha.recovery.started|action|completed|failed`, `ha.feed.gap|replay`,
`ha.readiness`, `ha.shutdown`.

---

## 10. Configuration (`[ha]`)

| Key | Default | Meaning |
|---|---|---|
| `replica_id` | generated | worker identity (unchanged from TASK 1) |
| `claim_lease_secs` | 45 | PER-EXECUTION claim lease (unchanged) |
| `claim_handoff_grace_secs` | 900 | ambiguous-send handoff grace (unchanged) |
| `flag_sync_secs` | 5 | runtime-flag sync (unchanged) |
| `book_sync_secs` | 30 | position-book sync (unchanged) |
| `mode` | `single` | `single` / `active_passive` / `active_active` |
| `heartbeat_secs` | 10 | heartbeat period |
| `heartbeat_timeout_secs` | 45 | staleness threshold (raised to ≥ 2 × heartbeat) |
| `role_lease_secs` | 45 | singleton role lease duration (floor 5 s) |
| `required_roles` | `[]` | roles that must be held to report READY |

---

## 11. Tests

`crates/core/tests/ha_distributed.rs` (17 offline tests against real
`AppState` workers sharing one store) plus 37 unit tests in `ha/*`. See
[TESTING.md](TESTING.md) and [CRASH-RECOVERY.md](CRASH-RECOVERY.md) §6.

---

## 12. Limitations

* The PostgreSQL path of `HaStore` is exercised by the gated integration
  test only; the sandbox has no database.
* Leases protect the singleton JOBS this suite runs; they are not a general
  distributed lock service and are not safe against arbitrary clock skew
  beyond the lease TTL (the store clock is the reference, which removes
  worker drift but not a badly wrong database clock).
* Feed gaps are detected on sequenced feeds. Signature-style feeds have no
  orderable key, so their "gap" concept is the venue's own pagination —
  duplicate suppression applies, gap detection does not.
* Active/active parallelism is bounded by what the per-execution claims
  cover: two workers never trade the same intent, but the suite has no
  cross-worker rate coordination beyond the existing risk limits.
