# Sniper engine — architecture, lifecycle, protocols, controls, replay, runbook

This document describes Module 1 (`crates/module-sniper`) **as implemented and
tested** after the "production-grade sniper engine" pass. Everything here is
backed by code in the crate and by a test named in §12. Nothing in this
document is a claim that a token is safe, that a launch is a good trade, or
that the module is profitable: the engine implements *measurable* checks with
*machine-readable* verdicts and leaves the trading decision to the operator's
configuration.

Related documents: `docs/EXECUTION-RELIABILITY.md` (the execution engine the
sniper submits through), `docs/RECONCILIATION.md` (source-of-truth model and
ambiguity handling), `docs/MODULES.md` (operator-level module guide).

---

## 1. Architecture

```
feeds ──► detect.rs ──► mpsc<LaunchEvent> ──► entry.rs (pipeline) ──► Executor ──► ledger/recon
 │            │                                   │   │   │
 │            │                                   │   │   └── risk::RiskEngine (one authority)
 │            │                                   │   └────── market.rs → gates.rs / slippage.rs
 │            │                                   └────────── pipeline.rs (stages, reasons, latency)
 │            └── event.rs (normalisation, deterministic ids, shape rules)
 └── PumpPortal WS · Solana logsSubscribe (per protocol) · Geyser transactionSubscribe

exit.rs (sweeper, independent of the feeds) ──► same Executor / ledger / journal / ownership
replay.rs (fixture-driven; no RPC, no executor, no ledger) ──► same precheck/gates/slippage/risk
```

| File | Responsibility |
|---|---|
| `src/event.rs` | Unified `LaunchEvent` (+ `LaunchProtocol`, `EventDefect`, `SequenceTracker`). Pure. |
| `src/detect.rs` | Feed adapters that emit `LaunchEvent`s: PumpPortal, `logsSubscribe` (pump.fun / PumpSwap / Raydium AMM v4), Geyser `transactionSubscribe`. Reconnect / gap / ordering metrics. |
| `src/pipeline.rs` | Stage vocabulary (`SniperStage` + transition table), `RejectReason` codes, `Lifecycle`, `LatencyTimeline`, `EntryRoute`, `select_route`, `precheck` (checks 1–6, shared with replay), counters. Pure. |
| `src/market.rs` | The only I/O between "validated" and "risk approved": one fresh chain read per protocol producing a protocol-neutral `MarketSnapshot` plus the venue context the transaction builders need. |
| `src/gates.rs` | Safety gates over a `MarketSnapshot` (pass / fail / skip), `GateReport`. Pure. |
| `src/slippage.rs` | Slippage engine (`fixed` / `liquidity_aware` / `price_impact`), price-impact model, hard maximum. Pure, `u128` arithmetic. |
| `src/entry.rs` | The staged entry pipeline (`Sniper::consider_event`), legacy `consider_launch` door, deterministic entry intent ids, audit events, latency metrics. |
| `src/exit.rs` | Exit sweeper (`ExitTracker`, failed-entry cleanup, ambiguous hold, venue-aware mark and sell, stale rule, retry backoff, deterministic exit intent ids). |
| `src/replay.rs` | Deterministic replay engine over recorded fixtures; never submits. |
| `src/lib.rs` | `Sniper` struct wiring (state, rpc, wallet, executor, risk, layouts, signers, intents, ownership, `exits: ExitTracker`), run loop, re-exports. |

Reused (not duplicated) infrastructure: `bot_core::risk::RiskEngine` (the one
risk decision), `bot_core::state::AppState::mark_launch_seen` (the one
authoritative dedup), `bot_core::execution` (ledger, `intent_id`, state
machine), `solana_kit::execute::Executor` (build → simulate → submit →
confirm, fee policy, provider failover), the write-ahead intent journal, the
distributed ownership permit, `AppEvent::Audit` for the decision trail and
`bot_core::obs::metrics` for every metric.

---

## 2. Unified launch event (`LaunchEvent`)

Every detection source is normalised into one record before the pipeline sees
it.

| Field | Populated from | Notes |
|---|---|---|
| `event_id` | computed | `evt_` + 32 hex chars of `digest("launch|{protocol}|{mint}|{pool}|{reference}")`; `reference` is `sig:<signature>` when known, else `slot:<slot>`, else `seq:<source>:<source_seq>`. Two feeds that see the same creation transaction compute the same id. |
| `protocol` | adapter | `pump_fun` \| `pump_swap` \| `raydium_amm_v4` |
| `source` | adapter | existing `LaunchFeed`: `pumpportal` \| `logsSubscribe` \| `transactionSubscribe` \| `manual` (replay fixtures carry whatever feed was recorded) |
| `slot`, `signature` | notification / transaction | `None` when the source does not carry them (PumpPortal has a signature, no slot). |
| `mint`, `creator`, `pool` | event decode | `pool` is required for the AMM protocols (`EventDefect::MissingPool`); the pump.fun curve is derived from the mint. |
| `base_mint`, `quote_mint` | event decode | `base_mint` must equal `mint`; quote is WSOL for every supported launch type. |
| `liquidity_quote_lamports`, `liquidity_base_raw`, `initial_price_sol`, `base_decimals` | what the source knows | optional; the decision uses the fresh `MarketSnapshot`, never these alone. |
| `event_ts`, `source_ts`, `observed_at` | block time / feed timestamp / local clock | `effective_ts()` = first available of the three; staleness is measured from it. |
| `source_seq` | `SequenceTracker` per feed connection | monotonic per source; used with the slot to detect reordering after a reconnect. |
| `raw_hash` | `raw_hash_of(payload)` | audit/replay reference to the raw payload; never empty. |
| `launch: TokenLaunch` | legacy record | name, symbol, metadata URI, market cap, creator buy, socials — consumed unchanged by the existing screening rules. |

**Shape rules** (`validate_shape(now)` → `EventDefect`, surfaced as
`INVALID_EVENT`): empty id, non-pubkey mint/creator/pool/quote, missing pool
for an AMM protocol, base ≠ mint, malformed signature (must decode to 64
bytes), slot 0, non-finite liquidity/price, `observed_at` more than
`MAX_CLOCK_SKEW_SECS` (5 s) in the future, `event_ts` after `observed_at`,
empty raw hash, and an `event_id` that does not match `compute_event_id()`.

