# Crash Recovery (TASK 6)

What happens when a worker dies at any point of the trading pipeline, what
a restart does about each unfinished order, and what a takeover reconciles.
Companion documents: [HA-ARCHITECTURE.md](HA-ARCHITECTURE.md) (workers,
leases, fencing, cursors) and
[DISTRIBUTED-OPERATIONS.md](DISTRIBUTED-OPERATIONS.md) (runbook).

The previous recovery documents remain valid and are the module-level
detail: [POLYMARKET-RECOVERY.md](POLYMARKET-RECOVERY.md),
[COPY-TRADING-RECOVERY.md](COPY-TRADING-RECOVERY.md),
[RECONCILIATION.md](RECONCILIATION.md),
[ACCOUNTING-LEDGER.md](ACCOUNTING-LEDGER.md) §8. This document is the
cross-cutting matrix.

---

## 1. The rule

> A crash must never turn into a second execution, a fabricated fill, or a
> silently dropped event.

Three invariants make that hold:

1. **Write-ahead, then act.** The `Submitted` row of the execution ledger is
   written BEFORE the send. Its absence therefore proves nothing was
   broadcast; its presence proves only that a send *started*.
2. **Unavailable ≠ absent.** A venue that cannot be read is never evidence
   that an order does not exist. It yields `HoldAmbiguous`, never a
   finalize and never a resubmit.
3. **Re-booking is safe, re-sending is not.** Every financial effect is
   keyed (`AccountingEvent::event_id`, `poly_fills.fill_id`, OMS
   `idempotency_key`), so replaying recovery is idempotent — while there is
   no action in the vocabulary that re-sends an order.

---

## 2. Crash boundaries (§5)

`bot_core::ha::CrashBoundary` names the twelve boundaries and maps each to
the local evidence a restart finds. With the venue not yet read (startup),
each resolves to exactly one action:

| Boundary | Local evidence at restart | Action (venue unread) | Why |
|---|---|---|---|
| before persistence | absent | `no_action` | nothing exists anywhere |
| after persistence | journaled, not sent | `close_unsent` | the submit row is written before the send |
| before risk decision | absent | `no_action` | no intent was journaled |
| after risk decision | journaled, not sent | `close_unsent` | decision made, nothing broadcast |
| before submission | journaled, not sent | `close_unsent` | provably nothing left the process |
| after submission | submitted, unknown | `hold_ambiguous` | may or may not have landed |
| before venue ack | submitted, unknown | `hold_ambiguous` | same |
| after venue ack | open | `hold_ambiguous` | local says open; the venue must confirm |
| before fill accounting | open | `hold_ambiguous` | the fill may exist on the venue |
| after fill accounting | terminal | `no_action` | already booked; the event id stops a re-book |
| during reconciliation | open | `hold_ambiguous` | the sweep simply runs again |
| during recovery | open | `hold_ambiguous` | recovery is idempotent and re-runs |

Once the venue answers, the same boundaries finalize deterministically —
e.g. a crash after submission with a venue `Filled` becomes
`finalize_filled`, with a partial becomes `resume_tracking`, and a crash
before submission whose order nevertheless rests on the venue (the send
landed before the journal write) becomes `adopt_from_venue`.

---

## 3. The order recovery matrix (§6)

`plan_order_recovery(local, venue)` is pure and total: every pair has
exactly one answer, and the same pair always gives the same answer.

| # | Situation | Local | Venue | Action |
|---|---|---|---|---|
| A | never submitted | absent | absent | `no_action` |
| B | journaled, not submitted | journaled_not_sent | absent / unavailable | `close_unsent` |
| C | submitted, response unknown | submitted_unknown | absent / unavailable | `hold_ambiguous` (+ reconcile) |
| D | venue acknowledged, local missing | absent / journaled_not_sent / terminal | open / partial | `adopt_from_venue` |
| E | partially filled | open / submitted_unknown | partial | `resume_tracking` |
| F | fully filled | any live | filled | `finalize_filled` |
| G | cancelled | any live | cancelled | `finalize_cancelled` |
| H | expired | any live | expired | `finalize_expired` |

Extra rules worth naming:

* **acknowledged-then-vanished** (`local open`, `venue absent`) is
  `hold_ambiguous`, not a failure — the TASK 4 verification pass found this
  case and it is preserved here.
* **local terminal, venue reports a fill** is `finalize_filled`: the ledger's
  event id makes the re-book a duplicate if it was already booked, and
  recovers the money if it was not.
* There is no `resubmit`. Ever.

Each resolved order is journaled in `ha_recovery_records`
(worker, generation, trigger, scope, subject, action, reason) and audited as
`ha.recovery.action`.

---

## 4. What a restart rebuilds

Startup order in `crates/server/src/main.rs`:

