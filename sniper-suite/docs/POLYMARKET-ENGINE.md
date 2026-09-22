# Polymarket Trading Engine (Module 3)

This document describes the Polymarket engine delivered in TASK 4: how a
market scan becomes (or does not become) a CLOB order, which component owns
which decision, how a live order is tracked from submission to its terminal
state, what is persisted, and every knob and metric an operator can use. The
companion documents are [POLYMARKET-OPERATIONS.md](POLYMARKET-OPERATIONS.md)
(runbook) and [POLYMARKET-RECOVERY.md](POLYMARKET-RECOVERY.md) (crash points,
restart procedure, reconciliation findings).

Nothing in this document is a claim of profitability or safety. The two
strategies shipped here (`value`, `search`) are deliberately simple and have
well-known adverse cases (stale books, thin markets, adverse selection when
resting at the ask, markets that resolve against you). The engine's job is to
make every decision deterministic, bounded, observable and recoverable — not
to make it profitable.

---

## 1. Scope and design rules

The engine is an **extension of the existing `module-polymarket` crate**. The
venue clients (`clob.rs`, `gamma.rs`, `ws.rs`), the credential derivation
(`auth.rs`), the CLOB V2 EIP-712 signer (`eip712.rs`, `orders.rs`), the
on-chain readers (`collateral.rs`, `ctf.rs`) and the two strategies
(`strategy.rs`) are kept and wired into a staged pipeline
(`PolyBot::process_signal`). Nothing was rewritten in parallel. The same
non-negotiables that govern the sniper and copy engines govern this module:

| Rule | Where it holds |
|---|---|
| One authoritative risk decision | `bot_core::risk::RiskEngine` — `check_polymarket_coded` (Polymarket-specific caps) followed by `check_entry` (generic limits, price band, per-order sizing). `strategy.rs` never reads `risk.*` and never sizes against the balance. |
| One authoritative dedup / idempotency | The **intent key** (`orders::intent_key`) is the OMS idempotency key. `OrderManager::get_by_key` is the only cross-restart duplicate check; the in-process `claim_inflight` set only serialises concurrent callers of the *same* key. |
| Never bypass the OMS / ledger | Every paper or live order is an `OrderManager` order (`BotModule::Polymarket`, venue `polymarket`). Fills reach positions only through `AppState::record_trade`, which is what the persistence layer, the dashboard and the reconciliation queue already consume. |
| Paper by default | `execution.mode = "paper"` builds the same `OrderSignal`, walks the same stages and books an in-process fill. No signer, API key or RPC is needed; nothing is POSTed. |
| Live fails closed | Live entries require a verified collateral read (`collateral.rs` via `polymarket.ctf_rpc_url`), a derived or configured API key and, for EOA signers, an ERC-20 allowance for the exchange. Any of those missing → `REJECTED`, never a fallback to the paper balance. |
| Cross-replica safety | Entries claim `poly:entry:<token_id>` through the shared `OwnershipRegistry` and are **fenced** before the POST; store failures fail closed (`OWNERSHIP_UNAVAILABLE`). |
| Replays never trade | There is no replay source in this module; the only entry point that sends is `process_signal` in live mode with a signer. Tests use an in-process mock venue (`tests/common/mod.rs`) — no test code ships in the crate. |

### Pre-existing behaviour kept

The strategies' decision rules (`value`: basket of both outcomes when
`1 - (ask_yes + ask_no) ≥ min_edge`; `search`: buy the first outcome of a
market whose question/slug contains a watch keyword) are unchanged. They were
**hardened**, not replaced: explicit market and quote gates with a stable
skip vocabulary (§4), size rounding to the CLOB's 0.01 grid, NaN-safe price
checks, and a deterministic `Verdict` for every outcome so the journal can
explain every non-trade.

### Defect fixed during TASK 4

`post_live` used to send the *configured* `polymarket.order_type` verbatim.
A lower-case value (`"gtd"`) reached the venue as-is and a config edit between
decision and POST could change the order type after the intent had been
frozen. The POST now uses the frozen signal's normalised type
(`tests/strategy_sizing.rs::gtd_expiry_and_order_type_reach_the_venue_as_configured`).

---

## 2. Component map