**Consistency** (`consistent_with`): two events describing the same launch
must agree on protocol, mint, pool, and — when both carry them — slot and
signature.

**Dedup key** (`dedup_key`): the mint for pump.fun (compatible with dedup
stores populated before this change) and `{protocol}:{mint}` for AMM
launches, because a token can legitimately launch on the curve and later open
a pool (two events, two decisions). The key is consumed by exactly one
mechanism, `AppState::mark_launch_seen` (memory, Redis or Postgres backed,
as configured) — feeds never deduplicate on their own.

`LaunchEvent` is `serde` (de)serialisable; the replay fixtures are its JSON
form. `tests/replay.rs::fixture_json_round_trips_and_rejects_garbage` covers
the compatibility guarantee.

---

## 3. Lifecycle and validation pipeline

### 3.1 Stages

```
DETECTED → VALIDATED → RISK_APPROVED → EXECUTION_READY → SUBMITTED → CONFIRMED
   └──────────┴──────────────┴─────────────────┘ → REJECTED     └── FAILED
```

`SniperStage::can_transition_to` is the only legal transition table: forward
steps are strictly linear, `REJECTED` is legal from any pre-submission stage,
`FAILED` is legal once a transaction was built or submitted, nothing leaves a
terminal stage. An out-of-order step is itself a rejection with reason
`INVALID_STATE` rather than silent corruption. Every stage transition is
counted (`sniper_stage_total{stage,protocol}`), timestamped
(`LatencyTimeline`) and, at the end of the pass, written to the audit trail as
`AppEvent::Audit { actor: "sniper", action: "sniper.entry.<stage>", target:
"<protocol>:<mint>", outcome }` where `outcome` carries the reason, the intent
id, the position id, slippage, price impact, total latency, the stage path
and the gate summary.

`SUBMITTED` is left standing (with the position booked) when the executor
reports `Sent` / `SendUnknown`: reconciliation owns that outcome (see
`docs/RECONCILIATION.md`), and the exit sweeper holds the position until the
ledger settles it (§8). `CONFIRMED` is reached only for `Confirmed` and
`PaperFilled` results.

### 3.2 Checks, in order, with the reason each one emits

| # | Check | Reason on failure |
|---|---|---|
| 1 | event shape (§2) | `INVALID_EVENT` |
| 2 | protocol routable under the live config (`trade_pumpswap` / `trade_raydium` / `use_jupiter_fallback`) | `INVALID_ROUTE` |
| 3 | global kill switch / emergency halt | `KILL_SWITCH` |
| 4 | module enabled and `risk.sniper_emergency_disable = false` | `STRATEGY_DISABLED` |
| 5 | launch age ≤ `sniper.max_launch_age_secs` (measured from `effective_ts`) | `STALE_EVENT` |
| 6 | symbol not gated by unresolved reconciliation | `SYMBOL_GATED` |
| 7 | authoritative dedup (`AppState::mark_launch_seen(dedup_key)`) | `DUPLICATE_EVENT` |
| 8 | static screening (`RiskEngine::check_launch_with_lists`: denylists, socials, creator buy, market-cap ceiling, repeat offenders) | `RISK_REJECTED` |
| 9 | RPC provider pool not tripped (`Rpc::unhealthy()`) | `EXECUTION_UNAVAILABLE` |
| 10 | wallet balance readable | `EXECUTION_UNAVAILABLE` |
| 11 | venue readable (`market::load_market`): curve / pool account exists and decodes, route resolvable | `EXECUTION_UNAVAILABLE` / `INVALID_ROUTE` / `POOL_NOT_READY` |
| 12 | safety gates (§4) | `POOL_NOT_READY` / `TOKEN_STATE_INVALID` / `INSUFFICIENT_LIQUIDITY` / `CONCENTRATION_LIMIT` / `STALE_EVENT` |
| 13 | slippage engine finds an allowed tolerance (§5) | `SLIPPAGE_LIMIT` |
| 14 | modelled price impact ≤ `sniper.max_price_impact_bps` | `PRICE_IMPACT_LIMIT` |
| 15 | fee budget (`pipeline::check_fee_budget`): the execution engine's fee policy accepts `execution.priority_fee_micro_lamports` (`FeePolicy::would_refuse`), and the worst-case transaction fee ≤ `sniper.max_entry_fee_lamports` (`0` = no budget) | `FEE_LIMIT` |
| 16 | `RiskEngine::check_entry` (sizing, min reserve, exposure caps, concurrent/pending caps, cooldowns, daily loss, venue check) | `EXPOSURE_LIMIT` / `SLIPPAGE_LIMIT` / `KILL_SWITCH` / `STRATEGY_DISABLED` / `RISK_REJECTED` |
| 17 | distributed ownership claim for the intent | `OWNERSHIP_LOST` |
| 18 | transaction built for the selected route | `EXECUTION_UNAVAILABLE` |
| 19 | `sniper.max_entry_latency_ms` (observe → hand-off) and `sniper.max_snapshot_age_ms` at submit time; kill switch re-checked | `STALE_EVENT` / `KILL_SWITCH` |

Checks 1–6 are the pure `pipeline::precheck` and run identically in replay.
Stage boundaries: checks 1–8 run in `DETECTED` and passing them advances to
`VALIDATED`; checks 9–16 (market read, gates, slippage, price impact, fee
budget, risk decision) advance to `RISK_APPROVED`; checks 17–19 advance to
`EXECUTION_READY`; the hand-off to the executor is `SUBMITTED`, and a
`Confirmed` / `PaperFilled` result is `CONFIRMED`.

