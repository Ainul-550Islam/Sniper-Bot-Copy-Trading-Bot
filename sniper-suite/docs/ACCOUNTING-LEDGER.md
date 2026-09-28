# Global Accounting Ledger (TASK 5)

This document describes the append-only, double-entry, idempotent ledger
that TASK 5 places under every trading module: how a fill becomes a ledger
event, how the event is booked exactly once, how positions are aggregated,
how the ledger is reconciled against the modules' own records and how it is
rebuilt after a restart. Companion documents:
[GLOBAL-RISK.md](GLOBAL-RISK.md) (the engine that reads this ledger) and
[RISK-OPERATIONS.md](RISK-OPERATIONS.md) (runbook).

Nothing in this document is a claim of safety or profitability. The ledger
records what the modules observed, once, and says when the records disagree.

---

## 1. Model

```text
module fill / fee / settlement / operator deposit …
  -> AccountingEvent                typed, explicit (§2)
  -> GlobalLedger::submit           validate → dedup → postings → book → journal → audit (§3)
  -> Entry (Postings)               double entry, balanced per quote asset (§4)
  -> PositionBook                   aggregation per module/venue/wallet/strategy/asset/quote/mode (§5)
  -> PortfolioView                  exposure / PnL / fees / utilization in reference units (§6)
  -> reconcile()                    module truth vs ledger → typed findings, never repaired (§7)
  -> recover()                      rebuild from ledger_events, replay-safe, gaps reported (§8)
```

Source files: `crates/core/src/accounting/{event,posting,book,ledger,view,
store,reconcile,recovery,metrics,audit}.rs`; durable repository
`crates/core/src/db/accounting.rs`; migration
`crates/core/migrations/0015_global_risk_accounting.sql`; server wiring
`crates/server/src/accounting.rs`; API routes in `crates/server/src/api.rs`.

Non-negotiables:

| Rule | Where it holds |
|---|---|
| Modules never mutate accounting state | They build an `AccountingEvent` and call `state.ledger().submit(event)`; the book, the postings and the journal are private to `GlobalLedger`. |
| ONE idempotency identity | `AccountingEvent::event_id` = `led_` + digest of `(kind, module, venue, wallet, reference_id)`. Memory index + `INSERT … ON CONFLICT (event_id) DO NOTHING` are the two halves of the same mechanism. |
| Append-only | No update or delete path exists for `ledger_events` / `ledger_postings`; corrections are new, referenced `correction` events. |
| Never repair silently | Reconciliation reports; only an operator books a correction, and it must name the finding / ticket it answers. |
| Never replay blindly | Recovery re-applies the journal through the same idempotent path; module positions without ledger history are reported as gaps, not synthesised. |

---

## 2. Events

`AccountingEvent` (`accounting/event.rs`) carries, for every financial
mutation:

| Field | Meaning |
|---|---|
| `kind` | `fill` · `fee` · `settlement` · `deposit` · `withdrawal` · `transfer` · `funding_adjustment` · `correction` |
| `module` | source module (`telegram` = operator input over the API) |
| `venue` | venue the money moved on (`paper` for paper fills / operator cash events) |
| `wallet` | our account: Solana pubkey, Polygon EOA, or an operator-named account |
| `strategy` | `sniper` · `copy:<leader>` · Polymarket strategy name · `operator` |
| `asset`, `quote_asset` | base asset (mint / token id) and the cash asset the amounts are in (`SOL`, `USDC`) |
| `side` | `buy` / `sell` for inventory-moving kinds |
| `quantity`, `price`, `quote_amount`, `fee` | base units, quote per unit, cash that moved (fee included on the way out: buy = spent, sell = received net of the venue fee — the `Trade.amount_in/out` convention), fee |
| `mode` | paper / simulate / live |
| `reference_id` | source identity of the fact: tx signature (Solana live), venue fill id (Polymarket), `paper:<trade id>@<ms>` (paper), bank / ticket reference (operator) |
| `correlation_id` | OMS order id / intent id / claim id (entries); the finding or ticket a correction answers |
| `position_id`, `trade_id` | the module position and trade record the event books — reconciliation join keys |
| `counterparty_wallet` | transfers |
| `ts`, `detail` | fact time, single-line detail |

`fill_event_for_trade(&Trade, wallet, strategy, reference, correlation)`
builds the fill event from a module's `Trade` record so both carry the same
figures. Validation (`AccountingEvent::validate`) refuses non-finite or
negative amounts, empty identity fields, inventory kinds without a side or
quantity, transfers without a counterparty and corrections without a
correlation.

### 2.1 Where the modules submit

