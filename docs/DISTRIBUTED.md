# Distributed Operation — Single Active Logical Execution Owner

Prompt 3 deliverable (§Z). This document describes how sniper-suite runs
safely as **multiple concurrent replicas** and states — precisely and
honestly — what is guaranteed, by which mechanism, and where the limits are.

> **The invariant.** ONE logical execution intent → AT MOST ONE **single
> active logical execution owner** → AT MOST ONE money-moving submission,
> until authoritative external state (chain, venue, database) proves what
> actually happened.

Everything below maps onto code; section references (§B…§Y) are the Prompt 3
requirement sections.

---

## 1. Concepts and vocabulary

| Term | Meaning |
|---|---|
| **Replica** | One running `sniper-suite` process. Identity: `[ha].replica_id` or generated `{hostname}-{pid}-{rand8}` at startup (§C). A restart is a NEW replica — the old life's claims expire by lease and are taken over with an incremented epoch. |
| **Logical execution id** | Stable, cross-replica name of ONE intent: `snipe:{mint}`, `copy:{wallet}:{mint}`, `exit:{position_id}:{rule}`, `poly:entry:{token_id}`. Never a per-replica id, never a raw signature. |
| **Claim** | A leased, fenced ownership record for one logical execution id (§B/§T). |
| **Epoch** | The fencing token. Increments on every takeover/re-acquisition. |
| **Lease** | Time-bounded ownership (`[ha].claim_lease_secs`, default 45 s, renewed every lease/3 while working, bounded by `MAX_RENEWALS = 40`). |
| **Handoff grace** | After an AMBIGUOUS terminal (`Sent`/`SendUnknown`), the id blocks re-acquisition for `[ha].claim_handoff_grace_secs` (default 900 s) while reconciliation proves the outcome (§I case E). |
| **Permit / guard** | The module-side API (`bot_core::ownership::Permit`, `ClaimGuard`). Business modules never touch raw Redis keys or SQL (§V). |

## 2. Claim state machine (exact)

```
              claim(id) — ATOMIC single statement
 (absent) ─────────────────────────────────────────► CLAIMED(epoch=1)
                                                          │ renew: CAS(owner,epoch,claimed,
                                                          │        unexpired), every lease/3,
                                                          │        bounded (MAX_RENEWALS)
             ┌────────────────────────────────────────────┤
             │ release() — determinate outcome            │ hand_off() — ambiguous outcome
             │ (Confirmed / SimulationFailed /            │ (Sent / SendUnknown / poly
             │  SendFailed / LandedFailed / paper)        │  SubmitUnknown / poly posted)
             ▼                                            ▼
         RELEASED ── re-acquirable NOW               HANDED_OFF ── re-acquirable
         (re-fired exit rule = new decision)         ONLY after handoff grace
 CLAIMED with lease_until < now ══► EXPIRED (implicit)
             └──► takeover: CLAIMED(epoch+1, takeover_count+1,
                  previous_owner recorded) — the stale owner is FENCED:
                  every verify/renew/release CAS on (owner,epoch) fails.
```

Rows/hashes are **never deleted** on the authoritative store (§M/§S):
`execution_claims` answers "which replica believed it owned this execution,
when it acquired, when it lost, who took over" forever. Redis terminal
hashes expire after 7 days (Redis is not the audit layer).

Because the claim row is overwritten by design (that is what makes
acquisition atomic), the Postgres store ALSO appends every transition to
`execution_claim_events` (migration `0011`): `acquired | reacquired |
takeover | released | handed_off | fenced | renew_rejected` with owner,
epoch, previous owner and a detail string naming the current holder on
rejections. The FULL multi-generation lineage of any logical execution is
therefore reconstructable from the database alone (`PostgresClaimStore::
events`). Event writes are best-effort: an audit-write failure is logged
but never changes the claim outcome — the claim row stays the authority.
Redis/memory stores have no events table (Postgres is the audit layer).

## 3. Store authority (§D/§K/§L)

Precedence, chosen once at startup and logged:

1. **Postgres** (`execution_claims`, migration `0009`) — AUTHORITATIVE
   whenever `[database]` is enabled. Acquisition is one atomic
   `INSERT … ON CONFLICT DO UPDATE … WHERE (lease expired | released |
   grace elapsed) RETURNING …` plus a `prev` CTE that classifies takeovers
   for metrics. Fencing ops are CAS `UPDATE … WHERE owner AND epoch AND
   status='claimed' [AND lease_until > now()]`.
2. **Redis** (`own:claim:{execution_id}` hashes, §U) — only when Postgres
   is absent. All transitions are single Lua scripts; **all timestamps come
   from `redis.call('TIME')`**, so replicas with skewed clocks cannot
   manufacture or extend leases. A claim is a lease, not money state: if
   Redis restarts, hashes vanish → claims read as absent → a replica may
   re-acquire with epoch 1. This is why the intent journal + reconciliation
   remain the backstop, and why Postgres is authoritative when configured.
3. **Memory** — process-local. Single-instance/paper deployments and tests
   only. **Never an HA safety mechanism**: startup logs a loud warning, and
   live mode + memory store logs an additional explicit
   "multi-replica WILL double-execute" warning (§K).

**Fail closed (§K/§L).** Every store call propagates errors; the registry
converts them to `BotError::OwnershipUnavailable` and money paths abort.
"Could not acquire ownership" is never "ownership acquired"; there is no
silent fallback to process-local locking for money operations. A failed DB
ack of a release does not mean the external action failed — the row keeps
its lease and expires on its own, and reconciliation (not blind retry)
decides what happened (§L).

## 4. Fencing (§E) — and its honest limits

`ClaimGuard::fence()` re-validates `(id, owner, epoch, claimed, unexpired)`
**immediately before every money-moving continuation** (journal write,
broadcast, venue POST, position mutation). A replica whose lease lapsed and
was taken over holds a stale epoch and gets `BotError::ClaimRejected`
before it can send anything.

Honest statement: this is **lease-based fencing**, not a fenced storage
layer — between a successful `fence()` and the actual broadcast there is a
small window in which a pathological pause (GC/VM freeze) could let the
lease lapse and another replica take over. The hard guarantees come from
the **combination**, exactly as designed in §I:

```
CLAIM ──► fence ──► intent journal (pre-broadcast) ──► broadcast
      ──► link signature ──► release / hand_off
```

* The intent journal makes every submission attempt durable before it
  leaves the process; a duplicate submission by a takeover replica leaves
  its own intent row, and startup/periodic reconciliation enqueues the
  symbol and blocks further entries until on-chain truth is established.
* Ambiguous outcomes hand off (grace window) instead of releasing, so no
  replica resubmits while the outcome is unknown.
* Chain-level idempotence helps: a duplicate spot sell fails on insufficient
  balance; OMS `idempotency_key` + `record_submitted` dedupe ledger writes.

Fencing rejects are metered (`bot_distributed_fencing_rejected_total`) and
logged loudly — a non-zero value means the system worked as designed but
something (pause, clock, network) deserves investigation.

## 5. Money-path inventory (§F/§P)

| Path | Logical id | Claim point | Fence point | Terminal |
|---|---|---|---|---|
| Sniper entry (curve + Jupiter fallback) | `snipe:{mint}` | `consider_launch`, after risk approval, before curve load | before `with_intent`/broadcast in both buy paths | `status.is_ambiguous()` (Sent/SendUnknown) → hand_off, else release |
| Sniper exit (sweeper) | `exit:{position.id}:{rule}` | `sweep_once`, per sell decision | in `sell_position` before the curve/Jupiter branch | by status; pre-broadcast errors release |
| Copy entry (mirror buy) | `copy:{wallet}:{mint}` | `mirror_trade`, after risk gates, before `buy` | before `with_intent`/`may_broadcast` | same rules; Jupiter path hands off unless confirmation was OBSERVED (`ConfirmOutcome::Confirmed`) |
| Copy exit (sweeper + whale mirror-exit) | `exit:{position.id}:{rule}` / `…:mirror_exit` | `sweep_once` / whale-sell branch | in `sell_position` | same rules |
| Polymarket entry | `poly:entry:{token_id}` | `act_on_decision`, after risk, before submit | before `submit_live` | POSTed → **hand_off** (order may rest on the book; grace ≫ book-sync interval); `SubmitUnknown` → hand_off; definite CLOB rejection → release; paper → release |
| Polymarket `flatten`/`cancel_all` | — | no claim | — | bookkeeping-only (local close / venue-wide cancel, idempotent); no submission is made per position |
| Telegram | — | none needed | — | audit (Prompt 2) proved Telegram has NO money-moving commands; kill/enable propagate via flag sync |