**Fee budget (check 15).** The estimate is deterministic — configuration and
policy only, no chain read — and identical in replay:
`total = 5 000 lamports (one signature) + ⌈priority × CU / 10⁶⌉ + Jito tip`,
where `priority` is the most the fee policy can settle on for the configured
request across the executor's attempts (`FeePolicy::max_payable`: the clamped
request escalated for `execution.send_retries` attempts in `fixed` mode,
`execution.fee_max_micro_lamports` in `adaptive` mode), `CU` is
`execution.compute_unit_limit` for the direct routes and the 1 400 000-CU
transaction maximum for a Jupiter-built transaction, and the tip is
`execution.jito_tip_lamports` when `execution.use_jito`. The estimate excludes
the token-account rent deposit (refundable, not a fee) and the venue's own
trading fee (priced through slippage / price impact). A priority fee the
engine's policy would refuse is reported here as `FEE_LIMIT` — before any
attempt is recorded or a cooldown starts — instead of surfacing later as a
failed submission; the engine's own `FeePolicy::decide` still runs at
submission time and remains the authority. The accepted estimate is carried
on the outcome (`fee_estimate_lamports`) and in the audit line
(`fee_est_lamports=`). Configuration validation warns at start-up when the
budget is below the fee of the configured first attempt (every entry would be
`FEE_LIMIT`).

### 3.3 Reason codes

`INVALID_EVENT`, `STALE_EVENT`, `DUPLICATE_EVENT`, `INSUFFICIENT_LIQUIDITY`,
`SLIPPAGE_LIMIT`, `PRICE_IMPACT_LIMIT`, `FEE_LIMIT`, `EXPOSURE_LIMIT`,
`RISK_REJECTED`, `EXECUTION_UNAVAILABLE`, `KILL_SWITCH`, `STRATEGY_DISABLED`,
`INVALID_ROUTE`, `INVALID_STATE`, `TOKEN_STATE_INVALID`, `POOL_NOT_READY`,
`CONCENTRATION_LIMIT`, `SYMBOL_GATED`, `OWNERSHIP_LOST`.

Risk-engine codes map onto this vocabulary in one place
(`RejectReason::from_risk_code`): `kill_switch` → `KILL_SWITCH`;
`module_disabled` / `sniper_emergency_disabled` → `STRATEGY_DISABLED`;
`slippage_cap` → `SLIPPAGE_LIMIT`; `max_open_positions`, `exposure_cap`,
`daily_loss_limit`, `sniper_daily_loss`, `sniper_pending_cap` →
`EXPOSURE_LIMIT`; everything else → `RISK_REJECTED`. The risk engine remains
the authority; the mapping only chooses the label.

Nothing is silently discarded: every rejection increments
`sniper_rejections_total{reason,stage,protocol}`, is logged with its detail,
and is written to the audit trail.

### 3.4 Routes

`EntryRoute` = `PumpCurve` | `PumpSwapDirect` | `RaydiumV4Direct` | `Jupiter`
(labels `curve` | `pumpswap` | `raydium` | `jupiter` in metrics, audit and the
intent id), chosen by `pipeline::select_route(protocol, cfg, curve_complete)`:

* pump.fun: the bonding curve while it is live; once complete, PumpSwap
  directly when `trade_pumpswap`, else Jupiter when `use_jupiter_fallback`,
  else `INVALID_ROUTE`;
* PumpSwap: direct when `trade_pumpswap`, else Jupiter fallback;
* Raydium AMM v4: direct when `trade_raydium`, else Jupiter fallback.

The route is part of the entry intent id (a curve buy and a Jupiter buy of the
same launch are different transactions) and is recorded on the position
(`venue`, `market_id`) so the exit path sells on the same venue.

---

## 4. Safety gates

Gates are pure functions over a `MarketSnapshot` — a protocol-neutral summary
of what the chain said about the token and its pool at one instant
(`market.rs` reads it fresh; never the warm account cache). Each gate yields
`pass`, `fail(detail)` or `skip(detail)`; skips mean the protocol/feed does
not expose the datum and become failures when `sniper.strict_gates = true`.
The full `GateReport` (every gate, not only the first failure) goes into the
audit outcome and `sniper_gate_results_total{gate,outcome}`.

| Gate id | What is measured | Config | Reason on fail |
|---|---|---|---|
| `pool_state` | venue accepts buys now (curve not complete; AMM status swappable; buys not disabled) | always on | `POOL_NOT_READY` |
| `pool_open_time` | Raydium `pool_open_time` ≤ now (skip when not read; pass for other protocols) | always on | `POOL_NOT_READY` |
| `mint_authority` | mint authority revoked (from the SPL mint account) | `require_mint_authority_revoked` | `TOKEN_STATE_INVALID` |
| `freeze_authority` | freeze authority revoked | `require_freeze_authority_revoked` | `TOKEN_STATE_INVALID` |
| `min_liquidity` | pricing reserve > 0 and quote liquidity ≥ `min_liquidity_sol` | `min_liquidity_sol` | `INSUFFICIENT_LIQUIDITY` |
| `price_sane` | spot price is a positive finite number | always on | `INSUFFICIENT_LIQUIDITY` |
| `decimals_sane` | base decimals ≤ 12 | always on | `TOKEN_STATE_INVALID` |
| `creator_concentration` | creator opening buy ≤ `max_creator_initial_buy_sol` (only PumpPortal reports it → skip elsewhere) | `max_creator_initial_buy_sol` | `CONCENTRATION_LIMIT` |
| `pool_supply_fraction` | share of total supply on the venue ≥ `min_pool_supply_fraction` (needs total supply) | `min_pool_supply_fraction` | `CONCENTRATION_LIMIT` |
| `snapshot_freshness` | snapshot age ≤ `max_snapshot_age_ms` | `max_snapshot_age_ms` | `STALE_EVENT` |

Unit tests for each gate and for the strict/skip semantics live in
`src/gates.rs`; integration coverage is in `tests/pipeline.rs`
(`token_state_and_liquidity_gates_reject_with_their_reasons`) and
`tests/failure_injection.rs`
(`unreadable_mint_is_skipped_by_default_and_rejected_under_strict_gates`,
`missing_liquidity_is_insufficient_liquidity`).

These gates verify on-chain facts at one instant. They do **not** detect
every malicious design, a later authority change, off-chain behaviour, or a
future liquidity withdrawal.

---

## 5. Slippage engine

`slippage::decide(SlippageInputs) -> Result<SlippageDecision, SlippageLimit>`
is one pure function.

