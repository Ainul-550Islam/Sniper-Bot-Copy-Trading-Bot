# Global Risk Engine (TASK 5)

This document describes the portfolio-level risk layer delivered in TASK 5:
the ONE global decision that runs in front of every module's own risk
checks, what it reads, what it refuses and why, and how it is configured.
The companion documents are [ACCOUNTING-LEDGER.md](ACCOUNTING-LEDGER.md)
(the ledger the engine reads its exposure and PnL from) and
[RISK-OPERATIONS.md](RISK-OPERATIONS.md) (runbook: kill switches, limits,
findings, recovery).

Nothing in this document is a claim of profitability or safety. A limit
that is not configured is not enforced; a limit that is configured is
enforced deterministically against the figures the ledger holds — no more,
no less.

---

## 1. Scope and design rules

TASK 5 adds one layer over the existing TASK 1–4 engines. It does not add a
bot or a venue, it does not change a strategy, and it does not remove or
bypass anything the modules already do.

| Rule | Where it holds |
|---|---|
| Exactly one final global decision | `bot_core::global_risk::GlobalRiskEngine::decide`, called from `RiskEngine::check_entry` (step 2b) — the single risk call every module already makes (sniper `entry.rs`, copy `event.rs`, Polymarket `pipeline.rs`). |
| Global first, module second | Only an `Accept` reaches the TASK 1–4 module checks (slippage, per-module caps, cooldowns, balance, venue checks). A global reject ends the pipeline with a `RiskCode::Global*` code and the module's own reject vocabulary label (`KillSwitch`, `ExposureLimit`, `DailyLossLimit`, …). |
| No second source of truth | Exposure, open-position counts, realized PnL, drawdown are read from the global ledger's aggregated book (`GlobalLedger::portfolio`) — never from a module's private counters. The modules' own `Position`s remain their operational record; accounting reconciliation compares the two. |
| Deterministic reasons | Fourteen closed `GlobalRejectReason`s; the first failing check in a fixed order is the reason; every figure the verdict used is journaled in the decision snapshot. |
| No hidden mutable state | Limits live in `[global_risk]`; runtime kill switches are durable rows with an append-only event log; the daily-loss and drawdown inputs are rebuilt from the journal on every start. |
| Off by default | Every limit is `0` = off and the kill lists are empty, so an unconfigured suite behaves exactly as before TASK 5. |

---

## 2. Pipeline position

```text
signal
  -> venue / strategy                (module pipeline stages, TASK 2/3/4)
  -> RiskEngine::check_entry
       1.  preflight: process kill switch, module enabled, legacy daily loss
       2.  request sanity (size > 0, finite)
       2b. GlobalRiskEngine::decide   <- TASK 5: the ONE global decision
       3+. module checks: slippage, per-module caps, cooldowns, balance, venue
  -> OMS / execution
  -> fill -> AccountingEvent -> GlobalLedger        (docs/ACCOUNTING-LEDGER.md)
  -> book -> PortfolioView -> next decision reads it
  -> accounting reconciliation (orders -> fills -> ledger -> positions)
```

The request the engine evaluates (`GlobalRiskRequest`) carries the module,
venue, **wallet**, **strategy label**, asset (mint / token id), quote asset
(`SOL` on Solana venues, `USDC` on the CLOB), the requested size in quote
units and the execution mode. The wallet and strategy come from the module
through two new `EntryRequest` fields:

| Module | `wallet` | `strategy` |
|---|---|---|
| sniper | trading wallet pubkey | `sniper` |
| copy | trading wallet pubkey | `copy:<leader pubkey>` |
| Polymarket | signer EOA (module name when no key is loaded) | `[polymarket].strategy` (`value` / `search`) |

An empty wallet or strategy (replay fixtures, older callers) is attributed to
the module name.

---

## 3. The decision

`GlobalRiskEngine::decide` evaluates the checks below in order and stops at
the first failure. Every check that needs a reference conversion is skipped
when its limit is `0`.

