# Distributed Operations Runbook (TASK 6)

How to run, observe and troubleshoot the suite as one or several workers.
Architecture: [HA-ARCHITECTURE.md](HA-ARCHITECTURE.md). Recovery matrix:
[CRASH-RECOVERY.md](CRASH-RECOVERY.md).

Nothing here is a safety or profitability claim. It is the operating
procedure for the ownership and durability layer.

---

## 1. Choosing a mode

| You want | `[ha].mode` | Also set |
|---|---|---|
| one process, simplest | `single` | nothing; a database is optional |
| one active + warm standbys | `active_passive` | `[database].enabled = true`, `required_roles` on every worker |
| several workers trading concurrently | `active_active` | `[database].enabled = true`; per-execution claims already keep them apart |

```toml
[ha]
mode = "active_active"
heartbeat_secs = 10
heartbeat_timeout_secs = 45
role_lease_secs = 45
required_roles = ["accounting_maintenance"]   # this worker is only READY while it owns it
```

Startup refuses a clustered mode without a database — the in-memory store is
process-local and would give each worker its own "truth".

**Every worker in a cluster must point at the same PostgreSQL instance and
run the same version.** Mixed versions are allowed only for a rolling
restart (the lease + generation model tolerates it), never as a steady
state.

---

## 2. Rolling restart / deploy

1. Check the cluster is healthy: `GET /api/ha` → every worker `live`, no
   unresolved gaps, no `recovery_required`.
2. `SIGTERM` one worker. It drains: stops accepting, persists cursors,
   releases its leases, reports NOT READY, exits (`ha.shutdown` audit
   records the phases).
3. A standby picks the released roles up **immediately** (release, not
   expiry — no lease-TTL wait).
4. Start the new version; watch it go `starting → recovering → ready` and
   its `ha.recovery.completed` audit line.
5. Repeat. Never restart two workers at once in active/passive: the second
   one has nowhere to fail over to.

If a worker is killed instead of drained, its leases expire after
`role_lease_secs` and are taken over then; nothing is lost, the gap is just
longer.

---

## 3. What to watch

| Signal | Meaning / action |
|---|---|
| `ha_worker_state{state="recovery_required"} == 1` | recovery could not finish — read `ha.recovery.failed`; the worker is deliberately NOT READY |
| `ha_workers_seen{health="stale"} > 0` | a worker missed its heartbeat deadline; check the process and the database |
| `ha_lease_takeovers_total` increasing | workers keep losing leases: DB latency, GC pauses, or `role_lease_secs` too short |
| `ha_fenced_mutations_total` increasing | a stale worker keeps trying to act; it is being refused (working as designed) — find out why it thinks it owns the role |
| `ha_readiness == 0` for a serving worker | read `/ready` → the `worker` component prints the exact reason |
| `ha_feed_gaps_total` increasing | a sequenced feed skipped events; see §5 |
| `ha_cursor_lag_secs{feed}` rising | the consumer is behind or the feed stopped |
| `ha_replay_events_total{outcome="duplicate"}` | normal after a restart; sustained growth means a feed is re-delivering |
| `ha_recovery_failures_total` | a recovery step could not complete (usually the journal) |

Audit stream: `actor = ha`, actions `ha.worker.*`, `ha.lease.*`,
`ha.recovery.*`, `ha.feed.*`, `ha.readiness`, `ha.shutdown`.

---

## 4. Reading ownership

* `GET /api/ha` — this worker's identity, generation, state, readiness with
  reasons, the roles it holds, the cluster registry with heartbeat ages, all
  leases with holder / generation / expiry, cursors with lag, unresolved
  gaps and the last recovery records.
* `GET /ready` — the probe a load balancer uses. The `worker` component
  carries the human reason when not ready.
* SQL: `SELECT role, holder, generation, expires_at, takeover_count FROM
  ha_leases ORDER BY role;`

A worker that owns nothing in active/passive is **supposed** to report NOT
READY. That is not a fault.

---

## 5. Feed gaps

A gap means a sequenced feed skipped positions. It is recorded, audited and
metered, and it stays `detected` until you resolve it. Never ignore it.

1. `GET /api/ha` → `gaps` (or `SELECT * FROM ha_feed_gaps WHERE status =
   'detected'`).
2. Decide whether the range matters (a Polymarket user-channel gap can hide
   a fill; a market-book gap is usually harmless).
3. Backfill: rewind the cursor deliberately (`HaRuntime::replay_from`,
   audited as `ha.feed.replay`) and let the feed re-deliver. Duplicate
   suppression stops anything already processed from being processed twice,
   and the ledger/OMS/claim idempotency stops any money effect from
   repeating.
4. Mark it resolved: `backfilled` after a successful replay, or `accepted`
   if you decide the loss is acceptable (both are audited).

---

## 6. Incident playbooks

**A worker is stuck in `recovery_required`.** Its durable state could not be
rebuilt (usually the database was unreachable during startup). Fix the
dependency and restart the process; it re-runs recovery from the journals.
Do not force it ready — it would trade against an empty book.

**Two workers both look active.** Check `ha_leases`: only one holder per
role is possible. If both claim to be "running", one of them is fenced and
simply has not noticed yet; its next fence, renewal or readiness refresh
fails and it steps down. Confirm with `ha_fenced_mutations_total`.

**The database went away.** Every worker fails closed: acquisitions error,
fences return `store_unavailable`, readiness drops, the accounting ledger
parks events as pending. No worker will keep mutating shared state. Restore
the database; pending events flush on the next maintenance tick and
reconciliation reports anything that drifted.

**A worker died mid-order.** Nothing is resubmitted. At restart the order
gets exactly one action (see [CRASH-RECOVERY.md](CRASH-RECOVERY.md) §3);
ambiguous ones are held for reconciliation, which finalizes them from venue
truth.

**Lease churn.** If `ha_lease_takeovers_total` climbs steadily, raise
`role_lease_secs` (and therefore the renewal interval) or fix the latency
that makes renewals miss.

---

## 6b. Which jobs are cluster singletons

| Lease role | Job | Effect if the holder dies |
|---|---|---|
| `reconciliation` | venue/chain reconciliation sweep | a standby takes it over after the lease expires (or immediately after a clean shutdown); claims are re-armed, nothing is lost |
| `recovery` | hourly retention / housekeeping | the next holder runs it on its own schedule |
| `state_sync` | periodic position re-verification | re-enqueues on the next tick of the new holder |
| `accounting_maintenance` | TASK 5 flush + reconcile + gauges | pending ledger events flush on the new holder's first tick |

Everything else (module trading loops, feeds, the API, persistence pumps)
runs on every worker: per-execution claims and the OMS / ledger idempotency
keep them from doing the same unit of work twice.

## 7. Capacity notes

* `heartbeat_secs` drives one small UPDATE per worker per period.
* Each leased worker performs one `verify` per tick and one `renew` per
  `role_lease_secs / 3`.
* Cursor writes happen once per processed item per feed; sequenced feeds
  with very high rates should be batched by the consumer before offering
  (the cursor API is cheap but the write is not free).

---

## 8. What this layer will not do

* It will not fabricate a fill, resubmit an order, or "fix" an unexplained
  difference.
* It will not make a standby ready without ownership.
* It will not take work from a stale worker before its lease expires.
* It has not been operated against a live venue, a funded wallet or a real
  multi-host cluster.