* Base tolerance precedence: per-token override (`slippage_overrides_bps[mint]`)
  → per-protocol override (`pumpswap_slippage_pct` / `raydium_slippage_pct`,
  applied to the direct AMM routes) → strategy base (`slippage_pct`).
* Modes (`slippage_mode`):
  * `fixed` — the base tolerance, unchanged (legacy behaviour). An
    over-limit request is passed through so the risk engine rejects it
    (`slippage_cap` → `SLIPPAGE_LIMIT`): one authoritative decision.
  * `liquidity_aware` — widens the base to at least twice the modelled price
    impact (never below the base); refuses when that exceeds the hard max.
  * `price_impact` — modelled impact plus the base as a buffer; refuses when
    that exceeds the hard max.
* Hard maximum: `risk.max_slippage_bps`. No mode ever *asks* the executor for
  more than that; the adaptive modes report `SLIPPAGE_LIMIT` themselves when
  the pool is too thin for the size.
* Price impact model: `price_impact_bps(trade_lamports, quote_reserve_lamports)`
  on the constant-product pair in `u128` (curve: virtual SOL reserve; AMM:
  quote vault). The independent ceiling `sniper.max_price_impact_bps` rejects
  with `PRICE_IMPACT_LIMIT` (check 14) regardless of mode.
* Every decision is observed on `sniper_slippage_bps{mode}`.

Arithmetic guarantees (tested in `src/slippage.rs`, including a randomised
property test): zero liquidity, dust liquidity, `u64::MAX` trade and reserve
values, boundary values at exactly the hard maximum, and percent ↔ bps
conversions are all defined inputs — no overflow, no panic, no negative or
`NaN` tolerance.

---

## 6. Exposure and risk controls (one risk engine)

All sniper limits are evaluated inside `bot_core::risk::RiskEngine` (the same
engine every module uses). New `RiskCode`s: `sniper_emergency_disabled`,
`sniper_daily_loss`, `sniper_pending_cap`, `sniper_token_cooldown`,
`sniper_failed_entry_cooldown`. Every `RiskDecision` now carries its `code`.

| Control | Key (`[risk]`) | Semantics |
|---|---|---|
| max position per token | `sniper_max_position_quote` | per-entry SOL cap; `0` = `max_position_quote`. The generic cap still applies (the smaller wins). |
| max total sniper exposure | `sniper_max_total_exposure_quote` | cap on the sum of open sniper exposure; `0` = the generic `max_position_quote × max_open_positions` envelope. |
| max concurrent positions | `sniper_max_concurrent_positions` | `0` = `max_open_positions`; the smaller wins. |
| max pending executions | `sniper_max_pending_executions` (default 2) | live sniper entry intents in the execution ledger (created/validated/submitted/pending), read through `RiskEngine::pending_sniper_entries`; `0` = unlimited. |
| per-token cooldown | `sniper_token_cooldown_secs` | seconds between two entry **attempts** on the same mint (`AppState::note_entry_attempt`). |
| failed-entry cooldown | `sniper_failed_entry_cooldown_secs` (default 120) | refuse a mint after an entry that the chain rejected / expired / did not fill (`AppState::note_failed_entry`). |
| daily sniper loss | `sniper_daily_loss_limit_quote` | module-scoped realized loss (`AppState::daily_realized(BotModule::Sniper)`), evaluated in addition to `daily_loss_limit_quote`. |
| emergency disable | `sniper_emergency_disable` | refuses every new sniper entry (pipeline check 4 **and** `check_entry`); exits keep running. |
| global kill switch | `kill_switch` / `/api/kill` | check 3, `check_entry`, and re-checked immediately before the hand-off to the executor; the exit sweeper flattens under it. |

Existing generic controls (min SOL reserve, position fraction, slippage cap,
duplicate symbol, re-entry cooldown, venue checks) continue to apply
unchanged.

---

## 7. Execution integration

* Entries and exits are `TxRequest`s handed to `solana_kit::execute::Executor`
  — the hardened engine of `docs/EXECUTION-RELIABILITY.md`. The sniper never
  calls `sendTransaction` itself.
* **Entry intent id**: `intent_id(["sniper", mint, "buy", route, launch_ref])`
  with `launch_ref` = creation signature, else slot, else observation time
  (ms). A replayed feed event or a post-crash retry maps onto the same ledger
  record and is refused as a duplicate by `ExecutionLedger::begin`.
* **Exit intent id**: `intent_id(["sniper", position.id, symbol, opened_at_ms,
  "sell", route, sell_raw, qty])` — a retry of the same decision maps onto the
  same record; the next partial exit (smaller position) gets a fresh id.
* The write-ahead intent journal records the intent before broadcast and
  abandons it on rejection; the distributed ownership permit wraps the
  submission; fee policy, blockhash freshness, simulation, confirmation
  tracking, `bot_execution_*` metrics and `execution.<state>` audit events are
  all inherited.
* **Fee policy, twice.** Check 15 asks the same `FeePolicy` the executor is
  about to be refreshed with (`fee_policy_from_config` on the same config
  snapshot) whether it would refuse the configured priority fee, and prices
  the worst case against `sniper.max_entry_fee_lamports`; the executor's
  `decide` then runs at submission with metrics and the emergency veto. The
  pre-check is side-effect free and can only be stricter than the engine.
* Ambiguous outcomes (`Sent` / `SendUnknown`, confirmation timeout) leave the
  ledger record pending for reconciliation; the position is booked so the
  sweeper can manage it once the outcome is known.

---

## 8. Exit hardening

`exit.rs::sweep_once` runs on its own cadence, independent of the feeds. Per
position, in order:

1. **failed-entry cleanup** (`failed_entry_cleanup`): if the entry intent's
   ledger state is failed / expired (`entry_fate` = `EntryFate::Failed`), the position
   is closed as `Failed` without a sell; audit `sniper.exit.failed_entry_cleanup`;
   metric action `failed_entry_cleanup`.
2. **ambiguous hold**: while the entry record is still submitted / pending,
   no sell is attempted (action `held_ambiguous`). The kill switch overrides
   this and flattens.
