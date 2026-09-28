# Copy-Trading Engine (Module 2)

This document describes the copy-trading engine delivered in TASK 3: how a
leader's on-chain trade becomes (or does not become) a mirrored position,
which component owns which decision, what is persisted, and every knob and
metric an operator can use. The companion documents are
[COPY-TRADING-OPERATIONS.md](COPY-TRADING-OPERATIONS.md) (runbook) and
[COPY-TRADING-RECOVERY.md](COPY-TRADING-RECOVERY.md) (crash points, restart
procedure, reconciliation findings).

Nothing in this document is a claim of profitability or safety. Mirroring a
wallet is a strategy with well-known adverse cases (front-running by the
leader, MEV, stale fills, leader exits you cannot follow). The engine's job
is to make each decision deterministic, bounded, observable and recoverable —
not to make it profitable.

---

## 1. Scope and design rules

The engine is an **extension of the existing `module-copy` crate**. The
feeds (`feeds.rs`), the legacy `mirror_trade` door and the execution paths
(`mirror.rs`: bonding curve and Jupiter), the TP/SL sweeper and the shared
sell path (`exit.rs`) are kept and wired into a staged pipeline
(`CopyBot::process_event`, `event.rs`). The same non-negotiables that govern
the sniper govern this module:

| Rule | Where it holds |
|---|---|
| One authoritative risk decision | `bot_core::risk::RiskEngine` — `check_copy_coded` (copy-specific) + `check_entry` (generic + copy caps). `policy.rs` never sizes or decides risk. |
| One authoritative dedup | `event_dedup::claim` → `AppState::mark_copy_event_seen` (namespace `copy_event`). The feeds' `mark_signature_seen` is fetch suppression only. |
| Never bypass the execution engine | Every buy/sell goes through `solana_kit::execute::Executor` with a deterministic intent id, the execution ledger, the fee policy and the write-ahead intent journal. |
| Replays never trade | `EventSource::Replay` is refused at the policy stage (`REPLAY_ONLY`); nothing else in the crate can send a transaction for a replayed event. |
| Paper by default | Unchanged: `execution.mode = "paper"` builds and books a paper fill; nothing is broadcast. |
| Cross-replica safety | Unchanged: entries claim `copy:{leader}:{mint}`, exits claim `exit:{position}:{rule}` through `OwnershipRegistry`; store failures fail closed. |

### Pre-existing defect fixed

