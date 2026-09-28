# Polymarket Recovery

What survives a crash of the Polymarket engine, what the process does when
it starts again, how to verify that recovery did the right thing, and what it
deliberately will **not** do. Companion to
[POLYMARKET-ENGINE.md](POLYMARKET-ENGINE.md) (§8 lifecycle, §9
reconciliation, §10 persistence) and
[POLYMARKET-OPERATIONS.md](POLYMARKET-OPERATIONS.md).

The guiding rule is the same as for the other engines: **after a restart the
engine must never book a fill twice, never invent a fill or a cancel, and
never place a new order on a token whose venue state it does not know.**
Where the venue's answer is missing, the order is held as `unknown` and
reconciliation — not a retry — decides.

---

## 1. Durable state and where it lives

| State | Store | Written when |
|---|---|---|
| intent → outcome | `poly_signals` (upsert by `signal_id`) | every `process_signal` outcome, including rejections |
| venue order lifecycle (`state`, `size_matched`, `position_id`, `replica_id`, `closed_at`) | `poly_orders` (upsert by `venue_order_id`) | on submit, on **every** applied observation (poll, user channel, recon), on every local transition |
| booked fill deltas | `poly_fills` (insert-once by `fill_id`) | before the position is updated; a duplicate `fill_id` aborts the booking |
| reconciliation findings | `poly_recon_findings` (append) | every finding |
| OMS order (`Created … Filled/Failed/Unknown`), executions | `orders`, `order_executions` (TASK 1) | stage 8 onwards; `external_id` = venue order id, `signature` = order signature |
| positions and trades | `positions`, `trades` (existing persistence layer) | `AppState::record_trade` on every fill delta |
| ownership claims | `OwnershipRegistry` store (Postgres / Redis / memory) | `poly:entry:<token>` claimed before the POST, handed off (grace window) after |

Without Postgres all of the above is in memory (`MemoryPolyStore`, in-memory
OMS). Restart recovery then has nothing to adopt; this configuration is for
paper mode only.

---

## 2. Crash points

Stage numbers refer to the pipeline table in the engine document (§5).

| Crash between | Persisted | On restart |
|---|---|---|
| 1–7 (before the OMS order) | a `poly_signals` row at the last stage | nothing to recover; the next scan re-evaluates and, if the decision is still valid, produces the **same** intent (same `signal_id`) |
| 8 `IDEMPOTENT` and 10 `SIGNED` | OMS order `Created` **without** `external_id` (the venue id is attached at `SIGNED`) | the OMS restore marks it `Unknown`; recovery **fails** it (`FailedUnsent`: it was never signed, so it cannot be on the venue). Nothing is re-issued; the next scan may produce the same decision again |
| 10 `SIGNED` and the `POST` | OMS `Submitted` with `external_id` and signature; `poly_orders` row `submitted` (written **before** the POST — write-ahead); ownership claim | adopted and **held as `unknown`** (`HeldAmbiguous`); post-recovery reconciliation asks the venue: found → `resolved_<state>` and matched size booked; 404 → `marked_failed`. The claim's lease expires on its own |
| the `POST` and the first observation | as above plus `OrderSent` event consumed by persistence | same as above; if the venue accepted the order it will be found in the open list |
| observations (resting for minutes/hours) | `poly_orders.size_matched` = everything booked so far, `poly_fills` rows, position updated | adopted with its `size_matched` (`AdoptedJournalOrder`); the next poll books only the delta between the venue's cumulative and the journaled amount — **no double booking** even if the process died between the position update and the journal upsert (the `fill_id` for a cumulative observation is deterministic and insert-once) |
| our cancel request and the venue's confirmation | order still `resting` locally | the next poll sees `canceled` and finishes it; if the venue lost it, reconciliation marks it `unknown` (accepted before) |
| a paper submit and its in-process fill | `poly_orders` row `submitted` with mode `paper`, OMS `Submitted` | `FailedStalePaper`: the paper order is failed (paper fills are atomic with the submit; an open paper row can only mean the process died in between). Nothing is re-filled |
| shutdown with `cancel_on_shutdown` | cancels confirmed so far are `cancelled` | remaining live orders are adopted as usual and cancelled by TTL/reprice or by you |

The user-channel and market websocket subscriptions are not persisted; they
are re-established from the tracked orders after recovery.

---

## 3. The restart procedure (what `run()` does)