| Module | Site | Reference | Correlation | Strategy |
|---|---|---|---|---|
| sniper entry | `module-sniper/src/entry.rs::record_execution` after the position is created | signature / paper ref | snipe intent id | `sniper` |
| sniper exit | `module-sniper/src/exit.rs::sell_position` after `book_pnl` | signature / paper ref | — (position id + signature) | `sniper` |
| copy entry | `module-copy/src/mirror.rs` after the position is created | signature / paper ref | copy intent id | `copy:<leader>` |
| copy exit | `module-copy/src/exit.rs::sell_position` after `book_pnl` | signature / paper ref | — | `copy:<leader>` (from `copied_wallet`) |
| Polymarket fill | `module-polymarket/src/lifecycle.rs::book_fill` after the fill journal accepted the fill and the position is updated | venue fill id (`poly_fills.fill_id`) | OMS order id | `[polymarket].strategy` |
| operator | `POST /api/accounting/events` (deposit, withdrawal, transfer, funding adjustment, fee, correction — never fill / settlement) | operator reference | ticket (required for corrections) | `operator` |

Each site is placed AFTER the module's own idempotency (execution ledger,
copy event dedup, Polymarket fill journal), so the ledger sees each fact once
per process life; the ledger's own identity check covers replays across
restarts and replicas.

---

## 3. Booking (`GlobalLedger::submit`)

Under ONE lock: (1) `validate` → `Rejected(reason)` on failure, nothing
changes; (2) `event_id` already known → `Duplicate`, nothing changes; (3)
expand the balanced postings with the cost of the leaving slice at the
running average cost; (4) apply to the book; (5) record the event in the
in-memory index. Two concurrent submissions of the same fact cannot both
pass step 2 (`ledger.rs::concurrent_identical_events_yield_one_mutation`,
`tests/global_risk_accounting.rs::concurrent_identical_events_from_many_tasks_book_once`).

After the lock: the durable write. `Some(true)` = journaled; `Some(false)`
= the journal already held the id (a previous process life booked it) →
`Duplicate`; `None` = journal unavailable → the event stays applied in
memory, is parked as **pending**, retried by `flush_pending` (maintenance
loop) and reported by reconciliation as `unresolved_financial_event` until
it lands. Then the position snapshot upsert (best-effort), metrics and the
audit records `global.ledger.<kind>` + `global.position.<transition>`.

Amounts are deliberately NOT part of the identity: a replay that carries a
different amount for the same fact is a duplicate (and, if it reflects a
real difference, a reconciliation finding), never a second booking.

---

## 4. Postings (double entry)