1. Stores attached (`accounting::attach`, `ha::attach`) — before anything
   reads or writes durable state.
2. `persist::restore` — positions, OMS orders (non-terminal → `Unknown`),
   unresolved transactions, the execution ledger (`Created`/`Validated` →
   failed-unsent; `Submitted`/`Pending` → kept live and enqueued for
   reconciliation).
3. `accounting::recover` — the global ledger replays `ledger_events` through
   the same idempotent path, rebuilding the book, the realized/peak series
   (daily-loss and drawdown inputs) and the duplicate index; module
   positions without ledger history are reported as gaps, never synthesised.
4. `ha::register_and_recover` — registers this worker life, then journals
   one deterministic action per unfinished order, plus scope records for the
   ledger and the cursors; ends in `Ready` (or `RecoveryRequired`).
5. Workers start; the modules start last.

Because step 4 runs before the venue has been read, it can only ever
`close_unsent` (provably nothing sent) or `hold_ambiguous` (everything else)
— the reconciliation worker and the module poll loops finalize later from
venue truth.

---

## 5. Failover reconciliation (§8)

After a takeover, the new owner reconciles the four record layers it now
owns before treating anything as settled:

| Layer | Compared by | Findings |
|---|---|---|
| orders | `ha_recovery_records` + OMS + execution ledger | one action per unfinished order |
| fills → ledger | TASK 5 accounting reconciliation (`orders → fills → ledger → positions`) | `missing_ledger_entry`, `duplicate_ledger_entry`, `orphan_accounting_event`, `unresolved_financial_event` |
| positions | module positions vs the aggregated book | `position_mismatch`, `quantity_mismatch` |
| risk state | rebuilt from `ledger_events` (realized series, peak, exposure) | drawdown / daily-loss gating restored before the first decision |
| cursors | `ha_cursors` + `ha_feed_gaps` | resume position, unresolved gaps |

Unexplained differences are **reported, never repaired**: the finding stays
until a backfill, a venue read or an explicit operator correction
(`POST /api/accounting/events` with `kind = correction`) resolves it.

---

## 6. Tests

`crates/core/tests/ha_distributed.rs`:

| Test | Proves |
|---|---|
| `worker_registration_generations_and_heartbeat_expiry` | identity, generations, zombie detection, stale survey takes nothing over |
| `lease_acquire_renew_loss_takeover_and_fencing` | acquire / renew / expiry / takeover, stale owner fenced on every path, guarded work does not run |
| `two_workers_race_for_one_lease_exactly_one_wins` | the race |
| `store_failure_is_never_ownership` | fail closed |
| `two_workers_cannot_execute_create_or_book_the_same_thing_twice` | one claim, one order intent, one ledger effect |
| `critical_proof_same_event_two_workers_one_execution_one_intent_one_ledger_effect` | the §17 critical proof end to end |
| `feed_cursor_recovery_gap_detection_and_replay` | durable cursors, duplicate suppression, gap detection, restart resume, deliberate replay, scoped cursors |
| `every_crash_boundary_has_one_deterministic_outcome` | all twelve boundaries |
| `restart_rebuilds_ledger_positions_and_risk_state_without_double_booking` | ledger / position / risk-state restart |
| `partial_fill_survives_a_restart_and_keeps_the_booked_part` | partial-fill restart |
| `recovery_records_are_journaled_and_auditable` | the recovery journal |
| `after_takeover_the_new_owner_reconciles_and_reports_findings` | failover reconciliation, old owner fenced |
| `single_worker_mode_needs_no_lease_to_be_ready` | single mode |
| `active_passive_standby_is_not_ready_until_it_owns_the_role` | active/passive |
| `readiness_fails_on_lost_lease_pending_recovery_and_unhealthy_dependency` | readiness failure modes incl. `RECOVERY_REQUIRED` |
| `graceful_shutdown_persists_cursors_releases_leases_and_stops` | graceful shutdown |
| `worker_state_machine_rejects_illegal_transitions` | the state machine |

Plus 37 unit tests in `crates/core/src/ha/*` (state matrix, lease
semantics, cursor arithmetic, the full recovery matrix, the memory store).

---

## 7. Limitations

* Recovery decisions at startup are taken with the venue unread by design;
  the finalizing read happens in the reconciliation worker and the module
  poll loops, which need their venues reachable.
* The PostgreSQL `HaStore` path is covered by a gated test only (no
  database in the build sandbox).
* `during_reconciliation` / `during_recovery` assume the sweep is
  restartable — it is, because every step is idempotent, but a sweep that
  crashes repeatedly will keep re-reporting the same findings until the
  underlying cause is fixed.
* Nothing here has been exercised against a live venue, a funded wallet or
  a real multi-host cluster.