| # | Check | Reason on failure | Input |
|---|---|---|---|
| 0 | request size finite and `> 0` | `invalid_request` | request |
| 1 | process-wide kill switch (`AppState::kill_switch`) | `global_kill_switch` | state |
| 2 | venue kill switch | `venue_kill_switch` | `[global_risk].killed_venues` ∪ runtime switches |
| 3 | strategy kill switch | `strategy_kill_switch` | `[global_risk].killed_strategies` ∪ runtime switches |
| 4 | reference rate present for the request's quote asset AND for every quote asset with open exposure — only when a reference-denominated limit is on | `reference_rate_missing` | `[global_risk.reference_rates]`, book |
| 5 | `max_open_positions` — open aggregated positions across every module (adding to an already-open aggregated position is not a new position) | `max_open_positions` | book |
| 6 | `max_order_notional_ref` | `order_notional` | request × rate |
| 7 | `max_portfolio_exposure_ref`, `max_wallet_exposure_ref`, `max_venue_exposure_ref`, `max_strategy_exposure_ref`, `max_asset_exposure_ref` — `current + requested > cap` | `portfolio_exposure` / `wallet_exposure` / `venue_exposure` / `strategy_exposure` / `asset_exposure` | `PortfolioView` slices |
| 8 | `max_daily_loss_ref` — today's net realized (realized − fees) `<= −limit` | `daily_loss` | realized series (UTC day) |
| 9 | `max_drawdown_ref` / `max_drawdown_pct × capital_base_ref` (the tighter) — `peak cumulative net realized − (cumulative net realized + unrealized) >= limit` | `drawdown` | realized series + marks |

Exposure of one aggregated position is `max(cost basis, quantity × mark)` —
the same rule the module engine applies to its own positions. Marks come
from the modules' operational positions (`AppState::marks`, latest
`last_mark` per symbol); an asset without a module mark uses its last fill
price.

### 3.1 Reference currency and rates

All limits are denominated in `[global_risk].reference_asset` (default
`USD`). Native quote figures are converted with
`[global_risk.reference_rates]` (`SOL = 150.0`, `USDC = 1.0`). **The suite
fetches no prices for this.** The rates are an operator statement; they are
part of the configuration record, re-read on config reload and copied into
every decision snapshot (`snapshot.rate`).

Fail-closed rule: when any reference-denominated limit is on and either the
request's quote asset has no rate, or a quote asset with open exposure has
no rate (the portfolio figures would be understated), the entry is refused
with `reference_rate_missing`. Without such a limit the rates are unused and
nothing is refused for their absence. Configuration validation warns when a
reference-denominated limit is on and `SOL` has no rate.

### 3.2 Mapping onto the module vocabulary

| `GlobalRejectReason` | `RiskCode` | sniper / copy label | Polymarket label |
|---|---|---|---|
| `global_kill_switch`, `venue_kill_switch`, `strategy_kill_switch` | `GlobalKillSwitch` | `KILL_SWITCH` | `KILL_SWITCH` |
| `max_open_positions`, `order_notional`, `*_exposure` | `GlobalExposure` | `EXPOSURE_LIMIT` | `EXPOSURE_CAP` |
| `daily_loss` | `GlobalDailyLoss` | `EXPOSURE_LIMIT` | `DAILY_LOSS_LIMIT` |
| `drawdown` | `GlobalDrawdown` | `EXPOSURE_LIMIT` | `EXPOSURE_CAP` |
| `reference_rate_missing`, `invalid_request` | `GlobalUnavailable` | `RISK_REJECTED` | `RISK_REJECTED` |

The `RiskDecision.reason` text always starts with
`global risk [<reason>]: <detail with the figures>`.

---

## 4. Kill switches

Three scopes:

| Scope | Where | Effect |
|---|---|---|
| process-wide | unchanged: `AppState::set_kill_switch`, `/api/kill`, Telegram, runtime-flag sync across replicas | every module refuses to send; the risk sweeper flattens |
| venue | `[global_risk].killed_venues` (config-pinned) or `POST /api/risk/kill-switch {"scope":"venue:<venue>"}` (runtime) | NEW entries on the venue are refused (`venue_kill_switch`); exits, cancels, polling and reconciliation keep running |
| strategy | `[global_risk].killed_strategies` or `POST /api/risk/kill-switch {"scope":"strategy:<label>"}` | NEW entries for the label are refused (`strategy_kill_switch`) |

Rules: either source can only tighten; a configuration-pinned switch cannot
be released at runtime (`409 pinned_by_config`) until the configuration
changes; runtime switches are durable (`kill_switches` current state,
`kill_switch_events` append-only history) and restored on every start; every
change is audited (`global.kill_switch.engage|release|restored`) and metered.

Venue names are `Venue::as_str` values: `pump.fun`, `pumpswap`,
`raydium-amm-v4`, `raydium-clmm`, `jupiter`, `polymarket`, `paper`.