Accounts (all in the event's quote asset, per wallet): `cash`, `inventory`
(open positions at fee-exclusive cost), `fees`, `realized_pnl`, `equity`
(deposits / withdrawals), `funding`, `adjustments` (corrections). Per event
and per quote asset, debits equal credits (`Entry::is_balanced`).

| Event | Postings (`q` cash moved, `f` fee, `c` cost of the sold slice) |
|---|---|
| buy fill / settlement-in / correction-in | Dr inventory `q − f` · Dr fees `f` · Cr cash (adjustments for corrections) `q` |
| sell fill / settlement-out / correction-out | Dr cash `q` · Dr fees `f` · Cr inventory `c` · Cr realized_pnl `q + f − c` (a debit when negative) |
| fee | Dr fees · Cr cash |
| deposit | Dr cash · Cr equity |
| withdrawal | Dr equity `q` · Dr fees `f` · Cr cash `q + f` |
| transfer | Dr cash(counterparty) `q` · Dr fees `f` · Cr cash(wallet) `q + f` |
| funding adjustment | Dr cash · Cr funding (negative: `side = sell`, reversed) |

Rows: `ledger_postings(event_id, seq, account, wallet, asset, side, amount,
quantity, base_asset)`; inventory lines carry the base quantity and asset.

---

## 5. Position aggregation (`PositionBook`)

Key: `(module, venue, wallet, strategy, asset, quote_asset, mode)`. Per
position: `qty`, `cost_basis` (fee-exclusive, running average cost),
`realized` (gross proceeds − cost of sold slices), `fees`, bought / sold
totals, `last_price`, event count, last event id and the module position
ids seen. Derived: `avg_cost`, `exposure(mark) = max(cost_basis, qty ×
mark)`, `unrealized(mark) = qty × mark − cost_basis`, `net_realized =
realized − fees`.

The arithmetic is the suite's average-cost convention
(`bot_core::reconciliation::reconstruct_pnl`); `book.rs` tests assert
equality with it. Against a module `Position` (fee-inclusive
`cost_basis`, `realised() = realized_quote − cost_basis`) the figures agree
on quantity always and on net realized once flat:
`net_realized = Σ net proceeds − Σ fee-inclusive cost`.

Cash-only kinds (deposit, withdrawal, transfer, funding, stand-alone fee)
land in the book's per-`(wallet, quote)` cash view and do not open
positions.

---

## 6. Portfolio view

`PortfolioView::compute(book, realized series, inputs)` — inputs are the
marks (from the modules' positions), the reference rates and the capital
base from `[global_risk]`, and the UTC day. Output (all in reference units
unless stated): `total_exposure_ref`, `realized_ref`, `unrealized_ref`,
`fees_ref`, `net_pnl_ref = realized + unrealized − fees`,
`realized_today_ref`, `cumulative_realized_ref`, `peak_realized_ref`,
`drawdown_ref()`, `open_positions`, `capital_base_ref`, `utilization =
exposure / capital base`, slices `by_venue`, `by_wallet`, `by_strategy`,
`by_asset`, `by_module`, native per-quote figures (`native`), and
`missing_rates` (quote assets with open exposure but no rate — those
positions are excluded from the reference totals and listed, never
guessed). Served by `GET /api/accounting/portfolio` and published as the
`global_portfolio_*` gauges.

---

## 7. Reconciliation (`accounting/reconcile.rs`)

`reconcile(ReconInputs)` is pure over snapshots of the FOUR record layers —
the OMS orders (intent), the modules' trades (fills) and positions (this
process life), and the ledger (book + journaled events + pending ids) —
plus a quantity tolerance (the same `QuantityTolerance` policy as position
reconciliation: 1 % relative, `1e-9` dust):

```text
OMS orders  ->  module trades (fills)  ->  ledger events  ->  positions
  (intent)        (what the venue           (money booked      (module book
                   reported filled)          exactly once)      + ledger book)
```

Findings:

| Kind | Rule |
|---|---|
| `missing_ledger_entry` | a module trade has no fill / settlement event matched by trade id, signature or Polymarket `fill=` note; or a `Filled` / `PartiallyFilled` OMS order whose trade exists but is itself unbooked |
| `duplicate_ledger_entry` | one trade is behind two ledger events with different ids |
| `position_mismatch` | an open module position (qty > dust) has no book position carrying its id |
| `quantity_mismatch` | per `(module, venue, asset, mode)`: Σ module qty vs Σ book qty beyond tolerance |
| `fee_mismatch` | the matched trade and event disagree on the fee |
| `pnl_mismatch` | flat on both sides but Σ module `realised()` vs Σ book `net_realized` beyond tolerance |
| `orphan_accounting_event` | an event recorded in this process life names a trade and a position no module knows |
| `unresolved_financial_event` | applied but not durably journaled (pending); or a `Filled` / `PartiallyFilled` OMS order that no ledger event references at all (a fill was reported but no money was booked) |

Order-layer rule (§5 "orders" input): an order is matched to the ledger by
the correlation id the modules stamp on their fill events (the OMS order
id / intent id), by the order's venue signature, by its external id, or
through a module trade that names it (`oms=<id>` in the note). Orders in
intent-only states — created, validated, queued, submitted, accepted,
failed, cancelled, expired, unknown, reconciled — are never reported: the
ledger books fills, not intents. A ledger event whose correlation id names a
known order is not an orphan even when its trade / position records are
gone.

Finding ids are digests of the identity (kind + subject), not of the
numbers: a persisting discrepancy is journaled (`accounting_recon_findings`),
audited (`global.recon.<kind>`) and metered once per process life; every
run still returns the full list (`ReconRun::findings` vs `ReconRun::new`).
The action is always `reported`.

The server runs a pass at startup and every
`accounting_reconcile_interval_secs` (`crates/server/src/accounting.rs`),
feeding it the last 5,000 OMS orders, the module positions and the last
5,000 trades; `GET /api/accounting/findings` lists the journal. Each finding
row names the subject it is about (`order_id` / `trade_id` / `event_id` /
`position_id`).

---

## 8. Recovery (`accounting/recovery.rs`)

`GlobalLedger::recover(module_positions)` runs at startup after the
persistence layer restored the modules' positions and BEFORE any module
trades (`main.rs`: `accounting::recover` right after `persist::restore`):

1. `load_events()` from the journal → `hydrate` re-applies every event
   through the same validate / dedup / postings / book path (`rebuilt`,
   `duplicate_skipped`); an unreadable journal is reported
   (`journal_unavailable`) and the ledger starts empty — reconciliation then
   flags every position.
2. `flush_pending` (`pending_flushed`).
3. The realized series (daily net realized, cumulative, peak) is rebuilt as
   a by-product, so daily-loss and drawdown gating is back before the first
   decision.
4. Every open module position without ledger history is reported as
   `gap_reported` — audited, metered, listed in the report and in the
   startup log. Nothing is synthesised: if the position is real, an operator
   books an opening `correction` (`side = buy`, the module position id in
   `position_id`, the ticket in `correlation_id`) which the reconciliation
   then matches; otherwise the `position_mismatch` finding stands.
5. Runtime kill switches are restored from `kill_switches`
   (`GlobalRiskEngine::restore`) before the ledger.

A fill replayed after the restart (a re-emitted confirmation, a recovery
re-submission) is `Duplicate`. Recovery is idempotent: a second run rebuilds
nothing and skips every id.

---

## 9. Durable tables (migration 0015)

| Table | Content |
|---|---|
| `ledger_events` | one row per event, PK `event_id`, guarded insert |
| `ledger_postings` | balanced lines per event, `UNIQUE (event_id, seq)`, inserted in the same transaction as the event |
| `global_positions` | derived aggregated-position snapshots (informational; rebuilt from events) |
| `global_risk_decisions` | every global decision with its snapshot |
| `kill_switches`, `kill_switch_events` | runtime switch state + append-only history |
| `accounting_recon_findings` | append-only findings (`order_id` / `trade_id` / `event_id` / `position_id` name the subject) |

Additive only — `IF NOT EXISTS`, no change to any earlier table. The
in-memory stores (`MemoryLedgerStore`, `MemoryRiskStore`) implement the same
contracts for tests and no-database runs.

---

## 10. Metrics and audit

Metrics: `global_ledger_events_total{kind,outcome}`,
`global_ledger_journal_errors_total{op}`, `global_ledger_pending_events`,
`global_accounting_findings_total{kind}`,
`global_accounting_recovery_actions_total{action}`,
`global_portfolio_total_exposure_ref_milli`,
`global_portfolio_exposure_ref_milli{scope,key}`,
`global_portfolio_utilization_milli`,
`global_portfolio_realized_pnl_ref_milli`,
`global_portfolio_unrealized_pnl_ref_milli`,
`global_portfolio_fees_ref_milli`, `global_portfolio_net_pnl_ref_milli`,
`global_portfolio_realized_today_ref_milli`,
`global_portfolio_drawdown_ref_milli`, `global_portfolio_open_positions`,
`global_portfolio_missing_rates`, plus
`bot_duplicate_execution_prevented_total{where="global_ledger"|"global_ledger_journal"}`.

Audit (actor `global-ledger`): `global.ledger.<kind>` per booked event,
`global.position.<opened|increased|reduced|closed>` per position transition,
`global.recon.<kind>` per new finding, `global.recovery.<action>` per
recovery action. Operator API entries are additionally recorded by the
control-plane audit trail (`ledger_event`, `kill_switch_engage|release`).

---

## 11. Tests

Unit: `accounting/event.rs` (identity, validation), `posting.rs` (balance
per kind), `book.rs` (average cost, over-sell clamp, fees, exposure),
`ledger.rs` (exactly-once, concurrency, pending / flush, journal-held
duplicate, realized series), `view.rs` (slices, missing rates),
`reconcile.rs` (every finding kind, stable ids, the order layer: filled
orders without a ledger event, intent-only states silent, correlation /
signature / trade matching, order-explained events are not orphans),
`recovery.rs` (rebuild, gaps, unavailable journal). Integration:
`crates/core/tests/global_risk_accounting.rs` (19, incl.
`the_order_layer_is_reconciled_against_the_ledger` against a real
`OrderManager`), the module suites
(`module-polymarket/tests/order_pipeline.rs`,
`module-sniper/tests/concurrency.rs`, `module-copy/tests/dedup_ordering.rs`
assert the fill events the modules submit), the API tests in
`crates/server/src/api.rs`, and the PostgreSQL round-trip in
`crates/core/tests/db_integration.rs` (gated on `POSTGRES_URL`).

---

## 12. Limitations

* Fees are what the modules report on their `Trade` (currently `0` for the
  Solana paths and Polymarket; the copy mirror passes the leader's fee
  field). The ledger books what it is given.
* `trades` are compared over the in-memory window of this process life
  (`AppState::trades(5000)`); older module trades are not re-checked for
  missing ledger entries after a restart (open positions are, via
  `position_mismatch`).
* Pre-TASK-5 positions have no ledger history until an operator books an
  opening correction; they are reported, not assumed.
* No live venue, funded wallet or database was used to produce the
  deliverable; the PostgreSQL path is covered by the gated integration test.