Before TASK 3, `feeds.rs::emit_signature` marked every decoded signature as
seen (`mark_signature_seen`) **and** `CopyBot::run` marked the same key again
before calling `mirror_trade`. The second mark returned `false`, so every
event delivered by the `logs_poll` and `transaction_subscribe` feeds was
dropped as a duplicate; only PumpPortal (which does not pre-mark) ever
reached the mirror. The pipeline now has its own namespace
(`AppState::seen_copy_events` / dedup facade namespace `copy_event`) keyed by
the **event** (`copy:{signature}:{leader}:{mint}:{side}`), and `run()` no
longer touches the `sig` namespace. The feeds' behaviour is unchanged: their
`mark_signature_seen` remains fetch suppression (decode a transaction once),
and every event they hand over is marked seen **exactly once** by
`event_dedup::claim` inside the pipeline. Regression tests:
`tests/dedup_ordering.rs::feed_signature_marks_never_starve_the_pipeline`,
`tests/geyser_feed.rs::geyser_feed_pushes_a_whale_buy_into_the_copy_channel`
(a Geyser-delivered trade is fresh for the pipeline after the feed's mark)
and `tests/copy_feed.rs::copy_feed_subscribes_tracked_wallets_and_maps_trades`
(PumpPortal deliveries are decided once, buy and sell of one mint are
distinct events).

---

## 2. Component map

```
crates/module-copy/src
├── lib.rs             CopyBot: wiring, run loop, sync_leaders, recover_after_restart, reconcile_once
├── feeds.rs           PumpPortal / logs_poll / transaction_subscribe → WalletTrade   (behaviour unchanged)
├── event.rs           LeaderTradeEvent, EventSource, EventDefect, CopyStage (15), RejectReason (29), CopyOutcome
│                      + CopyBot::process_event — the staged pipeline — and its exit stage (mirror_exit)
├── leader.rs          LeaderRegistry: follow / pause / resume / unfollow, config sync, counters
├── event_dedup.rs     claim / already_seen / seed — the one authoritative dedup
├── event_ordering.rs  OrderingTracker: per-leader slot cursor, strict/advisory, sequence gaps
├── policy.rs          evaluate(event, leader, [copy], ctx) → Enter | Exit | Skip     (pure)
├── sizing.rs          size_mirror(rule, [copy], leader_sol, balance) → SizeDecision   (pure)
├── intent.rs          entry_intent_id / entry_label (unchanged ids), exit_intent_id / exit_label (hardened), journal_record
├── mirror.rs          mirror_trade (legacy door → process_event), buy / buy_on_curve / buy_via_jupiter / record_buy
├── exit.rs            ExitSweeper (TP/SL/trailing/max-hold) + sell_position (uses intent::exit_intent_id)
├── reconcile.rs       reconcile(links, book, activity, ledger) → findings + actions    (pure)
├── recovery.rs        CopyStore trait, MemoryCopyStore, plan_recovery                  (pure planner)
├── metrics.rs         copy_* counters / gauges / histograms, LatencyTimeline
└── audit.rs           copy.entry.* / copy.exit.* / copy.leader.* / copy.recon.* / copy.recovery.*
crates/core
├── migrations/0013_copy_trading.sql   copy_leaders, copy_leader_events, copy_events, copy_links
├── src/db/copy.rs                     CopyRepo + record types (used by the server's DbCopyStore)
├── src/risk.rs                        copy_* controls, check_copy_coded, leader_exposure, 7 new RiskCodes
├── src/config.rs                      [copy] pipeline keys, [[copy.wallets]] keys, [risk] copy_* keys
└── src/state.rs                       mark_copy_event_seen / copy_event_seen / seen_copy_event_count
crates/server/src
├── recon.rs                           DbCopyStore (CopyStore over CopyRepo)
└── main.rs                            copy.with_copy_store(...) when Postgres is configured
```

---

## 3. The event model

`LeaderTradeEvent` (`event.rs`) is the canonical shape every stage works on.
Feeds still produce `bot_core::models::WalletTrade`; `CopyBot::event_from_trade`
lifts it, assigning a per-bot delivery number (`source_sequence`) and the
`EventSource` derived from `[copy].feed`.

| Field | Meaning |
|---|---|
| `event_id` | `cev_` + 32 hex of `sha256("v1|copy-event|leader|signature|mint|side")`. Same on every replica and across restarts. A token-for-token route yields two events (one per mint). |
| `leader`, `signature`, `slot`, `block_time` | The leader's transaction. `slot = 0` / `block_time = None` when the source has none. |
| `side` | `Long` = leader bought `mint`, `Short` = leader sold. |
| `mint`, `symbol`, `venue`, `token_amount`, `sol_amount`, `fee_sol`, `discriminator` | Decoded swap. |
| `source`, `source_sequence` | Which feed, and its delivery number (for gap detection). |
| `observed_at` | When this process first saw it. |

Derived: `dedup_key()` (`copy:{signature}:{leader}:{mint}:{side}`), `event_at()`
(chain time, else observation), `age_secs(now)`, `detection_lag_ms()`
(observation − chain time), `to_wallet_trade()` (lossless for every field the
risk engine and the legacy code read).

`validate(now)` runs once at the top of the pipeline and names the defect:
`empty_leader`, `empty_signature`, `empty_mint`, `mint_is_leader`,
`non_finite_amount`, `negative_amount`, `zero_size`, `invalid_fee`,
`future_timestamp` (chain time > now + 60 s), `foreign_venue` (Polymarket).

---

## 4. Leader lifecycle

`[copy].wallets` is the source of truth for **membership and rules**;
`leader.rs` adds a lifecycle and counters on top.

```
           follow                pause               unfollow
 (none) ───────────► ACTIVE ◄──────────► PAUSED ───────────► REMOVED ── follow ──► ACTIVE
                                resume
```

| State | Buys | Sells | Counted / journaled / reconciled |
|---|---|---|---|
| `ACTIVE` | mirrored (policy + risk permitting) | mirrored when `mirror_exits` and not `buys_only` | yes |
| `PAUSED` | refused `LEADER_PAUSED` | **still mirrored** (reducing exposure is always allowed) | yes |
| `REMOVED` | refused `LEADER_REMOVED` | refused `LEADER_REMOVED` (the sweeper's TP/SL keeps managing the position) | reconciliation flags `leader_removed_we_hold` |

Transitions are driven by config (hot reload): `CopyBot::sync_leaders` runs
`LeaderRegistry::sync_from_config` on every received event and every
reconciliation tick. New wallets are `followed`, missing ones `unfollowed`,
`paused = true/false` toggles `paused` / `resumed`, any other rule change is
`rule_changed`. Every transition is journaled (`copy_leader_events` with the
replica id), audited (`copy.leader.<event>`) and metered
(`copy_leader_events_total{event}`); the leader row (`copy_leaders`) is
upserted with status and counters.

Counters per leader: `events_seen`, `mirrored` (filled or ambiguous
entries), `rejected`, `last_event_at`, `last_slot`, `last_rejection`. On
restart they are restored from the journal (the larger view wins); a row for
a wallet that is no longer configured is ignored.

---

## 5. Pipeline stages and rejection reasons

`CopyBot::process_event` (`event.rs`; the legacy `CopyBot::mirror_trade` in
`mirror.rs` lifts a raw `WalletTrade` into an event and calls it):

```
RECEIVED → VALIDATED → LEADER_RESOLVED → DEDUPLICATED → ORDERED → POLICY_PASSED
         → SIZED → RISK_APPROVED → OWNERSHIP_CLAIMED → SUBMITTED → FILLED | AMBIGUOUS
                                                                   (sells: → EXIT_MIRRORED)
terminal on refusal: REJECTED (rule) | FAILED (error before/during execution)
```

| # | Stage | Check | Rejection |
|---|---|---|---|
| 1 | `RECEIVED` | shape (`validate`) | `INVALID_EVENT` (detail = defect) |
| 2 | `VALIDATED` | leader in registry | `LEADER_UNKNOWN` (quiet: not journaled, no dedup consumed) |
| 3 | `LEADER_RESOLVED` | authoritative dedup claim | `DUPLICATE_EVENT` (quiet: not journaled twice, not a leader rejection) |
| 4 | `DEDUPLICATED` | ordering (`strict_ordering`) | `OUT_OF_ORDER` |
| 5 | `ORDERED` | policy (§6) | `LEADER_REMOVED` `LEADER_PAUSED` `REPLAY_ONLY` `VENUE_DISABLED` `BELOW_LEADER_MIN` `STALE_EVENT` `SYMBOL_GATED` `SNIPER_HOLDS` `ALREADY_MIRRORING` / sells: `MIRROR_EXITS_DISABLED` `SELLS_NOT_MIRRORED` `NO_POSITION_TO_EXIT` |
| 6 | `POLICY_PASSED` | spendable balance, sizing (§6) | `BALANCE_UNAVAILABLE` (FAILED), `ZERO_SIZE`, `DUST_SIZE` |
| 7 | `SIZED` | risk gate 1 `check_copy_coded` | `KILL_SWITCH` `STRATEGY_DISABLED` `COPY_COOLDOWN` `STALE_EVENT` `LEADER_EXPOSURE` `EXPOSURE_LIMIT` `RISK_REJECTED` |
| 8 | `SIZED` | risk gate 2 `check_entry` (publishes `RiskRejected`) | `SLIPPAGE_LIMIT` `EXPOSURE_LIMIT` `RISK_REJECTED` … |
| 9 | `RISK_APPROVED` | ownership permit | `OWNERSHIP_UNAVAILABLE` (FAILED, fail closed), `OWNERSHIP_LOST` |
| 10 | `OWNERSHIP_CLAIMED` | execute (`buy` → curve / Jupiter) | `EXECUTION_FAILED` (FAILED; the mint is noted for `copy_failed_entry_cooldown_secs`) |

The `WalletTrade` dashboard event is published exactly once per unique
event (after dedup). `Signal` is published once the risk engine approved.
`RejectReason::from_risk_code` maps the risk engine's `RiskCode` onto the
pipeline vocabulary; the risk engine remains the authority, the pipeline
only chooses the label.

Routine rejections (`is_routine()`: duplicates, unknown leaders, policy
skips such as `BELOW_LEADER_MIN`, cooldowns, ownership lost, replay) are
journaled and metered but **not** audited, to keep the audit trail
actionable. Everything else terminal is audited (`copy.entry.<stage>` /
`copy.exit.<stage>`).

`CopyBot::mirror_trade(&WalletTrade, &Config)` — the pre-TASK-3 entry point —
is kept and runs the same pipeline; it returns `Err` only for `FAILED`.

---

## 6. Policy and sizing

**Policy** (`policy.rs`, pure) answers *should this be mirrored at all?* in
the documented order (first failing rule wins). Staleness has two knobs:
the wallet rule's `max_staleness_secs` (seconds since **we observed** the
trade, as before) and the global `[copy].max_event_age_secs` (seconds since
**chain time**, falling back to observation; `0` = off). Venue decoders
(`decode_pumpfun` / `decode_pumpswap` / `decode_raydium` / `decode_jupiter`)
are now enforced here (previously configured but unused). A leader's
**sell** ignores size, staleness and venue flags: if we hold the mint and
exits are allowed, the exit is mirrored — fully (`full_exit_on_their_exit`)
or proportionally (`leader tokens sold ÷ our qty`, clamped to a full close).

**Sizing** (`sizing.rs`, pure) turns the leader's SOL into a *request*:

1. base — `fixed_sol` (> 0) else `leader_sol × fraction_of_their_size`
   (identical to the pre-TASK-3 `size_for`);
2. wallet cap `max_sol`;
3. `[copy].max_sol_per_trade`, then `[copy].max_balance_fraction × spendable`;
4. floors — finite, positive, and ≥ `[copy].min_mirror_sol`.

`SizeDecision` records the mode (`fixed` / `proportional`), the base, the
request and which caps actually bound. The risk engine may still reduce the
request (`AllowReduced`): the booked position's cost basis is the **approved**
size, and both numbers appear in the outcome, the journal and the audit line.

`max_token_age_secs` remains accepted for config compatibility but is not
enforced: the event carries no token creation time (unchanged from before
TASK 3; listed under limitations).

---

## 7. Risk controls (the one authoritative decision)

All copy-specific controls are evaluated by `bot_core::risk::RiskEngine`;
the pipeline passes facts and labels the answer.

**Gate 1 — `check_copy_coded(trade, risk, requested_sol, leader_caps)`**

| Check | Code | Config |
|---|---|---|
| kill switch, module disabled, daily loss, `copy_emergency_disable`, copy daily loss | `KillSwitch` `ModuleDisabled` `DailyLossLimit` `CopyEmergencyDisabled` `CopyDailyLoss` | `risk.copy_emergency_disable`, `risk.copy_daily_loss_limit_quote` |
| leader size > 0 | `InvalidSize` | — |
| per-leader-per-mint cooldown | `CopyCooldown` | `risk.copy_cooldown_secs` (unchanged) |
| whale trade staleness | `StaleSignal` | `copy_cooldown_secs.max(30)` (unchanged rule) |
| open exposure mirrored from this leader + request | `CopyLeaderExposure` | `risk.copy_max_leader_exposure_quote` (global) and the wallet's `max_exposure_sol` (tightens) |
| open positions mirrored from this leader | `CopyLeaderExposure` | wallet `max_open_positions` |

`check_copy` (the pre-TASK-3 method) is kept and delegates with
`requested_sol = 0` (exposure checks skipped), preserving its behaviour.

**Gate 2 — `check_entry(EntryRequest{module: Copy, …})`** (unchanged order:
preflight → size → slippage → open positions → duplicate symbol → re-entry
cooldown → **copy throttles** → balance → caps → venue), with the copy
counterparts of the sniper's controls:

| Config (`[risk]`) | Effect | Code |
|---|---|---|
| `copy_max_position_quote` | per-entry cap (`0` = `max_position_quote`), never above the generic cap | reduces size |
| `copy_max_total_exposure_quote` | envelope over open copy exposure (`0` = generic) | `ExposureCap` |
| `copy_max_concurrent_positions` | open copy positions (`0` = `max_open_positions`) | `MaxOpenPositions` |
| `copy_max_pending_executions` | live copy **entry** intents in the ledger (`copy-*` labels, never `copy-exit-*`) | `CopyPendingCap` |
| `copy_failed_entry_cooldown_secs` | after a failed mirrored entry on a mint | `CopyFailedEntryCooldown` |

`RiskCode` gained `CopyEmergencyDisabled`, `CopyDailyLoss`, `CopyPendingCap`,
`CopyFailedEntryCooldown`, `CopyLeaderExposure`, `CopyCooldown`, `StaleSignal`;
`is_exposure_limit()` includes the first three copy codes. Failed-entry
timestamps are shared with the sniper (`AppState::note_failed_entry`); their
retention TTL now covers the copy cooldown as well.

---

## 8. Intents, execution and ownership

* **Entry intent id** (`intent.rs`): `intent_id(["copy", leader signature,
  leader, "buy", route, mint])` — the same parts as before TASK 3, so ids do
  not change across the upgrade. Labels: `copy-<mint8>` (curve),
  `copy-jup-<mint8>` (Jupiter).
* **Exit intent id** (`intent.rs::exit_intent_id`, consumed by `exit.rs`
  for the sweeper, the mirrored-exit stage and the reconciler's auto-exit):
  `intent_id(["copy", position id, mint, open time (unix ms), "sell", route,
  raw quantity sold, quantity held])`. Mint + open time are the hardening
  (the sniper's scheme): position counters restart with the process, so two
  positions can share an id string but never an exit identity. A retry of
  the same sell maps onto the same ledger record; the next partial exit
  (smaller holding) gets a fresh id. Labels unchanged: `copy-exit-<mint>`
  (curve), `copy-exit-jup-<mint>` (Jupiter) — `intent::exit_label`.
* The execution ledger refuses a second live attempt at the same intent
  (`EXECUTION_FAILED` with a "duplicate" detail, nothing broadcast).
* Write-ahead journal: `IntentSink::record` before broadcast, `link` after,
  `abandon` on a definite pre-broadcast failure — unchanged.
* Ownership: the permit is fenced immediately before broadcast; ambiguous
  outcomes (`Sent` / `SendUnknown` in live mode) park the claim for
  reconciliation, everything else releases it.

Outcome bookkeeping per fill: `Trade` + `Position` (`copied_wallet` set,
SL/TP from `risk.default_*`), `mark_copied`, `copy_links` row
(`entry_event_id`, `entry_signature`, `intent_id`, leader tokens, our qty),
`copy_events` row (`FILLED` / `AMBIGUOUS` with intent + position ids).

---

## 9. Ordering

`OrderingTracker` keeps one cursor per leader (highest processed slot + its
signature). Verdicts: `in_order`, `same_slot` (intra-slot order unknown →
process), `unknown_slot` (slot 0 → process, cursor unchanged),
`out_of_order` (behind the cursor). `[copy].strict_ordering = false`
(default) processes late events and counts them; `true` refuses them as
`OUT_OF_ORDER`. Sequence gaps (a numbered source skipped deliveries) are
counted (`copy_ordering_total{verdict="gap"}`) and logged, never rejected —
the answer to a gap is reconciliation, not dropping what did arrive. The
cursor is seeded from the journal at restart and dropped when a leader is
unfollowed.

---

## 10. Persistence (migration `0013_copy_trading.sql`)

| Table | Purpose | Written by |
|---|---|---|
| `copy_leaders` | leader row: status, source, followed_at, status_since, counters | `sync_leaders`, every finished event |
| `copy_leader_events` | append-only lifecycle history with replica id | `sync_leaders` |
| `copy_events` | one row per decided event (stage, reason, intent, position); `created_at` = first seen | `finish` (not for duplicates / unknown leaders) |
| `copy_links` | follower position ↔ leader entry; `open` → `closed` / `orphaned` / `mismatch` | fill, mirrored exit, reconciliation, recovery |

`recovery::CopyStore` is the trait the engine talks to; the server injects
`DbCopyStore` (over `bot_core::db::copy::CopyRepo`) when Postgres is
configured, otherwise `MemoryCopyStore` gives identical semantics for the
process lifetime. Writes are best effort (`copy_journal_errors_total{op}`),
reads answer `None` when the backend is unavailable so recovery degrades
explicitly.

---

## 11. Metrics

| Series | Kind | Labels |
|---|---|---|
| `copy_events_total` | counter | `source`, `side` |
| `copy_stage_total` | counter | `stage` |
| `copy_rejections_total` | counter | `reason`, `stage` |
| `copy_dedup_total` | counter | `outcome` = `fresh` / `duplicate` / `seeded` |
| `copy_ordering_total` | counter | `verdict` = `in_order` / `same_slot` / `unknown_slot` / `out_of_order` / `gap` |
| `copy_leader_events_total` | counter | `event` |
| `copy_leaders` | gauge | `status` |
| `copy_leader_exposure_sol_milli` | gauge | `leader` |
| `copy_recon_findings_total` | counter | `kind` |
| `copy_recovery_actions_total` | counter | `action` |
| `copy_journal_errors_total` | counter | `op` |
| `copy_detection_lag_ms` | histogram | — |
| `copy_<stage>_latency_ms` (`validated`, `deduplicated`, `ordered`, `policy`, `sized`, `risk`, `claimed`, `filled`, `rejected`) | histogram | — |
| `copy_total_latency_ms` | histogram | — |
| `copy_mirror_size_sol_milli` | histogram | — |

Existing series keep their meaning: `bot_symbol_gated_entries_total{module="copy"}`,
`bot_module_queue_depth{module="copy"}`, the module counters
(`inc_signals`, `inc_risk_rejected`, `inc_orders_*`).

---

## 12. Configuration reference

`[copy]` (all new keys default to the pre-TASK-3 behaviour):

| Key | Default | Meaning |
|---|---|---|
| `max_event_age_secs` | `30` | global ceiling on event age (chain time), `0` = off |
| `strict_ordering` | `false` | refuse `OUT_OF_ORDER` events |
| `max_sol_per_trade` | `0` | global cap on one mirrored buy |
| `max_balance_fraction` | `0` | global cap as a fraction of the spendable balance (`0..=1`) |
| `min_mirror_sol` | `0` | dust floor |
| `reconcile_interval_secs` | `60` | reconciliation cadence, `0` = never |
| `reconcile_auto_exit` | `false` | sell when reconciliation finds the leader fully exited (needs `mirror_exits`) |
| `recovery_lookback_hours` | `24` | re-seed dedup / cursors from the journal after a restart, `0` = off |

`[[copy.wallets]]`: `paused` (`false`), `max_exposure_sol` (`0`),
`max_open_positions` (`0`) in addition to the existing keys.

`[risk]`: `copy_max_position_quote`, `copy_max_total_exposure_quote`,
`copy_max_concurrent_positions`, `copy_max_pending_executions`,
`copy_failed_entry_cooldown_secs`, `copy_daily_loss_limit_quote`,
`copy_max_leader_exposure_quote` (all `0` = inherit / off),
`copy_emergency_disable` (`false`).

Environment overrides: `COPY_MAX_EVENT_AGE_SECS`, `COPY_STRICT_ORDERING`,
`COPY_MAX_SOL_PER_TRADE`, `COPY_MAX_BALANCE_FRACTION`, `COPY_MIN_MIRROR_SOL`,
`COPY_RECONCILE_INTERVAL_SECS`, `COPY_RECONCILE_AUTO_EXIT`,
`COPY_RECOVERY_LOOKBACK_HOURS`, `COPY_EMERGENCY_DISABLE`,
`COPY_MAX_POSITION_SOL`, `COPY_MAX_TOTAL_EXPOSURE_SOL`,
`COPY_MAX_CONCURRENT_POSITIONS`, `COPY_MAX_PENDING_EXECUTIONS`,
`COPY_FAILED_ENTRY_COOLDOWN_SECS`, `COPY_DAILY_LOSS_LIMIT_SOL`,
`COPY_MAX_LEADER_EXPOSURE_SOL`.

Validation (`validate_copy_engine`): non-negative finite numbers,
`max_balance_fraction ∈ [0, 1]`, per-wallet `max_exposure_sol ≥ 0` and
`max_staleness_secs ≥ 0`; warnings for `reconcile_auto_exit`,
`reconcile_interval_secs = 0`, every wallet paused, `copy_emergency_disable`,
and a copy cap above the generic cap.

---

## 13. Tests

| File | Covers |
|---|---|
| `tests/leader_lifecycle.rs` | config seeding, hot-reload sync, journal + audit per transition, state gating in the pipeline, invalid transitions, counter restore |
| `tests/event_pipeline.rs` | full paper pipeline (position, ledger, journal, link, events, audit, metrics), live terminal states in one flow (`FILLED`, `AMBIGUOUS`, `EXIT_MIRRORED` under the hardened exit id, `REJECTED`, `FAILED`; all 15 stage counters), malformed events, every policy reason, risk gates and labels, legacy `mirror_trade`, FAILED vs REJECTED |
| `tests/dedup_ordering.rs` | cross-feed duplicates, the feed-mark regression, one signature → distinct events, advisory vs strict ordering, per-leader cursors, gaps, seeding |
| `tests/geyser_feed.rs`, `tests/copy_feed.rs` (pre-existing) | feed behaviour unchanged, plus: a feed-delivered trade is fresh for the pipeline's `copy_event` mark exactly once (feed `sig` marks never drop events as duplicates) |
| `tests/policy_sizing.rs` | precedence table, sizing matrix and degenerate inputs, approved size → position, partial exits |
| `tests/intent_execution.rs` | live mode through the executor (deterministic intent, journal record/link/abandon), ledger duplicate guard, simulation failure, ambiguous broadcast, node rejection, pending cap |
| `tests/reconciliation.rs` | clean pass, sweeper-closed links, leader exit flagged then auto-mirrored, re-buy cancels, drift/orphans/ambiguous, unfollowed leader |
| `tests/crash_recovery.rs` | replayed backlog after restart, hold ambiguous / cleanup failed / repair links, idempotency, recovery before new events |
| `tests/concurrency.rs` | six workers × one event → one position; eight distinct events → eight positions; racing mirrored exits sell once |
| unit tests in every new module | pure logic (event, dedup namespaces, ordering, policy, sizing, intent ids, leader registry, reconcile, recovery planner, memory store, metrics timeline, audit sanitising) |
| `crates/core` | risk: copy codes / caps / leader exposure / throttles; config: defaults, bounds, warnings, TOML keys; `db_integration::pg_copy_journal_roundtrip` (gated on `POSTGRES_URL`) |

All of the above run offline against the sniper's mock JSON-RPC node
(`tests/common/mod.rs` includes `module-sniper/tests/common/mod.rs` by
path). Nothing in `src/` depends on test code.

---

## 14. Known limitations

* `max_token_age_secs` is accepted but not enforced (no token creation time
  in the event). Unchanged from before TASK 3.
* Leader **pause / resume / unfollow** are config-driven (hot reload). There
  is no API or Telegram command for them in this task; the registry exposes
  `snapshot()` for a future read-only endpoint.
* Reconciliation compares our own records (links, book, journaled leader
  activity, execution ledger). It does not read the chain for the leader's
  current holdings; a leader exit we never observed (feed outage) is not
  detected until the leader trades again.
* The in-memory dedup is process-local; restart safety without Redis relies
  on the journal re-seed (`recovery_lookback_hours`). With no database and no
  Redis, a replayed backlog older than the process is decided again — as
  before TASK 3.
* An event decided while a transient failure was in effect (balance read
  failed, ownership store down) is not reconsidered when redelivered; this
  matches the sniper's one-decision-per-event semantics and is documented in
  the operations guide.
* Sizing and risk work in SOL; USD-denominated caps are out of scope.