1. Load the signer (if any) and derive/load the API key (live only).
2. `recover_after_restart()`:
   1. **Journal first.** Every `poly_orders` row with `closed_at IS NULL`
      that this process does not already track is re-created as a
      `TrackedOrder` with its journaled `state`, `size_matched`,
      `position_id`, `replica_id` and — when the OMS still has the order —
      its `signature`. Rows in `submitted`/`unknown` are forced to `unknown`
      (`HeldAmbiguous`); everything else is `AdoptedJournalOrder`. Paper rows
      are failed (`FailedStalePaper`) in both the journal and the OMS.
   2. **Then the OMS.** Incomplete Polymarket orders (`incomplete()`) that
      carry a live venue id but have no journal row (older releases, rows
      created by the persistence layer from `OrderSent` events) are adopted
      as `unknown` from the OMS metadata (`AdoptedOmsOrder` +
      `HeldAmbiguous`) and journaled. Incomplete orders that **cannot** be on
      the venue — no venue id (never signed) or paper/simulate mode — are
      transitioned to `Failed` (`FailedUnsent`) so they stop counting as open
      and are not reloaded as `Unknown` on the next start. Orders of other
      modules are never touched.
   3. Every action is metered (`poly_recovery_actions_total{action}`) and
      audited (`poly.recovery.<action>`).
   4. **Post-recovery reconciliation** (live, credentials present, at least
      one action): `reconcile_once` compares the adopted set with the venue's
      open orders and resolves the held ones (engine doc §9). Its findings
      are part of the `RecoveryReport`.
3. Start the tickers: scan, order poll, reconciliation; connect the user
   channel and the heartbeat when credentials exist.

Recovery is **idempotent**: a second call adopts nothing (everything is
already tracked) — `tests/crash_recovery.rs::journal_orders_are_readopted_with_their_booked_quantity`,
`::never_sent_oms_orders_are_failed_instead_of_held`. The write-ahead
ordering (journal row before the POST) is asserted by
`::the_venue_claim_is_journaled_before_the_post`.

Adopted orders participate in every gate immediately: a new decision on the
same token is `ORDER_ALREADY_OPEN`/`ALREADY_IN_MARKET`, their unfilled
notional counts as resting exposure, and they are polled and cancelled by TTL
like orders placed in this life.

**Replayed fills after a restart.** An adopted order comes back without the
in-memory list of venue trade ids it had booked; the `poly_fills` journal
(`fill_id = trade:<id>`) is the authority instead. A user-channel `trade`
re-emitted after the restart (`MINED`/`CONFIRMED` for a trade booked before
it) is recognised by that row and discarded — the local cumulative, the
position and the OMS status do not move, the id is remembered, and
`bot_duplicate_execution_prevented_total{where="poly_fill_journal"}` counts
the replay. A cumulative poll whose fill row already exists (crash between
the fill insert and the order-snapshot upsert) advances the local snapshot
to venue truth without booking the ledger twice. The per-trade sum
(`trade_matched`, engine doc §8.2) also restarts at zero: until the first
poll that names its trades (`associate_trades`) re-bases it, a new
user-channel trade is remembered but booked only when the poll confirms the
cumulative — safe under-booking for one poll interval, never a double
booking (`tests/crash_recovery.rs::fills_replayed_after_restart_are_booked_once`).

---

## 4. Verifying a recovery

Logs:

```
polymarket restart recovery complete adopted=<n> held=<n> cleaned=<n>
```

(`cleaned` counts `failed_stale_paper` + `failed_unsent`.)

followed, in live mode, by `poly.recon.*` audit records for anything the
venue disagreed with.

Queries:

```sql
-- what was adopted / held / failed in the last start
select ts, action, target from audit_log
where action like 'poly.recovery.%' and ts > now() - interval '10 minutes'
order by ts;

-- held orders still unresolved
select venue_order_id, token_id, state, size_matched, submitted_at, updated_at
from poly_orders where closed_at is null and state in ('unknown', 'submitted');

-- fills booked more than once for one order (must be empty)
select venue_order_id, count(*) from poly_fills
group by venue_order_id, fill_id having count(*) > 1;

-- journal vs OMS disagreement on terminal state (should converge after one poll)
select p.venue_order_id, p.state, o.status
from poly_orders p join orders o on o.id = p.order_id
where p.closed_at is not null and o.status not in ('filled','cancelled','expired','failed','reconciled');
```

Expected after a clean recovery: no rows in the second query after a couple
of reconciliation passes; the third query is always empty; the fourth is
empty once the OMS has followed the terminal states (an ambiguous order that
was confirmed *resting* stays OMS-`Unknown` until it terminates — that is by
design, see engine doc §8.1).

Metrics: `poly_recovery_actions_total{action}` increments once per adopted
order; `poly_open_orders` equals the number of open journal rows;
`poly_recon_findings_total{kind="ambiguous_submit_resolved"}` accounts for the
held ones.