---

## 5. Decision journal, metrics, audit

Every decision — accept and reject — is a `GlobalRiskDecision` with a
deterministic id (`grd_` + digest of request identity, time and sequence),
the verdict, the reason, a human detail line and the `DecisionSnapshot`
(requested size in reference units, rate, portfolio / wallet / venue /
strategy / asset exposure before the order, open positions, net realized
today, drawdown, capital base). Decisions are kept in memory (last 256,
`GET /api/risk/global`) and journaled to `global_risk_decisions`
(best-effort; a failed write is metered, never blocks the decision).

Metrics (`global_risk_*`): `global_risk_decisions_total{verdict,reason}`,
`global_risk_rejections_total{reason}`,
`global_risk_kill_switch_changes_total{scope,action}`,
`global_risk_kill_switches_active{scope}`,
`global_risk_journal_errors_total{op}`. Audit: rejections as
`global.risk.reject` (actor `global-risk`, target = decision id, outcome =
the full decision line); kill-switch changes as `global.kill_switch.*`.

---

## 6. Configuration (`[global_risk]`)

| Key | Default | Meaning |
|---|---|---|
| `reference_asset` | `"USD"` | label of the reference currency |
| `reference_rates` | `{ USDC = 1.0 }` | quote asset → reference units |
| `capital_base_ref` | `0` | declared capital; backs utilization and `max_drawdown_pct` (0 = unknown) |
| `max_portfolio_exposure_ref` | `0` | cap on total open exposure (0 = off) |
| `max_wallet_exposure_ref` | `0` | cap per wallet |
| `max_venue_exposure_ref` | `0` | cap per venue |
| `max_strategy_exposure_ref` | `0` | cap per strategy label |
| `max_asset_exposure_ref` | `0` | cap per base asset |
| `max_open_positions` | `0` | cap on open aggregated positions across every module |
| `max_order_notional_ref` | `0` | cap on one order |
| `max_daily_loss_ref` | `0` | net realized loss for the UTC day that stops new entries |
| `max_drawdown_ref` | `0` | absolute drawdown from the realized peak that stops new entries |
| `max_drawdown_pct` | `0` | same as a fraction of `capital_base_ref` (needs the base) |
| `killed_venues` | `[]` | config-pinned venue switches |
| `killed_strategies` | `[]` | config-pinned strategy switches |
| `accounting_reconcile_interval_secs` | `60` | server maintenance loop period (0 = startup pass only) |

Validation (`config.rs::validate_global_risk`): every limit finite and
`>= 0`; `max_drawdown_pct` in `[0, 1]`; every rate finite and `> 0`; every
`killed_venues` entry a known venue; no empty strategy label; warnings when
`max_drawdown_pct` is set without a capital base and when a
reference-denominated limit is on without a `SOL` rate. Config reload
(`AppState::update_config`) re-applies limits, rates and kill lists to the
running engine.

The legacy `[risk]` limits (`daily_loss_limit_quote`, `max_open_positions`,
the per-module `sniper_*` / `copy_*` / `poly_*` caps) are unchanged and still
enforced by the module checks after the global decision. They are
denominated in native quote units; the global layer is the only place where
SOL and USDC exposure meet in one figure.

---

## 7. Tests

`crates/core/tests/global_risk_accounting.rs` (18 tests, offline, real
`AppState` + real `RiskEngine::check_entry`): portfolio / wallet / venue /
strategy / asset / open-position / order-notional limits, fail-closed
reference rates, daily loss from the ledger, drawdown from peak + unrealized,
configured + runtime venue / strategy kill switches (incl. journal restore),
config reload, attribution. Unit tests live next to the code
(`global_risk/*.rs`, `config.rs`). API routes are covered in
`crates/server/src/api.rs` tests. See [TESTING.md](TESTING.md).

---

## 8. Limitations

* Reference rates are operator statements, not market data; a stale rate
  scales every reference-denominated limit accordingly.
* Drawdown uses the realized peak since the journal began plus current
  unrealized; there is no separate durable high-water mark table (the peak is
  recomputed from `ledger_events` on every start).
* The global engine gates ENTRIES only. Exits, cancels and reconciliation are
  never refused by it — reducing exposure is always allowed.
* Exposure attribution depends on the labels the modules pass; a module that
  passes an empty wallet is attributed to its module name.
* Nothing here has been run against a live venue or a funded wallet.