3. **retry backoff** (`exit_retry_backoff_secs`): after a failed sell the
   position is skipped until the backoff elapses (action `backoff_skip`).
4. **mark to market** on the position's own venue: pump curve → PumpSwap →
   Raydium → Jupiter, driven by `position.venue` / `market_id`; a failed mark
   is counted (`mark_failed`) and tracked per position.
5. **decision**: `RiskEngine::check_exit` (take-profit incl. partial
   `take_profit_sell_fraction`, stop-loss, trailing stop, max hold, kill
   switch) plus the sniper **stale-position rule**
   (`stale_position_exit_secs`: no successful mark for that long → forced
   flatten, action `stale_exit`).
6. **sell** through the executor on the same venue as the entry (Jupiter
   fallback when the direct venue is gone; `INVALID_ROUTE` error when none),
   with the deterministic exit intent id, journal and ownership permit
   (actions `sold` / `sell_failed`).

Repeat prevention: the deterministic exit intent id + the global ledger make
a retried exit a duplicate, and concurrent sweeps over the same position sell
once (`tests/concurrency.rs::concurrent_sweeps_over_the_same_position_sell_once`).
Restart recovery: positions are reloaded from the book, the ledger is restored
from Postgres (`restore_execution_ledger`), and the first sweep applies steps
1–2 (`tests/failure_injection.rs::restart_with_a_failed_entry_cleans_the_position_up_without_selling`).

Partial execution: an on-chain swap is atomic (it fills entirely or fails), so
"partial" means a partial **exit** — `take_profit_sell_fraction < 1` or a
risk decision with `fraction < 1`. `sell_position` sells the fraction, books
the realised slice with `Position::apply_sell`, keeps the remainder open
(`PositionUpdate` event, `partial exit` log) and the next decision on the
smaller position gets a fresh intent id (the held quantity is part of it).
An exit that left the process ambiguously (`Sent` / `SendUnknown`) is booked
at its minimum-out proceeds and the ownership permit is finished with the
grace window so no replica re-sells while it may still land.