Multi-feed dedup (§H) keeps its existing identity (persistent dedup facade
for launches; `mark_copied` cooldown stays as a process-local rate guard) —
the claim is the authoritative cross-replica dedup: N replicas observing the
same whale trade produce exactly one mirrored execution.

**Claim-before-journal ordering (§I).** The claim is always acquired
*before* the intent row is written, so intent rows are attributable to the
owner that wrote them. Crash-at-any-stage behaviour (§J):

| Crash stage | Who recovers, how |
|---|---|
| 1. After claim, before intent | Lease expires (≤45 s); takeover; nothing was submitted; dedup/symbol state unchanged. |
| 2. After intent, before broadcast | Orphan `pending` intent → startup reconciliation enqueues + gates the symbol; claim expires → takeover possible but entry blocked by the gate until resolved. |
| 3. During broadcast (no link) | Same as 2 — ambiguous by definition, never auto-resubmitted. |
| 4. After link (signature known) | Reconciliation queries the chain; claim expired/handed-off; position book converges via DB. |
| 5. After position write, before release | Lease expires; the id is re-acquirable, but the position exists → `find_open`/risk duplicate-symbol checks refuse a second entry. |
| 6. During exit broadcast | Intent + signature attribution → reconciliation; handoff grace (if it got that far) blocks re-sell; on-chain duplicate sell fails on balance anyway. |
| 7. Mid flag write | Local state already applied; remote replicas converge on the next successful write (logged loudly on failure). |
| 8. Redis-only deployment restart | All claim hashes lost → everything reads absent → epoch-1 re-acquisition; intent journal (if DB) still catches in-flight ambiguity; this is the documented reason Postgres is authoritative when present. |

## 6. Renewal, shutdown, takeover (§N/§O/§J)

* **Renewal**: `ClaimGuard::run_guarded` races work against a ticker
  (lease/3 + epoch-derived jitter, bounded by `MAX_RENEWALS`) so a hung
  worker eventually loses its lease and gets fenced instead of holding
  ownership forever. Expired leases are never resurrected by a late renew —
  recovery is always a fresh claim (takeover path) so epoch/audit stay
  honest. Modules whose broadcast paths are internally timeout-bounded
  (executor, HTTP clients) fence + finish without needing the ticker.
* **Shutdown (§O)**: module loops observe the shutdown coordinator and stop
  claiming; in-flight guards complete their terminal transition (or drop →
  lease expiry, which is exactly the crash semantics); the flag-sync and
  book-sync tasks break on the same coordinator; renewal stops with the
  guard.
* **Takeover**: automatic at the next claim attempt after `lease_until`
  passes — no separate reaper needed. `takeover_count` and `previous_owner`
  record the lineage.

## 7. Global controls across replicas (§Q)

**Runtime flags** (`runtime_flags`, migration `0010`; Redis `own:flag:*`
fallback): the kill switch, the emergency-halt latch (`halted`, OR-ed into
the kill gate and set by `emergency_stop` / cleared by `/resume`) and the
module enable flags are GLOBAL state. Every local mutation
(`set_kill_switch`, `clear_halt`, `set_enabled`, `emergency_stop`) stamps
`flags_touched` and publishes through the attached `RuntimeFlagsWriter`.
Each replica runs a sync task every `[ha].flag_sync_secs` applying
`AppState::apply_flag_sync` with these staleness rules:

* **kill/halt ON applies immediately** — the safe direction wins over any
  local recency;
* kill/halt OFF and module flags apply only when the shared row is **newer
  than this replica's last local decision** (or never touched locally →
  boot convergence);
* remote values are never echoed back; store failures keep the local view
  (a stale local view is safer than a reset one) and are metered.