---

## 5. Reconciliation (continuous)

Recovery is a one-shot; `reconcile_once` runs every
`reconcile_interval_secs` for the life of the process and is what makes the
"held as unknown" strategy safe. Its findings, in the order they are
produced:

1. **Orphans** — venue open orders unknown locally → `reported`, or, when
   `reconcile_cancel_orphans = true`, `cancelled` only if the venue's answer
   names the id in `canceled` (`cancel_refused` / `cancel_unconfirmed` /
   `cancel_failed` otherwise — retried next pass).
2. **Local orders vs venue** — for every tracked non-terminal live order:
   present in the open list → matched-size drift is booked
   (`matched_size_mismatch`, only when the venue reports a size), ambiguous
   ones are resolved (`ambiguous_submit_resolved`); absent →
   `GET /data/order` decides between `resolved_<state>` (fills booked from
   venue truth), `marked_failed` (the venue never acknowledged the order —
   `submitted`/`unknown` with no venue status — and it is definitively
   absent) and `marked_unknown` (acknowledged before — resting, partially
   filled, or a `matched` FAK whose quantity was never read — now missing;
   kept for the next pass, which marks it `failed` if still absent, with
   nothing booked).
3. **Positions without orders** — `reported` only; the engine never sells.
4. **Stale orders** — TTL/GTD passed but not cancelled → `reported`
   (polling keeps retrying the cancel).

Paper mode reconciles against the position book only and never calls the
venue (`tests/reconciliation.rs::paper_mode_reconciliation_never_touches_the_venue`).

**Held quantity vs chain.** Independently of the module, the server's
recovery worker re-arms a `polymarket_position:<id>` claim for every open
LIVE Polymarket position every `recovery.position_recheck_interval_secs`
(only when `[polymarket].ctf_rpc_url` is configured) and resolves it against
the funder's settled outcome-token balance (ERC-1155 `balanceOf`) with the
same engine, tolerance and correction policy as Solana positions
(`docs/RECONCILIATION.md` §9). Divergences are flagged as risk events
(`drift_flag`, `unexpected_position`, `recon_correction`); an unresolved
claim entry-gates the token (or, unattributable, Module 3) until it
resolves. A resting or partially matched order on the same token suspends
the comparison — expected divergence, no correction.

---

## 6. Manual procedures

* **An order stuck in `unknown` for more than a few passes.** The venue
  neither lists it nor answers `GET /data/order`. Check the venue UI under the
  same API key. If it is gone, wait: a later pass returning 404 for an order
  that was once accepted keeps it `unknown` on purpose (the engine cannot
  tell "cancelled by the venue" from "API lag"). If you are certain, cancel
  via the venue API (a `canceled` confirmation finishes it) or close the OMS
  order through your usual operator tooling; do not edit `poly_orders`
  directly — the next observation would overwrite it.
* **Recovery adopted an order you do not want.** Cancel it in the venue UI;
  the next poll finishes it locally. Or set `order_ttl_secs` low for one
  restart.
* **Recovery reported `FailedStalePaper` in live mode.** Harmless: a paper
  order from an earlier paper session was still open in the journal. It is
  closed and never filled.
* **Journal and venue disagree on `size_matched` after every poll.** The
  venue payload is inconsistent (cumulative going down). The engine ignores
  lower cumulatives and refuses cumulatives above the order size; look at the
  raw venue answer before touching anything.
* **Database restored from a backup older than the last run.** Orders placed
  after the backup have no journal row but do have OMS rows only if the OMS
  database is the same one — if both are older, the venue's open list is
  the only truth: start with `reconcile_cancel_orphans = false`, read the
  `orphan_venue_order` findings, and cancel manually what should not rest.

---

## 7. What recovery will not do

* It does not re-submit anything. An intent that never reached the venue is
  failed; the next scan may produce the same decision again (in the same
  process life the OMS still remembers the key and reports
  `DUPLICATE_INTENT`; after the next restart the terminal order is not
  reloaded and the decision goes through).
* It does not guess fills. An accepted order the venue no longer reports is
  `unknown`, not `filled` and not `cancelled`.
* It does not sell or redeem. Positions are closed by market resolution or
  by an operator.
* It does not cancel orphans unless `reconcile_cancel_orphans = true`.
* It does not trust the OMS over the journal: the journal's `size_matched` is
  the booked amount; the OMS status follows it, never the other way round.
* It does not run against the venue in paper mode, and it does not run at
  all in live mode without credentials (orders are adopted and held; the
  first successful reconciliation after the key is available resolves them).