Reconciliation mismatch: sniper positions are reconciled by the existing
engine (`bot_core::reconciliation::compare_position`, applied by
`crates/server/src/recon.rs::SolanaPositionTruth`; see
`docs/RECONCILIATION.md`), not by the sweeper. Its verdicts for a sniper
position: entry signature still active → `UnknownExecution` (retry, no
correction; the sweeper's step 2 holds the position meanwhile); book open and
chain zero with a recorded sell fill → `MissingPosition` (position closed from
fill history, `recon_correction`); quantity drift explained by the fill
history → corrected to the on-chain quantity, otherwise `drift_flag`
(flagged, never rewritten); book closed but tokens still on chain — the
signature of an exit that never landed — → `UnexpectedPosition`
(`unexpected_position` flag, operator territory); open locally, zero on chain
and no exit fill → `RecoveryRequired` (paged, not corrected). While the
engine's symbol gate holds a mint, the pipeline refuses new entries in it
(`SYMBOL_GATED`, check 6).

The pump.fun mark uses the corrected `bot_core::maths::pump_spot_price_sol`
(6-decimal raw units) and `pump_market_cap_sol`; the previous constants
produced marks ~10⁶× too high and would have fired take-profit immediately on
any real curve.

---

## 9. Detection per protocol

| Protocol | Sources | Decode path | Event fields |
|---|---|---|---|
| pump.fun (`pump_fun`) | PumpPortal `subscribeNewToken`; `logsSubscribe` on the pump program; Geyser `transactionSubscribe` | `Create` event decode (existing `solana_kit::pump` / `events`); PumpPortal payload validation | mint, creator, curve as pool, virtual reserves → liquidity/price when reported, signature (+ slot for log/Geyser) |
| PumpSwap (`pump_swap`) | `logsSubscribe` on the PumpSwap program when `trade_pumpswap`; Geyser | `CreatePoolEvent` decode (existing `solana_kit::pumpswap`); `market.rs` re-reads the pool + vaults on chain | pool, base/quote mints, creator, initial reserves, slot, signature |
| Raydium AMM v4 (`raydium_amm_v4`) | `logsSubscribe` on the AMM v4 program when `trade_raydium`; Geyser | `initialize2` log line (`solana_kit::raydium::find_initialize2_log` / `parse_initialize2_log`) → the creating transaction is fetched and its instructions decoded (`solana_kit::decode::decode_instructions`) into a `PoolInitEvent`; `market.rs` decodes `AmmInfo` + vaults | pool, coin/pc mints (base must be the non-WSOL side), `pool_open_time`, initial liquidity, slot, signature |

Raydium CLMM / CPMM / LaunchLab program ids remain recognised by the existing
adapter but are never traded (unchanged behaviour).

Feed robustness (all sources): reconnect with backoff via `SolanaWs`
(`sniper_feed_reconnects_total{source}`), gap notifications
(`sniper_feed_gaps_total`), slot regression after a reconnect
(`sniper_feed_out_of_order_total`, logged with the observed sequence),
malformed payloads dropped and logged (never forwarded), per-source
`SequenceTracker`. Missed-event handling is by redundancy (several feeds race;
the pipeline's dedup makes that safe) — there is no historical backfill of
launches that occurred while every feed was down, by design: a launch older
than `max_launch_age_secs` is `STALE_EVENT` anyway.

---

## 10. Latency measurement

`LatencyTimeline` records eight marks: `event_at` (the event's best estimate
of when the launch happened), `observed_at` (event received by this
process), `detected_at` (pipeline picked it up), `validated_at`, `risk_at`
(risk decision), `built_at` (transaction constructed), `submitted_at`
(handed to the execution engine), `confirmed_at`. Histograms (ms,
`LATENCY_BUCKETS_MS`, label `protocol`):

| Metric | Span |
|---|---|
| `sniper_detection_latency_ms` | `event_at` → `observed_at` (feed latency; only when the source stamps events) |
| `sniper_validation_latency_ms` | `detected_at` → `validated_at` |
| `sniper_risk_latency_ms` | `validated_at` → `risk_at` |
| `sniper_build_latency_ms` | `risk_at` → `built_at` |
| `sniper_submission_latency_ms` | `built_at` → `submitted_at` |
| `sniper_confirmation_latency_ms` | `submitted_at` → `confirmed_at` |
| `sniper_total_latency_ms` | `observed_at` → the last mark reached |

`sniper.max_entry_latency_ms` is enforced immediately before the hand-off to
the executor (check 19). These are measurements of this process; they are not
a guarantee about landing position or fill quality.

---

## 11. Replay system

`replay::ReplayEngine::run(&ReplayFixture) -> ReplayReport` drives the
**same** validation code as the live path — `pipeline::precheck`, the
authoritative dedup in `AppState`, `RiskEngine::check_launch_with_lists` and
`check_entry`, `gates::evaluate`, `slippage::decide`, `select_route`,
`pipeline::check_fee_budget` and the deterministic entry intent id — and
stops at `EXECUTION_READY`. The engine holds no RPC handle, wallet, executor
or ledger, so submitting is impossible by construction. Time is
fixture-controlled (`observed_at + now_offset_ms`).

Fixture format (`crates/module-sniper/fixtures/replay/*.json`):
`{ name, description, wallet_sol, sniper: SniperConfig, risk: RiskConfig,
execution?: ExecutionConfig, steps: [ { event: LaunchEvent, snapshot:
MarketSnapshot | null, curve_complete, now_offset_ms, kill_switch,
symbol_gated, emergency_disable, book_realized_sol, expect: { stage,
reason } } ] }`. The optional `execution` section pins the fee-policy inputs
the fee budget (check 15) is priced from (priority fee, compute-unit limit,
retries, policy bounds, Jito tip); omitted, the engine's current
`[execution]` section applies. Replay never executes, so `mode` /
`allow_live_trading` inside it are inert.
The four optional per-step knobs model operator actions between
observations: `kill_switch` toggles the global kill switch, `symbol_gated`
blocks/unblocks the mint in the reconciliation gate, `emergency_disable`
sets/clears `risk.sniper_emergency_disable`, and `book_realized_sol` books
realised sniper PnL (negative = loss) so the daily-loss control can be
replayed without a database. All are `null` unless a step uses them.

| Fixture | Scenario | Expected |
|---|---|---|
| `01_valid_launch` | pump.fun launch, healthy curve | `EXECUTION_READY`, route `curve`, deterministic intent id |
| `02_duplicate_event` | same launch delivered twice (second copy from another feed) | `EXECUTION_READY`, then `DUPLICATE_EVENT` |
| `03_stale_event` | launch older than `max_launch_age_secs` | `STALE_EVENT` |
| `04_malformed_events` | bad mint / signature / missing pool / id mismatch | `INVALID_EVENT` each |
| `05_insufficient_liquidity` | quote reserve below `min_liquidity_sol`; zero pricing reserve | `INSUFFICIENT_LIQUIDITY` |
| `06_excessive_slippage` | thin pool under `liquidity_aware` mode | `SLIPPAGE_LIMIT` |
| `06b_price_impact_limit` | impact above `max_price_impact_bps` | `PRICE_IMPACT_LIMIT` |
| `07_risk_rejection` | wallet below `min_sol_reserve`; kill switch engaged; mint blocked by the reconciliation gate | `RISK_REJECTED`, `KILL_SWITCH`, `SYMBOL_GATED` |
| `08_successful_intent` | two distinct launches | two `EXECUTION_READY` with distinct intent ids |
| `09_failed_execution` | unreadable venue (`snapshot = null`) | `EXECUTION_UNAVAILABLE` |
| `10_reconnect_gap` | launch A; after a websocket gap (sequence jump + slot regression) launch B; then A re-delivered | A and B `EXECUTION_READY`; the re-delivery `DUPLICATE_EVENT` |
| `11_pumpswap_launch` | PumpSwap pool creation | `EXECUTION_READY`, route `pumpswap` |
| `12_raydium_launch` | Raydium AMM v4 pool; `pool_open_time` in the future | `EXECUTION_READY` / `POOL_NOT_READY` |
| `13_token_state_and_concentration` | freeze authority present; creator buy over the cap | `TOKEN_STATE_INVALID` / `CONCENTRATION_LIMIT` |
| `13b_strict_gates` | unreadable mint under `strict_gates` | `TOKEN_STATE_INVALID` |
| `14_invalid_route` | AMM launch with both direct venue and Jupiter fallback off | `INVALID_ROUTE` |
| `15_strategy_disabled_and_exposure` | `risk.sniper_emergency_disable` on, then released; `sniper_max_concurrent_positions = 0` (inherit); −0.5 SOL realised loss against `sniper_daily_loss_limit_quote = 0.5` | `STRATEGY_DISABLED`, then `EXECUTION_READY`, then `EXPOSURE_LIMIT` |
| `16_fee_budget` | pinned `[execution]` (250 000 µlamports/CU × 400 000 CU, 2 attempts at +50 %, fixed) → worst case 155 000 lamports; `max_entry_fee_lamports = 120000` | `FEE_LIMIT` at `VALIDATED` |
| `16b_fee_budget_off` | same launch and `[execution]`, `max_entry_fee_lamports = 0` | `EXECUTION_READY` (outcome carries `fee_estimate_lamports = 155000`) |

`tests/replay.rs` asserts that the checked-in fixtures are byte-identical to
the generator (`checked_in_fixtures_match_the_generator`), that every fixture
reaches its recorded outcome, that two runs are identical and the global
execution ledger is untouched, that AMM fixtures select the direct venues, and
that the JSON round-trips and rejects garbage. Regenerate after an intentional
change with:

```bash
cargo test -p module-sniper --test replay -- --ignored regenerate_replay_fixtures
```

---

## 12. Failure scenarios → behaviour → test

| # | Scenario | Behaviour | Test (`crates/module-sniper/tests/`) |
|---|---|---|---|
| 1 | WS disconnect during detection | `SolanaWs` reconnects with backoff, resubscribes; reconnect counted; launches after the reconnect are detected | `failure_injection::ws_disconnect_during_detection_reconnects_and_keeps_detecting` |
| 2 | WS disconnect after detection | the entry already in the pipeline completes; the feed's fate does not affect it | `failure_injection::ws_disconnect_after_detection_does_not_affect_the_entry` |
| 3 | RPC timeout (no fallback) | `EXECUTION_UNAVAILABLE`; no position, no intent | `failure_injection::rpc_timeout_without_fallback_is_execution_unavailable` |
| 4 | RPC failover | provider pool fails over; the entry completes on the healthy provider | `failure_injection::rpc_failover_to_a_healthy_provider_completes_the_entry`, `tripped_provider_pool_is_reported_before_any_read` |
| 5 | stale blockhash | executor refreshes before broadcast; node-rejected blockhash is a definite failure → failed-entry cooldown | `failure_injection::stale_blockhash_is_replaced_before_broadcast`, `blockhash_rejected_by_the_node_is_a_definite_failure_with_cooldown` |
| 6 | failed simulation | never broadcast; entry marked `FAILED`; position not held | `failure_injection::failed_simulation_never_broadcasts_and_marks_the_entry_failed` |
| 7 | submission failure / ambiguous send | rejection → `FAILED` + cooldown; ambiguous (`SendUnknown`) → position booked, intent left pending for reconciliation | `failure_injection::ambiguous_send_books_the_position_and_leaves_the_intent_pending` |
| 8 | confirmation timeout | intent parked pending; the sweeper holds the position (`held_ambiguous`) | `failure_injection::confirmation_timeout_parks_the_entry_and_the_sweeper_holds_it` |
| 9 | duplicate event | `DUPLICATE_EVENT` from the one authoritative dedup | `pipeline::duplicate_event_is_rejected_by_the_authoritative_dedup`, `concurrency::the_same_launch_seen_by_many_tasks_opens_exactly_one_position` |
| 10 | duplicate execution intent | `ExecutionLedger::begin` refuses the second intent with the same id | `failure_injection::duplicate_intent_is_refused_by_the_execution_ledger` |
| 11 | process restart | book + ledger restored; failed entries cleaned up without selling; pending ones held | `failure_injection::restart_with_a_failed_entry_cleans_the_position_up_without_selling` |
| 12 | database failure | the intent journal is written before broadcast and abandoned on rejection; ledger persistence failures are covered by the Phase-2 sink tests (`server/src/persist.rs`) | `failure_injection::intent_journal_records_before_broadcast_and_abandons_on_rejection` |
| 13 | risk-engine rejection | mapped reason, no intent, audit + metric | `failure_injection::sniper_daily_loss_limit_blocks_new_entries`, `pipeline::risk_exposure_limits_map_to_exposure_limit` |
| 14 | kill switch during execution | re-checked right before hand-off → `KILL_SWITCH`; sweeper flattens under it | `failure_injection::kill_switch_engaged_mid_pipeline_stops_the_hand_off`, `exit_sweeper::kill_switch_flattens_without_marking` |
| 15 | malformed protocol data | malformed curve account → `EXECUTION_UNAVAILABLE`; malformed feed payloads dropped by the detector | `failure_injection::malformed_curve_account_is_execution_unavailable_not_a_trade`, `malformed_feed_payloads_are_dropped_by_the_detector` |
| 16 | stale protocol data | snapshot older than `max_snapshot_age_ms` at submit → `STALE_EVENT` | `failure_injection::stale_market_snapshot_is_refused_at_submit_time`, `pipeline::latency_budget_is_enforced_right_before_submission` |
| 17 | missing liquidity | `INSUFFICIENT_LIQUIDITY` | `failure_injection::missing_liquidity_is_insufficient_liquidity` |
| 18 | invalid token state | freeze/mint authority → `TOKEN_STATE_INVALID`; unreadable mint skipped by default, rejected under `strict_gates`; missing pool account → `POOL_NOT_READY` | `failure_injection::unreadable_mint_is_skipped_by_default_and_rejected_under_strict_gates`, `pumpswap_event_without_a_pool_account_is_pool_not_ready` |

Concurrency (`tests/concurrency.rs`): the same launch seen by many tasks opens
exactly one position; many distinct launches in parallel each get one
position and one intent; entries and the sweeper race without double
selling; concurrent sweeps sell once.

---

## 13. Metrics added

| Metric | Labels | Meaning |
|---|---|---|
| `sniper_events_total` | `protocol`, `source` | events entering the pipeline |
| `sniper_stage_total` | `stage`, `protocol` | lifecycle transitions |
| `sniper_rejections_total` | `reason`, `stage`, `protocol` | every rejection |
| `sniper_gate_results_total` | `gate`, `outcome` | pass / fail / skip per gate |
| `sniper_slippage_bps` (histogram) | `mode` | selected slippage tolerance |
| `sniper_{detection,validation,risk,build,submission,confirmation,total}_latency_ms` (histograms) | `protocol` | §10 |
| `sniper_feed_events_total` | `source`, `protocol` | events emitted per feed |
| `sniper_feed_reconnects_total` | `source` | feed reconnects |
| `sniper_feed_gaps_total` | `source` | gap notifications |
| `sniper_feed_out_of_order_total` | `source` | slot regressions after reconnect |
| `sniper_exit_actions_total` | `action` = `sold` \| `failed_entry_cleanup` \| `stale_exit` \| `held_ambiguous` \| `backoff_skip` \| `mark_failed` \| `sell_failed` | sweeper actions |

Pre-existing `bot_execution_*`, `bot_rpc_*`, `bot_ws_*`, `bot_priority_fee_*`
and `bot_symbol_gated_entries_total` continue to be emitted by the layers the
sniper uses. All metrics go through `bot_core::obs::metrics`.

---

## 14. Configuration reference

`[sniper]` (defaults in parentheses; every key is optional; all keys also
have `SNIPER_*` environment overrides listed in `README.md`):

| Key | Default | Effect |
|---|---|---|
| `slippage_mode` | `"fixed"` | `fixed` \| `liquidity_aware` \| `price_impact` (§5) |
| `pumpswap_slippage_pct`, `raydium_slippage_pct` | unset | per-protocol base tolerance (percent) for the direct AMM routes |
| `slippage_overrides_bps` | `{}` | per-mint tolerance in bps (`{ "<mint>" = 500 }`); ≤ 10000; clamped by `risk.max_slippage_bps` |
| `max_price_impact_bps` | `2500` | `PRICE_IMPACT_LIMIT` ceiling; `0` = off; ≤ 10000 |
| `max_entry_fee_lamports` | `0` | `FEE_LIMIT` budget for the worst-case entry transaction fee (§3.2, check 15); `0` = off; otherwise ≥ 5000 (one signature); warns when the configured first attempt already exceeds it |
| `min_liquidity_sol` | `0.0` | `min_liquidity` gate |
| `require_mint_authority_revoked` | `false` | `mint_authority` gate |
| `require_freeze_authority_revoked` | `true` | `freeze_authority` gate |
| `max_creator_initial_buy_sol` | `0.0` | `creator_concentration` gate (PumpPortal only) |
| `min_pool_supply_fraction` | `0.0` | `pool_supply_fraction` gate; in `[0, 1]` |
| `max_snapshot_age_ms` | `2000` | `snapshot_freshness` gate + submit-time check; must be > 0 |
| `strict_gates` | `false` | skipped gates become failures |
| `stale_position_exit_secs` | `0` | forced exit when no successful mark for this long; `0` = off |
| `exit_retry_backoff_secs` | `10` | backoff after a failed sell (`0` warns: retried every sweep) |
| `failed_entry_cleanup` | `true` | close never-held positions without selling |
| `trade_pumpswap`, `trade_raydium` | `true` | (existing keys) now also enable the PumpSwap / Raydium **detection** feeds on `logsSubscribe`/Geyser |
| `max_entry_latency_ms`, `max_launch_age_secs` | `1000`, `30` | (existing keys) `STALE_EVENT` thresholds |

`[risk]` sniper keys: see §6. Validation (`validate_sniper_engine`) rejects an
unknown slippage mode, out-of-range percentages/fractions/bps, negative
cooldowns, `max_snapshot_age_ms = 0` and a non-zero `max_entry_fee_lamports`
below 5000, and warns when overrides exceed the hard max, when a protocol is
traded without any Solana websocket feed, when `sniper_emergency_disable` is
set, and when `max_entry_fee_lamports` is below the fee of the configured
first attempt. Environment override for the budget:
`SNIPER_MAX_ENTRY_FEE_LAMPORTS`.

No database migration was added by this pass: the decision trail uses the
existing `AppEvent::Audit` path (persisted by the server's audit sink) and the
execution ledger uses migration `0012_execution_lifecycle.sql` from the
execution-reliability pass.

---

## 15. Operator runbook

**Enable safely.** Keep `[execution] mode = "paper"` while validating: the
whole pipeline runs, positions are booked as paper fills, and every metric,
audit event and rejection reason is real. Switch on protocols one at a time
(`trade_pumpswap`, `trade_raydium`) and watch `sniper_rejections_total` by
reason before raising `buy_sol`.

**Read a rejection.** `sniper.entry.rejected` audit rows carry the reason
code, the stage and the full gate summary; the same reason is the metric
label. `INVALID_ROUTE` means the config disallows every venue for that
protocol; `EXECUTION_UNAVAILABLE` points at RPC health (`bot_rpc_*`) or an
unreadable venue; `STALE_EVENT` at `max_launch_age_secs`,
`max_entry_latency_ms` or `max_snapshot_age_ms`.

**Stop entries without stopping exits.** `risk.sniper_emergency_disable = true`
(or `SNIPER_EMERGENCY_DISABLE=true` and restart) refuses new entries; the
sweeper keeps managing open positions. `/api/kill` stops everything and
flattens.

**Ambiguous entries.** A position whose entry shows `held_ambiguous` in
`sniper_exit_actions_total` has an intent in `submitted`/`pending`; wait for
reconciliation (`GET /api/executions?state=pending`) — it will either confirm
(sweeper manages it) or fail (sweeper closes it without selling when
`failed_entry_cleanup` is on).

**Feeds.** A rising `sniper_feed_reconnects_total` with flat
`sniper_feed_events_total` on one source while the others keep producing is a
degraded provider, not a bot fault; `sniper_feed_out_of_order_total` after a
reconnect is expected and harmless (dedup covers re-delivery).

**Positions that cannot be priced.** `mark_failed` growing for one position
means the venue can no longer be read; with `stale_position_exit_secs > 0` the
sweeper force-exits after that many seconds (blind exit — off by default).

**Replay a recorded scenario.** Drop a fixture into
`crates/module-sniper/fixtures/replay/` and run
`cargo test -p module-sniper --test replay`; the report prints each step's
stage, reason and would-be intent id. Nothing is submitted.

**Change the pipeline.** Any change to `precheck`, gates, slippage, routing or
the intent id formula changes recorded verdicts: regenerate the fixtures
(§11) and review the diff before committing.

---

## 16. Known limitations

* No historical backfill of launches missed while every feed was down.
* Gates verify on-chain facts at one instant (§4); they are not a rug or
  scam detector and are not marketed as such.
* The creator-concentration gate depends on PumpPortal reporting the opening
  buy; other feeds skip it (or fail it under `strict_gates`).
* Raydium detection covers AMM v4 `initialize2` only; CLMM / CPMM / LaunchLab
  pools are recognised but not traded (pre-existing scope).
* Daily-loss counters (the existing `daily_loss_limit_quote` and the new
  `sniper_daily_loss_limit_quote`) are in-memory and UTC-day scoped: they
  start from zero after a process restart and are not rebuilt from the
  closed-position history (pre-existing behaviour, shared by the new
  sniper-scoped limit).
* Latency figures are process-local measurements, not landing guarantees.
* Paper mode books fills at the snapshot price; it does not model competing
  order flow.
* The fee budget (check 15) is a configuration-derived upper bound, not a
  quote: it does not read the priority-fee oracle, prices Jupiter-built
  transactions at the 1 400 000-CU maximum, and excludes the refundable
  token-account rent deposit and the venue's trading fee.
* Reconciliation of sniper positions against on-chain balances is the
  existing engine's job (§8); a sell that never landed after an ambiguous
  broadcast is flagged for an operator (`unexpected_position`), not
  re-opened automatically.