Assumption: replica clocks are roughly NTP-synced (staleness compares the
writer's DB timestamp against the local decision time).

**Cluster-wide risk view (gap closure)**: with Postgres attached the server
also installs a `GlobalRiskOracle` (`PostgresRiskOracle`). On EVERY risk
check the engine combines the local view with the shared `positions` table:
open-position capacity uses `max(local_count, global_count)` and the daily
realized-loss gate uses `min(local_pnl, global_pnl)` (more negative wins) —
so global limits converge **within one query** instead of within
`book_sync_secs`, and one replica's losses count against every replica's
daily limit. Oracle failures return "unknown" and fall back to the local
view (§K: risk never depends on store availability); an oracle can only
TIGHTEN a limit, never loosen it. Honest approximation: `realized_today`
attributes the full lifecycle PnL (`realized_quote - cost_basis`) of
positions CLOSED today (UTC) to today; partial exits on still-open
positions are visible only through each replica's local accumulator until
the position closes.

**Position book sync**: every `[ha].book_sync_secs` each replica merges
`PositionRepo::list_open()` into local state (`AppState::merge_positions`):
insert unknown rows; overwrite a local row only when the DB row is newer
AND the local row is not terminal (never resurrect locally closed
positions). This is what makes risk capacity (max open positions, daily
exposure, duplicate-symbol checks) converge across replicas — and it is
what makes `exit:{position.id}:{rule}` a stable shared identity (position
ids also converge at startup via DB restore).

## 8. Observability (§R/§S)

Metrics (low cardinality only — **no wallets, signatures, tokens or order
ids as labels**):

| Metric | Meaning |
|---|---|
| `bot_distributed_claim_acquired_total{module,kind}` | Claims won |
| `bot_distributed_claim_rejected_total{module,kind,reason}` | Claims lost (`owned_by_other`) or blocked (`store_unavailable` — fail closed) |
| `bot_distributed_claim_released_total{module,kind}` | Terminal transitions (release + handoff) |
| `bot_distributed_claim_expired_total{module}` | Expired leases found at takeover time |
| `bot_distributed_claim_takeover_total{module}` | Takeovers of expired claims (emitted by all three stores) |
| `bot_distributed_claim_renewal_failed_total{module}` | Rejected-or-failed renewals |
| `bot_distributed_fencing_rejected_total{module}` | Money continuations blocked by a lost claim |
| `bot_distributed_claim_terminal_lost_total{module}` | Terminal transitions refused because ownership was lost mid-work (fenced during execution) |
| `bot_distributed_flag_sync_applied_total` / `…_errors_total` | Flag convergence activity/failures |
| `bot_replica_info{replica,backend}` | Static identity gauge (always 1) |

Logs are structured (`execution_id`, `replica`, `epoch`, `takeover_count`,
`previous_owner`, `backend`), so the full ownership history of any logical
execution is reconstructable from the claim table + logs (§S): who claimed
it, when the lease lapsed, who took over, which generation was fenced, and
how each generation ended.

## 9. Configuration

```toml
[ha]
replica_id = ""                  # empty = generated {hostname}-{pid}-{rand8}
claim_lease_secs = 45            # floor 5
claim_handoff_grace_secs = 900   # floor 60
flag_sync_secs = 5               # floor 1
book_sync_secs = 30              # floor 5
```

Env overrides: `HA_REPLICA_ID`, `HA_CLAIM_LEASE_SECS`,
`HA_CLAIM_HANDOFF_GRACE_SECS`, `HA_FLAG_SYNC_SECS`, `HA_BOOK_SYNC_SECS`.

**Deployment checklist for >1 replica:**

1. `[database] enabled = true` (authoritative claims + flags + book). Redis
   alone works but is lease-only; memory store + multiple replicas is a
   misconfiguration (warned loudly at startup).
2. Unique `replica_id` per replica (or leave empty for generated ids).
3. NTP-synced clocks.
4. Same `[ha]` lease/grace across replicas (a replica with a longer lease
   than its peers' assumption only slows takeover; a shorter one risks
   fencing itself mid-work — keep them equal).
5. Watch `bot_distributed_fencing_rejected_total` and
   `bot_distributed_claim_expired_total`: steady non-zero values mean
   leases are too short for your execution latency, or a replica is
   unhealthy.

## 10. Testing (§W/§X/§Y)

Deterministic units (injected clock, no sleeps) in
`crates/core/src/ownership.rs` and `state.rs`; live-store integration in
`crates/core/tests/` (gated on `POSTGRES_URL` / `REDIS_URL`, run with
`--test-threads=1`):

* **Units**: two/three-replica single winner; same-owner reclaim rejected;
  expiry → takeover → stale generation fenced (verify/renew/release);
  repeated takeovers (epoch 3, takeover_count 2, lineage); release
  re-acquirable vs handoff grace (blocks even the owner, post-grace
  acquirable); renewal extends lease; renewal budget bounded;
  `run_guarded` renews during slow work; registry + fence + renew fail
  closed on store errors (fault injection via `FailingStore`/`FlakyStore`);
  permit glue (Unmanaged/Lost/Owned); flag sync rules (kill ON immediate,
  OFF/flags staleness-gated); book merge rules (insert / newer-overwrites /
  never resurrect terminal); replica id configured-vs-generated; status
  string wire contract.
* **Postgres integration**: single owner + loser sees holder; 8-way
  concurrent race → exactly one winner; expiry takeover + fencing + audit
  lineage; release/handoff grace; renewal extends past original expiry;
  runtime-flags roundtrip.
* **Redis integration**: two-replica single owner; expiry takeover +
  fencing; release/handoff grace; renewal; flags roundtrip.
* **Two-context integration (`distributed_integration.rs`, §X)**: two fully
  independent contexts (own PG pool, own Redis connection, own AppState,
  own registry) sharing the same servers — PG claim race, Redis claim race,
  kill-switch propagation A→B (incl. `may_broadcast` actually blocked on B
  and module-flag convergence), book sync A→DB→B plus cross-context exit
  claim on the converged position id.
* **Two-replica MODULE-layer test (`module-copy/tests/two_replica_mirror.rs`,
  gap-8 closure)**: two independent `CopyBot` instances (own AppState, own
  registry, shared Postgres) receive the SAME whale trade concurrently —
  exactly one passes the claim gate (proven by it reaching the network
  stage against a dead port), the loser skips silently, the shared claim
  row + `execution_claim_events` lineage name the winner, and the
  pre-broadcast failure released the claim cleanly.
* **Gap-closure additions**: `GlobalRiskOracle` unit semantics (tightens
  capacity/daily-loss, "unknown" falls back to local, never loosens);
  Postgres claim-event lineage (`acquired → takeover → fenced → released`
  across two generations); `PostgresRiskOracle` count/PnL queries against
  real rows.

Fault injection (§Y) is deterministic and non-destructive: failing/flaky
store wrappers at the registry, guard and flag-sync layers; no test kills
shared infrastructure or relies on production-like defaults being
destructive.

## 11. Known limits (honest)

1. **Lease-based fencing window** (§4): a pathological pause between
   `fence()` and broadcast can still allow a takeover; the journal +
   reconciliation + handoff grace + chain-level balance checks are the
   compensating controls. There is no epoch-checked storage endpoint on
   Solana/Polymarket to close this fully.
2. **Redis-only deployments** lose claim state on Redis restart (epoch-1
   re-acquisition possible). Postgres removes this.
3. **Flag sync is periodic** (`flag_sync_secs`): kill propagation across
   replicas is bounded by that interval, not instant. The local replica
   applies the kill synchronously.
4. **Capacity/PnL convergence**: CLOSED by the `GlobalRiskOracle` (per-check
   shared-DB view, §7) — a residual millisecond-scale race between two
   replicas' concurrent count queries and inserts remains (claims still
   prevent duplicate executions of the SAME intent). The position book sync
   remains the mechanism for position DETAIL convergence (exit identities,
   local checks); `realized_today` carries the documented close-day
   attribution approximation.
5. **Sandbox test caveat**: the integration environment runs PG/Redis with
   durability off — it proves SQL/Lua/protocol/logic, not crash-durability
   of the stores themselves.
6. Claim ROW lineage is one generation deep (`previous_owner`/
   `takeover_count` — the row is overwritten by design to keep acquisition
   atomic); on Postgres the full multi-generation history lives in
   `execution_claim_events`; Redis/memory deployments have logs only.
