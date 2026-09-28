# Copy-Trading Recovery

What the copy engine does after a crash or restart, what it will never do,
and how to verify the result. It builds on the generic execution recovery
in [RECONCILIATION.md](RECONCILIATION.md) (ledger hydration, signature
truth sources, claim reconciliation) — nothing here replaces those steps;
this document covers the copy-specific state layered on top: the leader
registry, the processed-event journal, the leader ↔ follower links.

**Invariant: recovery never broadcasts a transaction.** It re-marks dedup
keys, raises ordering cursors, closes or rebuilds bookkeeping rows and, for
one well-defined case (an entry the ledger proves never landed), closes a
position **without selling** because there is nothing to sell.

---

## 1. Durable state and where it lives

| State | Store | Survives restart? |
|---|---|---|
| leader status + counters | `copy_leaders` (Postgres) / `MemoryCopyStore` | yes with Postgres; config re-seeds membership either way |
| processed events (stage, reason, intent, position) | `copy_events` | yes with Postgres |
| leader ↔ position links | `copy_links` | yes with Postgres |
| dedup keys | dedup facade (Redis / Postgres) or in-memory | yes with a durable facade; otherwise **re-seeded from `copy_events`** |
| ordering cursors | in memory | re-seeded from `copy_events` |
| execution intents | execution ledger (hydrated from `execution_lifecycle`) + intent journal | yes (generic recovery) |
| positions | position book (`positions` table) | yes (generic recovery) |

Without Postgres and without Redis, the copy engine has no memory across
restarts beyond the position book, exactly as before TASK 3.

---

## 2. Crash points

Read top to bottom as "where in `process_event` the process died".

| # | Crash point | Evidence after restart | Recovery action | Money at risk |
|---|---|---|---|---|
| A | before the dedup claim | nothing | none; the feed redelivers and the event is decided normally | none |
| B | after the claim, before broadcast (policy / sizing / risk / permit) | in-memory claim lost; journal row absent or `REJECTED` | with a durable dedup facade the event stays decided; otherwise the redelivery is decided again (same inputs → same decision, unless the market moved and staleness now refuses it) | none — nothing was sent |
| C | after the write-ahead intent record, before broadcast | intent journal row `pending`, no ledger signature | generic: the intent reconciler parks the orphan and gates the symbol; copy: nothing to do | none |
| D | after broadcast, before fill bookkeeping | ledger record `submitted` / `pending` with a signature; no position | **`HoldAmbiguous`**: never resubmit; the ledger's `resolve_after_restart` + the signature truth source decide; if it landed, the position is recreated by generic recovery from the ledger/trade, and the next copy reconciliation `RestoreLink`s it | the mirrored size, resolved by reconciliation |
| E | after fill bookkeeping, before the link / journal write | position with `copied_wallet`, no link, journal row missing or `RISK_APPROVED` | **`RestoreLink`** from the position (leader = `copied_wallet`, entry signature from the position, event id from the journal when present) | none |
| F | entry provably never landed (ledger `failed` / `expired` after the crash) but a position was booked | position whose `entry_signature` the ledger settled as failed | **`CleanupFailedEntry`**: qty and cost basis zeroed, position closed as `Failed`, link closed — no sell (no tokens were received) | none |
| G | position closed (sweeper / operator) but the link stayed `open` | closed or missing position, open link | **`CloseLink`** (`closed` / `orphaned`) | none |
| H | during a mirrored exit | ledger exit intent live | generic recovery owns the exit signature; copy reconciliation closes the link once the position is gone | the exit's proceeds, resolved by reconciliation |

Plus, always:

* **`SeedDedup`** — every event journaled inside
  `copy.recovery_lookback_hours` has its dedup key re-marked, so a feed
  backlog replayed after the restart is refused as `DUPLICATE_EVENT` even
  when the dedup facade is in-memory.
* **`SeedCursor`** — each leader's ordering cursor is raised to the newest
  journaled slot, so `strict_ordering` treats the replayed backlog as late,
  not as new.
* Leader counters are restored from `copy_leaders` (larger view wins);
  membership and the paused flag are re-derived from config.

---

## 3. The restart procedure (what `run()` does)

1. `sync_leaders(cfg)` — registry from config; transitions journaled.
2. `recover_after_restart(cfg)`:
   1. `load_leaders()` → restore counters (or log that the journal is
      unavailable);
   2. `events_since(now − lookback)` → `SeedDedup` + `SeedCursor`;
   3. `open_links()`, the copy positions (open and closed) and the
      execution ledger's copy records → `plan_recovery` (pure, deterministic)
      → apply each action, one `copy_recovery_actions_total{action}` and one
      `copy.recovery.<action>` audit record each;
