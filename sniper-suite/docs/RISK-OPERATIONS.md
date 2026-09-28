# Global Risk & Accounting — Operations Runbook (TASK 5)

How to operate the global layer described in [GLOBAL-RISK.md](GLOBAL-RISK.md)
and [ACCOUNTING-LEDGER.md](ACCOUNTING-LEDGER.md): setting limits, using the
kill switches, reading the portfolio, handling reconciliation findings and
what to check after a restart. Every command below is an authenticated API
call (`x-api-key`; roles per [API.md](API.md)) or a config change.

This runbook does not make the suite safe or profitable. It makes the
limits you configure enforceable and the books you keep checkable.

---

## 1. First-time setup

1. Decide the reference currency and the rates (`[global_risk]`):

   ```toml
   [global_risk]
   reference_asset = "USD"
   capital_base_ref = 25000.0          # what you consider deployed capital
   max_portfolio_exposure_ref = 5000.0
   max_wallet_exposure_ref = 5000.0
   max_venue_exposure_ref = 3000.0
   max_strategy_exposure_ref = 2000.0
   max_asset_exposure_ref = 500.0
   max_open_positions = 12
   max_order_notional_ref = 250.0
   max_daily_loss_ref = 400.0
   max_drawdown_pct = 0.05

   [global_risk.reference_rates]
   USDC = 1.0
   SOL = 150.0                          # your statement — not fetched
   ```

   Any reference-denominated limit without a `SOL` rate refuses every
   Solana entry with `reference_rate_missing` (the startup log warns).
2. Keep every limit at `0` you do not want; `0` is "off", not "zero
   allowed".
3. Restart or reload. `GET /api/risk/global` shows the configuration in
   force, the active switches, the portfolio totals and recent decisions.
4. On the first start after the upgrade, expect `gap_reported` for every
   open position that predates the ledger (§5).

---

## 2. Reading the portfolio

| Where | What |
|---|---|
| `GET /api/accounting/portfolio` | exposure / realized / unrealized / fees / net / utilization, per venue, wallet, strategy, asset, module; native per-quote figures; `missing_rates` |
| `GET /api/accounting/events?limit=N` | recent ledger events, counts by kind, pending (not yet journaled) ids |
| `GET /api/accounting/findings?limit=N` | reconciliation findings journal |
| `GET /api/risk/global?limit=N` | config, kill switches, totals, last N decisions with their snapshots |
| `/metrics` | `global_portfolio_*`, `global_ledger_*`, `global_risk_*`, `global_accounting_*` |

Alert suggestions: `global_ledger_pending_events > 0` for more than one
maintenance interval; any increase of
`global_accounting_findings_total{kind!="unresolved_financial_event"}`;
`global_risk_rejections_total{reason="reference_rate_missing"}` increasing
(a rate is missing); `global_portfolio_missing_rates > 0`;
`global_risk_journal_errors_total` / `global_ledger_journal_errors_total`
increasing (database trouble — the engine keeps deciding on the in-memory
state, the ledger parks events as pending).

---

## 3. Kill switches

| Action | Command |
|---|---|
| stop everything | `POST /api/kill` (unchanged process-wide switch; flattens per the risk sweeper) |
| refuse new entries on one venue | `POST /api/risk/kill-switch {"scope":"venue:polymarket","engaged":true,"reason":"…"}` |
| refuse new entries for one strategy | `POST /api/risk/kill-switch {"scope":"strategy:copy:<leader pubkey>","engaged":true,"reason":"…"}` |
| release | same body with `"engaged":false` |
| pin in configuration | `killed_venues = ["polymarket"]`, `killed_strategies = ["copy:<leader>"]` — cannot be released over the API (`409 pinned_by_config`) |

Venue names: `pump.fun`, `pumpswap`, `raydium-amm-v4`, `raydium-clmm`,
`jupiter`, `polymarket`, `paper`. Runtime switches survive restarts
(`kill_switches` table) and are audited (`global.kill_switch.*` on the
event bus, `kill_switch_engage|release` in the control-plane audit trail).
Exits, cancels and reconciliation are never blocked by a venue / strategy
switch.

---

## 4. Reading a rejection

Every global reject is visible in three places with the same decision id:
the module's reject reason text (`global risk [<reason>]: <detail>`), the
audit record `global.risk.reject`, and `GET /api/risk/global`
(`recent_decisions[].snapshot`). The snapshot holds the exact figures the
verdict used: requested size in reference units and the rate, portfolio /
wallet / venue / strategy / asset exposure BEFORE the order, open positions,
net realized today, drawdown, capital base.

| Reason | Typical cause | Response |
|---|---|---|
| `*_exposure`, `max_open_positions`, `order_notional` | limits doing their job | wait for exits or raise the limit deliberately |
| `daily_loss` | today's net realized reached the cap | stays until the UTC day rolls; do not "reset" by editing the journal |
| `drawdown` | peak-to-current (incl. unrealized) reached the cap | review open positions; the limit lifts as PnL recovers |
| `reference_rate_missing` | a quote asset has no rate | add the rate; until then no entry on that asset is evaluated |
| `venue_kill_switch` / `strategy_kill_switch` | a switch is engaged | release it or leave it |

---

## 5. Reconciliation findings