| File | Role |
|---|---|
| `crates/module-polymarket/src/lib.rs` | `PolyBot` construction (`new`, `with_ownership` / `with_store` / `with_signer` / `with_api_key`), accessors, and the supervised run loop (scan / poll / reconcile / user-event / shutdown ticks). One concern per file below — the same layout as `module-copy`. |
| `src/discover.rs` | `scan_once` → `discover` (Gamma catalogue) → `quotes_for` (market-websocket cache, REST book fallback) → `process_market` (strategy verdicts, skips metered) → `build_signal` (frozen `OrderSignal`). |
| `src/pipeline.rs` | `process_signal` / `run_pipeline`: the 17 stages, the ONE risk decision, the ONE idempotency key, the ownership claim, `sign_signal`, `post_live`, write-ahead journaling, `record_outcome` (journal + metrics + audit); `PipelineCtx`, the in-flight guard, `new_tracked`. |
| `src/lifecycle.rs` | `apply_observation` (the single door for venue facts), `book_fill` (fill journal → trade → position → OMS), `poll_orders_once`, `apply_user_event`, `cancel_tracked_order` / `cancel_all_tracked` (venue-confirmed only), `finish_locally`, `confirm_fill_and_kill_quantity`, the tracker map and OMS transition helpers. |
| `src/reconcile.rs` | `ReconKind`, `ReconFinding`, `reconcile_once` (orphans, missing-on-venue, matched-size mismatch, ambiguous submits, positions without orders, stale orders). |
| `src/recovery.rs` | `RecoveryAction`, `RecoveryReport`, `recover_after_restart` (journal + OMS re-adoption, ambiguous held, stale paper / unsent failed, immediate live reconciliation). |
| `src/funding.rs` | `CollateralSnapshot`, `available_collateral`, `read_collateral`, `ensure_live_funding`, `resolve_sizing_balance` — the paper/live money separation (§7). |
| `src/store.rs` | The `PolyStore` journal trait, the migration-0014 record types, `MemoryPolyStore`, and `journal_order` (the engine's order-snapshot write path). |
| `src/metrics.rs` | Every `poly_*` series (and the two shared `bot_*` families the engine feeds) behind one helper each; `publish_gauges`. |
| `src/audit.rs` | `AUDIT_ACTOR`, the `poly.signal.* / poly.order.* / poly.recon.* / poly.recovery.*` vocabulary, `sanitize`. |
| `src/venue.rs` | `ensure_api_key` (L1 → L2), `authed_client`, heartbeat, `order_status` (server reconciliation truth), kill-switch `cancel_all` / `flatten`. |
| `src/strategy.rs` | `market_gate`, `quote_gate`, `evaluate_market` → `Vec<Verdict>` (`Enter(OrderDecision)` / `Skip{reason}`), `round_size`. Pure functions. |
| `src/orders.rs` | `OrderSignal` (frozen intent), `intent_key`, `PolyStage`, `RejectReason`, `LocalOrderState` machine, `VenueOrderState` parsing, `TrackedOrder::apply_venue` (cumulative fill accounting), order building and EIP-712 signing (`sign_order_bundle`). |
| `src/clob.rs` | L1/L2-authenticated CLOB client: `post_order`, `cancel_order(s)`, `order`, `open_orders`, `trades`, `heartbeat`, tick size, books. |
| `src/ws.rs` | Public market feed (books) and the authenticated **user** channel: `run_user_feed`, `parse_user_message` → `UserEvent::{Order, Trade}`. |
| `src/auth.rs` / `src/eip712.rs` | API key derivation (L1 signature) and CLOB V2 typed-data hashing. |
| `src/collateral.rs` / `src/ctf.rs` | ERC-20 balance/allowance/decimals and ERC-1155 (CTF) balance reads over JSON-RPC. |
| `crates/core/src/risk.rs` | `check_polymarket_coded`, `check_entry` (Polymarket branch: price band, liquidity floor, `poly_*` caps, re-entry cooldown). |
| `crates/core/src/oms.rs` | Order state machine shared by all modules (`Unknown` may only leave to a terminal state). |
| `crates/core/src/db/polymarket.rs` + `migrations/0014_polymarket_trading.sql` | `PolyRepo` over the four journal tables (§10). |
| `crates/server/src/recon.rs` (`DbPolyStore`) | Postgres implementation of `PolyStore`; every failed write is metered (`poly_journal_errors_total`). |
| `crates/server/src/persist.rs` | Unchanged consumer: `OrderSent`/`Trade` events for venue `polymarket` create/attach OMS records and enqueue `polymarket_order` reconciliation. |

---

## 3. The signal model

```
Gamma market ──► strategy::evaluate_market ──► Verdict::Enter(OrderDecision)
                                                      │
                                     PolyBot::build_signal (tick size from the
                                     venue, GTD expiry from config, mode)
                                                      ▼
                                              OrderSignal  (frozen intent)
                                                      │
                                              PolyBot::process_signal
```

`OrderDecision` is what a strategy wants: token, outcome, side (buy only in
the shipped strategies), `size_tokens`, `limit_price`, `stake_usd`,
`condition_id`, `neg_risk`, human `reason`.

`OrderSignal` freezes it together with everything that changes the venue
order: execution mode, upper-cased order type, tick size, GTD expiration and
the market question. Its identity is

```
intent_key = sha256("poly-intent-v1|condition|token|side|price@tick|size@0.01|ORDER_TYPE|expiration|mode")
signal_id  = "psig_" + intent_key[..32]
```

`created_at` is **not** part of the identity: the same decision produced by
two scans, two threads or two process lives is one intent. Re-pricing is
never done in place — a new price is a new intent (§8.4).

An intent key maps to **at most one OMS order for as long as the OMS
remembers it**: within a process life a venue-rejected or failed order keeps
its key, so the identical decision is `DUPLICATE_INTENT` rather than a retry
storm at the same price; after a restart the OMS reloads only non-terminal
orders, so a terminal one no longer blocks. Restart recovery fails OMS orders
that can never be on the venue (§ recovery doc) precisely so they do not come
back as `Unknown` on every start.

---

## 4. Strategies and their gates

`evaluate_market(market, quotes, cfg, now)` returns one `Verdict` per outcome
it considered (or one market-level skip). `evaluate` keeps only the entries
for callers that do not care about the explanation; `PolyBot::process_market`
records every skip as a `poly_strategy_skips_total{strategy,reason}` sample.

| Gate | Skip reason | Rule |
|---|---|---|
| market | `MARKET_INACTIVE` / `MARKET_CLOSED` / `NOT_ACCEPTING_ORDERS` | Gamma flags. |
| market | `RESOLVING_SOON` | `end_date - now < min_time_to_resolution_secs` (0 = off). |
| market | `LOW_LIQUIDITY` | `liquidity < min_liquidity_usd` (0 = off). |
| stake | `INVALID_STAKE` | `stake_usd` not finite or ≤ 0. |
| strategy | `UNKNOWN_STRATEGY` | `strategy` not `value|arb|arbitrage|search|keyword|keywords`. |
| value | `NOT_BINARY` | the basket invariant needs exactly two outcomes. |
| quote | `NO_QUOTE` | no book snapshot for the token. |
| quote | `ONE_SIDED_BOOK` | bid or ask not a finite positive number. |
| quote | `CROSSED_BOOK` | `ask < bid`. |
| quote | `PRICE_OUT_OF_RANGE` | ask outside `[0.01, 0.99)`. |
| quote | `SPREAD_TOO_WIDE` | `ask - bid > max_spread` (0 = off). |
| quote | `STALE_QUOTE` | snapshot older than `quote_max_age_secs` (0 = off; untimestamped quotes count as fresh). |
| value | `NO_EDGE` | `1 - (ask_a + ask_b) < min_edge`. |
| search | `NO_KEYWORDS_CONFIGURED` / `NO_KEYWORD_MATCH` | `watch_keywords` empty / no hit in question+slug (case-insensitive). |
| sizing | `SIZE_TOO_SMALL` | rounded size `< min_order_size` or 0. |

Sizing inside the strategies: `value` buys `floor(stake_usd / (ask_a + ask_b), 0.01)`
tokens of **each** outcome (the basket costs at most `stake_usd`); `search`
buys `floor(stake_usd / ask, 0.01)` tokens of the first outcome. Neither looks
at the wallet — the balance check is the risk engine's (§7).

---

## 5. Pipeline stages and rejection reasons

`process_signal(signal, market, quotes, cfg)` walks the stages below in
order and stops at the first failure. The outcome (`PolyOutcome`) carries the
last stage reached, the reject reason, a detail string, timings, and the OMS
/ venue / position ids when they exist. Every outcome is journaled to
`poly_signals` (upsert by `signal_id`), metered
(`poly_stage_total{stage}`, `poly_signals_total{outcome}`,
`poly_rejections_total{reason}`, `poly_pipeline_latency_ms`) and audited
(`poly.signal.<stage>`).

| # | Stage | What is checked | Reject reasons |
|---|---|---|---|
| 1 | `RECEIVED` → `VALIDATED` | token / condition id present, price finite and inside `(0,1)`, size finite and positive | `INVALID_TOKEN`, `INVALID_PRICE`, `INVALID_SIZE` |
| 2 | `MARKET_RESOLVED` | `market_gate` on the Gamma document passed in | `MARKET_NOT_TRADEABLE`, `RESOLVING_SOON`, `LOW_LIQUIDITY` |
| 3 | `QUOTED` | `quote_gate` on the current book (the caller's snapshot, refreshed by `ingest_quote` from the market feed) | `NO_QUOTE`, `STALE_QUOTE`, `SPREAD_TOO_WIDE`, `BAD_BOOK`, `PRICE_OUTSIDE_BAND` |
| 4 | `EXPOSURE_CHECKED` | no open position for the token (`AppState::find_open`), no tracked non-terminal order for the token, `max_open_markets` over positions + tracked orders, symbol gating | `ALREADY_IN_MARKET`, `ORDER_ALREADY_OPEN`, `MAX_OPEN_MARKETS`, `SYMBOL_GATED` |
| 5 | `SIZED` | `round_size`, `min_order_size` | `SIZE_TOO_SMALL` |
| 6 | `RISK_APPROVED` | the available collateral is read first (live: on-chain USDC balance of the funder; paper: the demo balance), then **the one risk decision** (§7), which may resize down | `NOT_CONFIGURED` (live without RPC / signer address), `COLLATERAL_UNAVAILABLE`, `KILL_SWITCH`, `MODULE_DISABLED`, `EMERGENCY_DISABLED`, `DAILY_LOSS_LIMIT`, `MAX_OPEN_POSITIONS`, `OPEN_ORDER_CAP`, `EXPOSURE_CAP`, `INSUFFICIENT_BALANCE`, `RISK_REJECTED` (price band, liquidity floor, re-entry cooldown, …), `SIZE_TOO_SMALL` (risk-resized below the minimum) |
| 7 | `COLLATERAL_VERIFIED` | live only: on-chain balance **minus the collateral already committed by our resting buys** ≥ approved stake, and for EOA signers (`signature_type = 0`) the settling exchange (`neg_risk` selects which) holds an ERC-20 allowance ≥ stake; read failures reject, there is no fallback | `INSUFFICIENT_FUNDING`, `COLLATERAL_UNAVAILABLE` |
| 8 | `IDEMPOTENT` | in-flight claim, `OrderManager::get_by_key(intent_key)`, then `OrderManager::create` | `DUPLICATE_INTENT`, `OMS_REJECTED` |
| 9 | `OWNERSHIP_CLAIMED` | `Permit::acquire("poly:entry:<token>")`; fenced before sending | `OWNED_BY_OTHER_REPLICA`, `OWNERSHIP_UNAVAILABLE` |
| 10 | `SIGNED` | CLOB V2 order built and signed; venue order id = struct hash | `SIGNING_FAILED` |
| 11 | `SUBMITTED` | `POST /order` (live) or in-process fill (paper) | `VENUE_REJECTED` (definite `success:false`), `SUBMIT_UNKNOWN` (transport failure → stage `AMBIGUOUS`, §8.3) |
| — | `FILLED` / `RESTING` / `AMBIGUOUS` | terminal pipeline stages; `RESTING` hands the order to the lifecycle tracker (§8) | |

Rejections before stage 8 have **no side effects** other than the journal row
and metrics. From stage 8 on, an OMS order exists; a rejection at stage 9–11
transitions it to `Failed` with the reason.

---

## 6. Exposure and sizing

* `size_tokens` is always `floor(x, 0.01)`; `stake = size × limit_price`.
* The risk engine may return a smaller `sized_quote` (per-order cap,
  balance fraction, available collateral); the pipeline rescales
  `size = floor(approved / price, 0.01)` **at the same price** and re-checks
  `min_order_size`. The OMS record, the signed order and the journal all carry
  the approved size — never the strategy's request.
* `max_open_markets` counts distinct condition ids over open positions *and*
  tracked non-terminal orders. A second leg in an already-open market is
  allowed; a new market beyond the cap is `MAX_OPEN_MARKETS`.
* Resting buy orders count as exposure: `resting_quote` (sum of unfilled
  `size × price` over tracked orders) is added to positions for
  `poly_max_total_exposure_quote`, and per-market for
  `poly_max_market_exposure_quote`.

---

## 7. Risk controls (the one authoritative decision)

Both calls happen inside stage 6 and nowhere else in the crate.

`RiskEngine::check_polymarket_coded(risk, stake, open_orders, resting_quote, market_quote)`:

| Check | Code → reject reason |
|---|---|
| preflight (kill switch, module disabled, generic daily loss, `poly_emergency_disable`, `poly_daily_loss_limit_quote`) | `KILL_SWITCH` / `MODULE_DISABLED` / `DAILY_LOSS_LIMIT` / `EMERGENCY_DISABLED` |
| stake finite and > 0 | `InvalidSize` → `INVALID_SIZE` |
| `poly_max_open_orders` (resting CLOB orders) | `PolyOpenOrderCap` → `OPEN_ORDER_CAP` |
| `poly_max_total_exposure_quote` over positions + resting + this order | `ExposureCap` → `EXPOSURE_CAP` |
| `poly_max_market_exposure_quote` inside one condition id | `PolyMarketExposure` → `EXPOSURE_CAP` |

`RiskEngine::check_entry(EntryRequest{module: Polymarket, …})` (generic path,
shared with the other modules): price band `poly_price_floor..=poly_price_ceiling`,
`poly_min_liquidity_usd`, `max_open_positions` / `poly_max_concurrent_positions`,
duplicate symbol, **re-entry cooldown** (`risk.reentry_cooldown_secs`, applies
to a token closed recently — set to `0` to allow immediate re-entry), balance
reserve and per-order sizing (`poly_max_position_quote`, else
`max_position_quote`, `max_position_fraction`). A `RiskRejected` event is
published for every refusal, exactly as for the sniper.

Two properties worth knowing:

* **Currency.** The generic `daily_loss_limit_quote` counter is one aggregate
  across modules (SOL for the Solana modules). Polymarket realized PnL is
  booked in USDC into the same counter. Operators who run Polymarket next to
  the Solana modules should size `daily_loss_limit_quote` with that in mind
  or rely on `poly_daily_loss_limit_quote` (USDC, Polymarket-only).
* **Re-entry cooldown.** After a position on a token closes, a new entry on
  that token is `RISK_REJECTED` (`reentry_cooldown`) for
  `reentry_cooldown_secs`. This is evaluated before the idempotency stage, so
  a re-issued identical intent after a close is reported as the cooldown, not
  as a duplicate, until the window passes.

---

## 8. Live order lifecycle

### 8.1 States

`TrackedOrder.state` (`LocalOrderState`) is the engine's own truth for one
venue order; the OMS order mirrors it through `oms_status()`.

| Local | Meaning | OMS |
|---|---|---|
| `submitted` | signed and POSTed, no venue answer applied yet | `Submitted` |
| `resting` | venue says `live`/`delayed`/`unmatched`, nothing matched | `Accepted` |
| `partially_filled` | `0 < size_matched < size_tokens` | `PartiallyFilled` |
| `filled` | venue `matched`: the whole order for `GTC`/`GTD`/`FOK`; for `FAK` exactly the quantity the venue reported (the killed remainder is never booked, so `remaining()` may be > 0) | `Filled` (terminal) |
| `cancelled` / `expired` | venue confirmed | `Cancelled` / `Expired` (terminal) |
| `unknown` | ambiguous POST, or an accepted order the venue no longer reports | `Unknown` |
| `failed` | definite venue rejection, or recon proved the order never existed | `Failed` (terminal) |

Transitions are checked (`can_transition_to`): terminal states are final,
`resting` cannot regress to `submitted`, `partially_filled` cannot regress to
`resting`, `unknown` may go anywhere (it is a lack of knowledge, not a
state). The OMS has a stricter rule inherited from TASK 1: **`Unknown` may
only leave through a terminal state or `Reconciled`.** An ambiguous submit
that turns out to be resting therefore stays `Unknown` in the OMS (still
counted as open) while `poly_orders.state` carries `resting`/`partially_filled`;
it converges when the order fills, cancels or expires.

### 8.2 Observations and fill accounting

Every source produces a `VenueObservation` and goes through one function,
`TrackedOrder::apply_venue`, which computes the **delta** between the venue's
cumulative `size_matched` and what has already been booked:

| Source (`FillSource`) | Where from | Semantics |
|---|---|---|
| `submit` | the `POST /order` response | status only. `matched` is the whole order for `GTC`/`GTD`/`FOK` (those types report it only when everything matched). For `FAK` it is an acknowledgement, not a quantity: nothing is booked, one immediate `GET /data/order` fetches `size_matched`, and if the venue cannot answer yet the order stays open (acknowledged) for poll / user channel / reconciliation — a killed remainder is never fabricated into a fill |
| `poll` | `GET /data/order?order_id=` every `order_poll_interval_secs` | cumulative `size_matched`; `404`/`null` for an accepted order → `unknown` (never invented as a fill or a cancel) |
| `user_ws` | authenticated user channel (§8.5) | `order` messages are cumulative (a lower cumulative than booked is ignored as stale); `trade` messages are per-trade deltas keyed by `trade_id`, booked once across `MATCHED → MINED → CONFIRMED` and reconciled against the cumulative already captured by polls (below); `FAILED` trades are ignored |
| `recon` | `reconcile_once` (§9) | cumulative, from the open-order list or `GET /data/order` |

Guards: a cumulative above `size_tokens` is refused (`lifecycle` error, no
booking); duplicate `trade_id`s are dropped; terminal orders ignore late
messages; a `GET /data/order` payload without a parseable `size_matched` is
"not reported" (`None`), never a zero. A positive delta books **one** trade:
`AppState::record_trade` (position open / add, `PositionUpdate` event,
`poly_fills_total{source}`), `poly_fills` journal row (`fill_id =
trade:<id>` when the venue named the trade, else
`pfill_<sha256(venue_order_id|size_matched|source)>` so the same cumulative
observation from two sources cannot be booked twice), `poly_orders` upsert,
OMS transition, and — on terminal states — an audit `poly.order.<state>`.

**Per-trade deltas vs polled cumulatives.** The two sources measure the same
quantity in different coordinates, and a trade event can arrive *after* a
poll already captured its fill (the channel was reconnecting; the first
sighting is the `MINED` re-emission). `TrackedOrder` therefore keeps
`trade_matched`, the sum of the distinct per-trade deltas seen in this
process, and a trade books only the excess of that sum over `size_matched`
(`max(trade_matched, size_matched) − size_matched`), never
`size_matched + delta`. Cumulative observations that name their trades
(`associate_trades` on `GET /data/order`, the open list and `order` events)
remember those ids — later events for them are no-ops — and re-base
`trade_matched` to the reported cumulative, so a genuinely new trade books
in full immediately. Without named trades a trade the poll captured first is
never double-booked; at worst a new trade is booked when the next poll
confirms it (under-booking for one poll interval, never over-booking).
`trade_matched` is not journaled: after a restart it starts at zero and is
re-based by the first cumulative that names its trades.

**Replay across restarts.** The in-memory `booked_trade_ids` list dies with
the process; the `poly_fills` journal does not, and it is the authority. When
the journal already holds the fill of a **per-trade** observation (a user
channel `trade` re-emitted as `MINED`/`CONFIRMED` after a restart), the
observation is discarded: the local cumulative and state stay where they
were, only the trade id is remembered, and
`bot_duplicate_execution_prevented_total{where="poly_fill_journal"}` counts
it. A **cumulative** observation whose fill row already exists (the process
died between the fill insert and the order-snapshot upsert) still advances
the local snapshot to venue truth — the ledger was written before the crash
— without booking the trade or the position a second time.

### 8.3 Ambiguous submits

A transport-level failure of `POST /order` (timeout, 5xx, connection reset)
means the signed order **may** be resting. The pipeline returns stage
`AMBIGUOUS`, the OMS order goes `Unknown`, the tracked order is journaled in
`unknown`, the ownership permit is released with hand-off (the venue may own
the order now) and nothing is retried. Reconciliation resolves it: found on
the venue → `resolved_<state>` and any matched size is booked from venue
truth; definitively absent (`404`) → `marked_failed` and the token is free.

### 8.4 Cancel, TTL, expiry, reprice, shutdown, heartbeat

`poll_orders_once` applies, after the venue answers, the local rules in this
order and issues **one** `DELETE /order` for the first that matches:

1. `ttl` — `order_ttl_secs > 0` and the order has rested longer (a partial
   fill is kept; only the remainder is cancelled).
2. `expired` — a GTD whose `expiration` has passed but the venue has not yet
   reported it.
3. `reprice` — `reprice_threshold > 0` and the best ask moved more than that
   above a resting BUY. The order is cancelled; the **next scan** produces a
   new decision at the new price, which is a new intent. Nothing is amended
   in place.

A cancel is applied locally only when the venue confirms it
(`canceled` list); a refused cancel (`not_canceled`) leaves the order open and
is retried on the next poll. An answer that names neither list (empty body,
unrecognised shape) is **not** a confirmation: the engine reads
`GET /data/order` and applies whatever the venue says through the lifecycle
machine (`cancelled`/`expired` → closed with any partial fill kept; still
`live` → stays open, retried next poll; absent → left to the poll's
"vanished" path and reconciliation). `cancel_all` (kill switch) follows the
same rule per order: only ids in the venue's `canceled` list are closed
locally, the rest stay open until venue truth says otherwise.
`cancel_on_shutdown = true` cancels every tracked live order when the module
stops. `heartbeat = true` posts the CLOB
dead-man's switch every `heartbeat_interval_secs` so the venue itself cancels
resting orders if the process disappears (config validation warns when both
are off).

### 8.5 User websocket channel

With `use_user_websocket = true` and credentials, `ws::run_user_feed`
subscribes to the CLOB `user` channel with
`{"auth":{apiKey,secret,passphrase},"type":"user","markets":[…]}` for the
markets that have tracked orders, sends a `PING` every 10 s and reconnects
with 1 → 30 s backoff. Parsed `UserEvent`s are delivered to the run loop
(`Tick::User`) and applied through `apply_user_event` → `apply_venue`.
Polling **always** runs as well: the channel lowers latency, it is not the
source of truth. Events for order ids the engine does not track are ignored
(they belong to another replica or a human), so a shared API key is safe.

---

## 9. Reconciliation (local ↔ venue)

`reconcile_once` runs every `reconcile_interval_secs` and after restart
recovery. In live mode with credentials it lists the venue's open orders for
our API key (`GET /data/orders`) and compares:

| `ReconKind` | Condition | Action (`poly_recon_findings.action`) |
|---|---|---|
| `orphan_venue_order` | open on the venue, unknown locally | `reported`, or — when `reconcile_cancel_orphans = true` — `cancelled` (named in the venue's `canceled` list), `cancel_refused` (named in `not_canceled`), `cancel_unconfirmed` (answer named neither), `cancel_failed` (request error); anything but `cancelled` is retried next run |
| `local_order_missing_on_venue` | tracked non-terminal order not in the open list | `GET /data/order`: `resolved_<state>` (filled / cancelled / expired, fills booked); absent → `marked_failed` if the venue never acknowledged it (`submitted`/`unknown` with no venue status), `marked_unknown` if it had (resting, partially filled, or a `matched` FAK whose quantity was never read) |
| `ambiguous_submit_resolved` | a `submitted`/`unknown` order found on the venue | `resolved_<state>`, matched size booked from venue truth |
| `matched_size_mismatch` | venue `size_matched` reported and ≠ local | applied (delta booked), detail `venue matched X vs local Y`; an open-list row without a parseable `size_matched` is not a mismatch |
| `position_without_order` | an open Polymarket position whose token has no tracked order (e.g. a legacy or manual fill) | `reported` |
| `stale_order` | a non-terminal order whose TTL (`order_ttl_secs`) or GTD expiry has passed but that polling has not managed to cancel | `reported` |

Paper mode reconciles against the position book only and never calls the
venue. Every finding is journaled (`poly_recon_findings`), metered
(`poly_recon_findings_total{kind}`) and audited (`poly.recon.<kind>`).

**Positions against the chain (server side).** `reconcile_once` compares
records with the venue's order book; what is actually *held* is verified by
the server's recovery worker, exactly as for Solana positions: every
`recovery.position_recheck_interval_secs` each open LIVE Polymarket position
is re-armed as a `polymarket_position:<id>` claim, resolved by
`PolymarketPositionTruth` (`crates/server/src/recon.rs`) against the
funder's settled outcome-token balance (`ctf.rs`, ERC-1155 `balanceOf`, six
decimals) through the shared reconciliation engine and the one correction
policy (`settle_position_outcome`: flags, fill-justified corrections only).
A non-terminal Polymarket OMS order on the token marks the snapshot as
"execution in flight" so resting / partially matched orders never trigger a
correction; a failed read retries and is never treated as zero. The claim
kind is raised only when `[polymarket].ctf_rpc_url` is set; unresolved
`polymarket_position` claims gate Module 3 only (`docs/RECONCILIATION.md`
§6/§9).

---

## 10. Persistence (migration `0014_polymarket_trading.sql`)

Additive only; nothing in earlier migrations is altered. All tables are
written through `PolyRepo` (`crates/core/src/db/polymarket.rs`) behind the
`PolyStore` trait; the module keeps working with `MemoryPolyStore` when no
database is configured (state is then in-process only — see the recovery
document).

| Table | Key | Purpose |
|---|---|---|
| `poly_signals` | `signal_id` | one row per intent, upserted with the latest stage / reject reason / detail / ids. |
| `poly_orders` | `venue_order_id` | lifecycle snapshot of every venue (or paper) order: state, `size_matched`, `position_id`, `replica_id`, `closed_at`. Partial index on open rows. |
| `poly_fills` | `fill_id` | every booked delta with `source` (`submit`/`poll`/`user_ws`/`recon`) — insert-once (`ON CONFLICT DO NOTHING`). |
| `poly_recon_findings` | `id` | append-only findings with `kind`, `action`, `detail`, `replica_id`. |

The OMS (`orders`, `order_executions`) and `positions` / `trades` tables from
earlier migrations keep their roles; `poly_orders.order_id` joins to the OMS
order and `position_id` to the position.

---

## 11. Metrics

| Metric | Labels | Meaning |
|---|---|---|
| `poly_signals_total` | `outcome` | pipeline outcomes by terminal stage |
| `poly_stage_total` | `stage` | every stage reached |
| `poly_rejections_total` | `reason` | reject reasons |
| `poly_strategy_skips_total` | `strategy`,`reason` | strategy-level skips |
| `poly_pipeline_latency_ms` | — | histogram, decision → outcome |
| `poly_quote_age_ms` | — | age of the quote used at decision time |
| `poly_orders_total` | `result` | venue orders by terminal result (`filled`, `cancelled`, `expired`, `failed`) |
| `poly_order_transitions_total` | `from`,`to` | lifecycle transitions |
| `poly_fills_total` | `source` | booked fill deltas |
| `poly_cancel_total` | `reason` | cancel requests by `ttl`/`expired`/`reprice`/`shutdown`/`recon`/`cancel_all`/manual (requests, not confirmations) |
| `poly_open_orders` (gauge) | — | tracked non-terminal orders |
| `poly_exposure_usd_milli` (gauge) | — | positions + resting, in milli-USDC |
| `poly_recon_findings_total` | `kind` | reconciliation findings |
| `poly_recovery_actions_total` | `action` | restart recovery actions (`adopted_journal_order`, `adopted_oms_order`, `held_ambiguous`, `failed_stale_paper`, `failed_unsent`) |
| `poly_journal_errors_total` | `op` | failed journal writes/reads (module and `DbPolyStore`) |
| `bot_duplicate_execution_prevented_total` | `where=poly_pipeline` / `poly_fill_journal` | duplicates stopped by the idempotency layers |

Audit actions (actor `polymarket`): `poly.signal.<stage>`,
`poly.order.<terminal state>`, `poly.recon.<kind>`, `poly.recovery.<action>`.

---

## 12. Configuration reference

`[polymarket]` (all keys are in `config.toml.example`; defaults in
`PolymarketConfig::default()`):

| Key | Default | Effect |
|---|---|---|
| `enabled` | `false` | spawn the module |
| `clob_url`, `gamma_url`, `data_url`, `ws_url` | production endpoints | venue endpoints (override for a mock) |
| `chain_id`, `exchange_domain_version`, `exchange_address`, `neg_risk_exchange_address`, `collateral_address`, `conditional_tokens_address` | Polygon / CLOB V2 | signing domain and contracts |
| `ctf_rpc_url` | `https://polygon-rpc.com` | JSON-RPC for collateral and CTF reads; empty disables live entries |
| `signature_type`, `funder_address` | `0`, unset | EOA / proxy / safe / deposit-wallet signing |
| `order_type` | `GTC` | `GTC` / `GTD` / `FOK` / `FAK`; normalised to upper case |
| `stake_usd` | `5.0` | USDC per decision (basket total for `value`) |
| `expiration_secs` | `3600` | GTD only |
| `strategy`, `min_edge`, `watch_keywords` | `value`, `0.03`, `[]` | strategy selection and parameters |
| `scan_interval_secs`, `max_open_markets` | `60`, `5` | scan cadence and market cap |
| `use_websocket`, `heartbeat`, `heartbeat_interval_secs` | `true`, `true`, `10` | market feed, dead-man's switch |
| `builder_code` | unset | fee attribution (bytes32 hex) |
| `max_spread` | `0.10` | quote gate |
| `min_liquidity_usd` | `0.0` | market gate |
| `quote_max_age_secs` | `120` | quote gate |
| `min_time_to_resolution_secs` | `3600` | market gate |
| `min_order_size` | `5.0` | tokens |
| `order_poll_interval_secs` | `15` | status polling |
| `order_ttl_secs` | `0` | local cancel after resting this long (0 = never) |
| `reprice_threshold` | `0.0` | cancel-and-requote distance (0 = off) |
| `use_user_websocket` | `true` | user channel |
| `cancel_on_shutdown` | `true` | cancel resting orders on stop |
| `reconcile_interval_secs` | `60` | reconciliation cadence (0 = off) |
| `reconcile_cancel_orphans` | `false` | cancel unknown venue orders (else report) |

`[risk]` Polymarket keys (`0` = inherit / off): `poly_max_position_quote`,
`poly_max_total_exposure_quote`, `poly_max_market_exposure_quote`,
`poly_max_concurrent_positions`, `poly_max_open_orders`,
`poly_daily_loss_limit_quote`, `poly_emergency_disable`; plus the pre-existing
`poly_min_liquidity_usd`, `poly_min_edge`, `poly_price_floor`,
`poly_price_ceiling`, and the shared `reentry_cooldown_secs`.

Secrets come from the environment only: `POLYGON_PRIVATE_KEY` (or
`POLYMARKET_PRIVATE_KEY`) and optionally `POLY_API_KEY` / `POLY_API_SECRET` /
`POLY_API_PASSPHRASE` (otherwise derived with an L1 signature at start-up).
No secret is ever written to the journal, the audit trail or logs.

Validation (`config.rs::validate`) rejects negative or non-finite values,
probabilities outside `[0, 1]` (`max_spread`, `reprice_threshold`), an
`order_type` other than `GTC|GTD|FOK|FAK`, `GTD` without `expiration_secs`,
`order_poll_interval_secs = 0` and `max_open_markets = 0`; it **warns** when
`reconcile_interval_secs = 0`, when `reconcile_cancel_orphans` is on, and when
both `heartbeat` and `cancel_on_shutdown` are off. An unknown `strategy` is
not a config error: every market is skipped with `UNKNOWN_STRATEGY`.
`bundled_example_config_parses` asserts every key above is present in
`config.toml.example`.

---

## 13. Tests

`cargo test -p module-polymarket -- --test-threads=1` (all suites use an
in-process axum mock of the CLOB, Gamma, Polygon RPC and the user websocket;
no network):

| Suite | Tests | Covers |
|---|---|---|
| unit (`src/*`) | 94 | strategy verdicts and gates, intent identity, lifecycle state machine and fill accounting (incl. FAK `matched` semantics, late trade events after a poll, `associate_trades` re-basing, the EOA funder/signer guard), user-message parsing, EIP-712 vectors, clients |
| `tests/order_pipeline.rs` | 10 | every stage, gates, exposure caps, the single risk decision, collateral verification, `process_market` |
| `tests/order_lifecycle.rs` | 11 | rest → poll → fill, TTL with partial fill, venue rejection, ambiguous submits (both resolutions), vanished orders, reprice/expiry cancels, refused cancels, unrecognised cancel answers (status re-read, never assumed), cancel-all confirmed per id, FAK `matched` booked only at the venue-reported quantity |
| `tests/user_ws.rs` | 5 | cumulative order updates, per-trade deltas booked once, trade events a poll already captured never booked twice (named trades remembered, new trades still immediate), a real websocket server (auth frame, ordering, clean stop), bot-channel end to end |
| `tests/idempotency_concurrency.rs` | 4 | 8 concurrent identical signals → 1 order, restart + shared OMS, semantic intent identity, cross-replica ownership |
| `tests/reconciliation.rs` | 6 | orphans (report / cancel policy), missing-on-venue resolution, ambiguous resolution booked once, matched-size drift, position-without-order / stale, paper never calls the venue |
| `tests/crash_recovery.rs` | 7 | journal re-adoption with booked quantity (no double booking), ambiguous held and resolved, OMS-only adoption, paper recovery without credentials, never-sent OMS orders failed, fills replayed after a restart booked once (per-trade replays discarded, cumulative catch-up without re-booking), write-ahead journaling before the POST |
| `tests/strategy_sizing.rs` | 5 | both basket legs booked and deterministic, search sizing and rounding, GTD/FOK reaching the venue, exposure caps from the shared risk engine, concurrent-position cap and price band |
| `tests/mock_clob_gamma.rs` | 5 | pre-existing client tests (kept) |

Core: `crates/core/tests/db_integration.rs` covers migration 0014 and
`PolyRepo` round-trips; `risk.rs` unit tests cover every `poly_*` cap.

---

## 14. Known limitations

* **Buy-side only.** The shipped strategies only buy; positions are closed by
  market resolution (redemption is outside this module) or manually. There is
  no automated exit/TP-SL for Polymarket positions.
* **Basket legs are independent orders.** `value` posts two GTC orders; one
  may fill while the other rests. The unfilled leg is exposed to the price
  moving away — TTL/reprice bound the time, not the outcome.
* **Resting at the ask is a taker-priced maker order.** A resting BUY at the
  ask that does not fill immediately usually means the book moved; the engine
  does not chase.
* **OMS `Unknown` semantics.** See §8.1: an ambiguous submit later confirmed
  resting stays OMS-`Unknown` until terminal.
* **In-memory journal without a database.** Without Postgres,
  `MemoryPolyStore` is used and restart recovery can only adopt what the OMS
  (also in-memory in that configuration) remembers — i.e. nothing. Live
  trading without a database is not a supported configuration.
* **Single API key per process.** The user channel subscribes with the
  process's key; orders placed by other keys are only visible through the
  open-order list of *this* key.
* **No fee modelling.** Polymarket CLOB fees are venue-side and depend on the
  market; the engine treats `stake = size × price` as the committed notional.
* **On-chain position verification needs the Polygon reader.** Without
  `[polymarket].ctf_rpc_url` no `polymarket_position` claim is raised: the
  order lifecycle is still reconciled against the CLOB, but what is held is
  then known only from booked fills. The position truth source is exercised
  offline through the shared engine's unit tests (`compare_position`,
  observation conversion, claim-kind gating) — not against a live Polygon
  node.
* **FAK quantities come only from the venue.** A fill-and-kill order's
  `matched` answer carries no quantity; the engine books exactly what
  `GET /data/order`, a user-channel `trade` or reconciliation reports. If the
  venue acknowledges `matched` and then never reports the order again, the
  order is held `unknown`, reconciliation marks it `failed` with nothing
  booked, and the `polymarket_position` chain truth (when configured) is the
  backstop that surfaces tokens held without a booked fill. The venue's
  exact `matched`-with-partial-fill reporting for FAK has not been observed
  against the live CLOB (see §14, first-run checklist in
  `POLYMARKET-OPERATIONS.md`).