3. spawn the TP/SL sweeper — only now, so a `CleanupFailedEntry` position is
   never swept as if it held tokens;
4. start consuming the feed.

The plan is idempotent: running it twice repairs nothing the second time
(`HoldAmbiguous` is reported again while the ledger record stays live —
that is a statement of fact, not an action).

---

## 4. Verifying a recovery

```text
grep 'copy restart recovery applied'            # actions count
grep 'copy dedup re-seeded from journal'        # seeded=N journaled=M
grep 'mirrored entry still live'                # HoldAmbiguous — expect follow-up from the execution reconciler
grep 'failed copy entry cleaned up'             # CleanupFailedEntry
```

Metrics: `copy_recovery_actions_total{action=~"seed_dedup|seed_cursor|hold_ambiguous|cleanup_failed_entry|restore_link|close_link"}`.

Journal:

```sql
-- links vs book after restart
select l.position_id, l.status, p.status as position_status
  from copy_links l left join positions p on p.id = l.position_id
 where l.status = 'open' and (p.id is null or p.status not in ('open', 'closing'));
-- should be empty after the first reconciliation pass

-- held ambiguous entries
select x.intent_id, x.state, x.signature, x.updated_at
  from execution_lifecycle x
 where x.module = 'copy' and x.label like 'copy-%' and x.label not like 'copy-exit%'
   and x.state in ('created', 'validated', 'submitted', 'pending');
```

An ambiguous entry stays held until the execution reconciler proves the
signature landed or not (RECONCILIATION.md §"ambiguity matrix"). Do not
sell it manually before then: if the entry never landed there are no
tokens, and if it did the sweeper will manage it once the position is
restored.

---

## 5. Reconciliation (continuous)

`reconcile_once` runs every `copy.reconcile_interval_secs` after startup
and compares three views: the open links, the position book and the
journaled leader activity inside the lookback window, plus the execution
ledger for ambiguity. Findings and actions:

| Finding | Action | Sells? |
|---|---|---|
| `leader_exited_we_hold` | `Flag`, or `MirrorExit` when `reconcile_auto_exit && mirror_exits` | only with auto exit, through `exit::sell_position` under an ownership permit (`exit:{position}:recon_exit`) |
| `leader_removed_we_hold` | `Flag` | no |
| `link_without_position` | `CloseLink` | no |
| `quantity_mismatch` | `UpdateLink` | no |
| `orphan_position` | `Flag` | no |
| `ambiguous_entry` | `Flag` | no |

"Leader exited" means: a journaled leader **sell** on the mint after the
link opened, with no later leader **buy**. Activity is what the pipeline
observed — a leader sell that reached this process while exits were off or
the leader was paused is journaled as `REJECTED` with its reason and still
counts as activity. A sell that never reached this process (feed outage) is
not known; the next leader trade on the mint or a manual check is required.

---

## 6. Manual procedures

**Force-close a link whose position you closed by hand**

```sql
update copy_links set status = 'closed', closed_at = now(), note = 'manual close',
       updated_at = now()
 where position_id = $1 and status = 'open';
```

(The next reconciliation pass would do the same.)

**Replay the decision history of one leader trade**

```sql
select event_id, stage, reject_reason, detail, intent_id, position_id, created_at, updated_at
  from copy_events where signature = $1;
```

**Re-seed dedup after restoring a database backup**

Nothing to do: the next restart re-seeds from `copy_events` within the
lookback window. To widen the window once, raise
`copy.recovery_lookback_hours` (or `COPY_RECOVERY_LOOKBACK_HOURS`) before
the restart.

**A leader trade was mirrored twice**

That requires two different `event_id`s for the same swap — i.e. the
decoder produced different (leader, signature, mint, side) tuples — or two
replicas without a shared dedup facade and without shared ownership. Check
`copy_events` for the two rows; check `execution_lifecycle` for the two
intents (their ids differ only if the mint or side differed). Both
positions are legitimate book entries; sell one manually if desired. The
cross-replica guard is the ownership claim `copy:{leader}:{mint}`, which
requires the shared claim store (DISTRIBUTED.md).

---

## 7. What recovery will not do

* It never resubmits a transaction, never rebuilds a "missing" mirror for a
  leader trade that was decided before the crash, and never sells on a
  finding unless `reconcile_auto_exit` is on.
* It does not rewrite the position book except for `CleanupFailedEntry`
  (ledger-proven non-landing) — the same rule the sniper's sweeper applies.
* It does not consult the chain for the leader's holdings.
* It does not undo a decision made under a transient failure (e.g. a
  `BALANCE_UNAVAILABLE` event stays decided). If that matters operationally,
  the leader trade can be re-injected only through a **new** event id,
  which the engine deliberately does not provide.