Findings are reported, never repaired. Each carries the kind, the subject
(module / venue / asset / position / event / trade / **order**), `expected`
(module side), `actual` (ledger side) and a detail line. The pass compares
four layers: OMS orders → module trades (fills) → ledger events →
positions.

| Kind | What to do |
|---|---|
| `position_mismatch` (open module position, no ledger history) | expected once after the TASK 5 upgrade for pre-existing positions. If the position is real, book an opening correction: `POST /api/accounting/events {"kind":"correction","side":"buy","wallet":"<wallet>","asset":"<mint or token id>","quote_asset":"SOL","quantity":<qty>,"quote_amount":<cost>,"position_id":"<module position id>","reference_id":"open-<position id>","correlation_id":"<ticket>"}`. The next pass matches it. |
| `missing_ledger_entry` | a module trade never reached the ledger (journal was down and the process died before `flush_pending`, or a code path bypassed the submit). Check `pending` first; otherwise book a correction referencing the trade id in `correlation_id` and open a defect. |
| `duplicate_ledger_entry` | the same trade behind two events (different wallet / module labels). Investigate the labels; correct with a `correction` `side = sell` of the surplus, referencing the finding. |
| `quantity_mismatch` | module and ledger disagree on the open quantity of one asset. Compare `GET /api/positions` with `GET /api/accounting/portfolio`; the venue / chain truth (position reconciliation, `docs/RECONCILIATION.md`) decides which side is wrong; correct the wrong side explicitly. |
| `fee_mismatch` / `pnl_mismatch` | figures differ on a flat asset. Usually a fee reported differently; document and correct if material. |
| `orphan_accounting_event` | a ledger event names a trade / position / order no module knows. Check whether the module record was lost (restore) before reversing the event with a correction. |
| `unresolved_financial_event` with an `order_id` | a `Filled` / `PartiallyFilled` OMS order that no ledger event references: the venue reported a fill but nothing was booked. Compare `GET /api/orders/:id` with `GET /api/accounting/events`; if the fill is real, book a `correction` (`side` = the order side, `correlation_id` = the OMS order id) and open a defect — a module fill site failed to submit. |
| `unresolved_financial_event` (no `order_id`) | pending durable write. Fix the database; `flush_pending` runs every maintenance interval; the finding clears when the row lands. |

Corrections are ordinary ledger events: idempotent on their reference,
posted against `adjustments`, audited, and they must name what they answer
(`correlation_id`) — the API refuses them otherwise.

---

## 6. After a restart

Startup order (server `main.rs`): stores attached → module positions
restored → kill switches restored → ledger rebuilt from `ledger_events` →
one reconciliation pass → gauges → workers → modules. Check:

1. The lifecycle event / log line `global ledger recovered: journal_available=true rebuilt=<n> duplicates_skipped=0 pending_flushed=<n> open_positions=<n> gaps=<n>`.
2. `journal_available=false` means the ledger started EMPTY: every module
   position will be reported as `position_mismatch` and every
   reference-denominated limit is evaluated against zero exposure. Treat as
   an incident: engage the process kill switch, restore database
   connectivity, restart.
3. `gaps > 0`: list them under `global.recovery.gap_reported` in the audit
   feed; decide per position (§5).
4. `GET /api/risk/global` → `kill_switches` shows the runtime switches that
   came back.

Replayed confirmations after a restart (user-channel trade re-emissions,
recovery re-submissions) are `duplicate` outcomes in
`global_ledger_events_total{outcome="duplicate"}` — expected, not an
incident.

---

## 7. Operator-entered events

`POST /api/accounting/events` (operator role) books deposits, withdrawals,
transfers, funding adjustments, stand-alone fees and corrections through the
same ledger door the modules use: same idempotency (your `reference_id` is
the identity — reuse it and you get `duplicate`, not a second booking), same
postings, same audit. Fills and settlements are refused over the API — they
are only ever booked by the module that observed them.

Minimal bodies:

```json
{"kind":"deposit","wallet":"treasury","quote_asset":"USDC","quote_amount":1000,"reference_id":"bank-2026-09-21-01"}
{"kind":"withdrawal","wallet":"treasury","quote_asset":"USDC","quote_amount":250,"fee":1,"reference_id":"bank-2026-09-21-02"}
{"kind":"transfer","wallet":"treasury","counterparty_wallet":"<trading pubkey>","quote_asset":"SOL","quote_amount":5,"reference_id":"xfer-17"}
{"kind":"funding_adjustment","wallet":"<eoa>","quote_asset":"USDC","quote_amount":0.4,"side":"sell","reference_id":"funding-2026-09-21"}
```

---

## 8. Changing limits at runtime

Config reload (`AppState::update_config`, the existing `/api/config`
mutation paths and Telegram) re-applies `[global_risk]` to the running
engine: limits, rates and kill lists take effect on the next decision.
Rate changes rescale every reference-denominated figure — change them
deliberately and record why (the decision journal keeps the rate used per
decision).

---

## 9. What this layer does not do

* It does not fetch prices, does not settle anything and does not move
  money; it books what the modules observed and refuses new entries.
* It never cancels or closes a position on its own; the process kill switch
  and the module sweepers keep that role.
* It has not been operated against a live venue or a funded wallet.
