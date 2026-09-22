# Polymarket Operations Runbook

Operator-facing companion to [POLYMARKET-ENGINE.md](POLYMARKET-ENGINE.md)
(how the engine decides) and [POLYMARKET-RECOVERY.md](POLYMARKET-RECOVERY.md)
(what happens after a crash). It covers enabling the module, moving from
paper to live, the emergency controls, what to watch, how to tune the
lifecycle knobs and how to act on reconciliation findings.

None of the procedures below make trading safe or profitable; they make the
engine's behaviour predictable and its state inspectable.

---

## 1. Enabling the module

1. Set `[polymarket].enabled = true` and keep `execution.mode = "paper"`.
   Paper mode needs **no** signer, API key or RPC: decisions walk the full
   pipeline, orders exist in the OMS (`/api/orders`, module `polymarket`),
   fills are booked in-process, and the journal tables fill up exactly as in
   live mode. Nothing is POSTed.
2. Pick the strategy: `strategy = "value"` (basket edge, both outcomes of a
   binary market) or `"search"` with `watch_keywords` (first outcome of a
   matching market). Set `stake_usd`, `min_edge`, and the gates
   (`max_spread`, `min_liquidity_usd`, `quote_max_age_secs`,
   `min_time_to_resolution_secs`). Every gate produces an explicit skip reason
   you can see in `poly_strategy_skips_total{strategy,reason}`.
3. Decide the exposure controls in `[risk]` **before** going live:
   `poly_max_position_quote`, `poly_max_total_exposure_quote`,
   `poly_max_market_exposure_quote`, `poly_max_concurrent_positions`,
   `poly_max_open_orders`, `poly_daily_loss_limit_quote`. Every `0` inherits
   the generic limit or disables the check — decide each one explicitly. Note
   that `reentry_cooldown_secs` (shared) also applies to Polymarket tokens.
4. With Postgres configured (`[database]`, `auto_migrate = true`) migration
   `0014_polymarket_trading` applies on connect and the durable journal
   (`poly_signals`, `poly_orders`, `poly_fills`, `poly_recon_findings`) is
   used automatically. **Live trading without a database is not a supported
   configuration** — restart recovery has nothing to adopt from.

Startup log lines to expect: `polymarket signer loaded` (or the warning `no
POLYMARKET_PRIVATE_KEY set — running read-only (paper fills only)`),
`polymarket restart recovery complete` (only when something was adopted),
then one scan per `scan_interval_secs`.

### Going live — checklist

| Step | Why |
|---|---|
| `POLYGON_PRIVATE_KEY` (or `POLYMARKET_PRIVATE_KEY`) in the environment | signs CLOB V2 orders; also derives the L2 API key at start-up unless `POLY_API_KEY` / `POLY_API_SECRET` / `POLY_API_PASSPHRASE` are provided |
| `signature_type` and `funder_address` match how the funds are held | `0` = the EOA itself holds USDC; `1/2/3` = proxy / safe / deposit wallet at `funder_address`. With type `0` a `funder_address` that is not the signing key's address is refused locally (`SIGNING_FAILED`, nothing is posted) |
| `ctf_rpc_url` reachable, `collateral_address` correct | live sizing reads the USDC balance (and decimals) on chain; if the read fails every live entry is `COLLATERAL_UNAVAILABLE` |
| EOA signers: ERC-20 allowance granted to `exchange_address` **and** `neg_risk_exchange_address` | without it entries are `INSUFFICIENT_FUNDING` (the check reads the allowance of the exchange that would settle the order) |
| `execution.mode = "live"` **and** `execution.allow_live_trading = true` | both are required, as for every module |
| `heartbeat = true` or `cancel_on_shutdown = true` (ideally both) | otherwise resting orders survive a crash unattended (config validation warns) |
| `reconcile_interval_secs > 0` | local state is compared with the venue's open orders; `0` warns |
| decide `reconcile_cancel_orphans` | `false` (default) only reports venue orders the engine does not know; `true` cancels them — do not enable while another process trades with the same API key |

First live minutes: watch `poly_signals_total{outcome}`; the first order
should appear as `poly.signal.resting` (GTC/GTD) or `poly.signal.filled`
(FOK or an immediately matched order) in the audit trail, with an OMS
order in `Submitted → Accepted` and a `poly_orders` row in `resting`.

`order_type = "FAK"` specifically: the venue's `matched` answer to the POST
carries no quantity, so the engine books nothing from it. It reads
`GET /data/order` once immediately (audit `poly.order.filled` with
`matched=<reported>/<size>` when the venue answers) and otherwise leaves the
order open — `poly_orders.state = submitted`, `venue_status = matched` —
until the next poll, a user-channel `trade` or reconciliation reports the
quantity. Before relying on FAK live, place one small FAK order and confirm
in `poly_fills` that `size_tokens` equals the venue's `size_matched` (the
CLOB's partial-fill reporting for FAK has not been observed against the live
venue by this repository's tests). A FAK the venue acknowledged but then
never reported again ends as `local_order_missing_on_venue` /
`marked_unknown` → `marked_failed` with nothing booked; the
`polymarket_position` chain truth is the backstop for tokens held without a
booked fill.

---

## 2. Emergency controls

| Goal | Control | Effect |
|---|---|---|
| stop everything now | kill switch (`POST /api/kill`, Telegram, `risk.kill_switch`) | every module: entries refused by preflight (`KILL_SWITCH`); the Polymarket loop keeps polling, reconciling and applying user-channel events but places nothing |
| stop new Polymarket entries only | `risk.poly_emergency_disable = true` (hot reload) | `EMERGENCY_DISABLED` for new entries; polling, cancels, reconciliation and fills of already-resting orders continue |
| pull resting orders | stop the process with `cancel_on_shutdown = true` (default) | `cancel_all_tracked("shutdown")` cancels every tracked live order one by one and applies only venue-confirmed cancels locally (an answer that names neither `canceled` nor `not_canceled` triggers a `GET /data/order` re-read, never an assumption); refused or unconfirmed cancels are logged and left for the next start's reconciliation |
| venue-side dead-man's switch | `heartbeat = true` (default) | the CLOB cancels our resting orders if heartbeats stop for its window (process death, network partition) |
| disable the module | `POST /api/modules/polymarket/disable` or `[polymarket].enabled = false` | loop keeps running in `disabled` detail, no scans, no orders; existing orders are **not** cancelled — use the shutdown path or cancel manually first |
| lower size immediately | `polymarket.stake_usd`, `risk.poly_max_position_quote` (hot reload) | next decision |

Everything above is audited (`poly.signal.*`, `poly.order.*`) and visible in
`/api/audit`.

---

## 3. What to watch

### Metrics (`/metrics`)

| Signal | Healthy | Investigate when |
|---|---|---|
| `poly_signals_total{outcome}` | mostly `rejected` with strategy/exposure reasons, a few `resting`/`filled` | `ambiguous` grows (venue/network trouble), `failed` grows (`VENUE_REJECTED` — check allowance, tick size, minimum size) |
| `poly_rejections_total{reason}` | dominated by `NO_EDGE`, `ALREADY_IN_MARKET`, `ORDER_ALREADY_OPEN`, `MAX_OPEN_MARKETS` | `COLLATERAL_UNAVAILABLE` (RPC), `INSUFFICIENT_FUNDING` (balance/allowance), `OWNERSHIP_UNAVAILABLE` (claim store), `SUBMIT_UNKNOWN` |
| `poly_open_orders` (gauge) | ≤ `risk.poly_max_open_orders` | stuck at a value while `poly_order_transitions_total` is flat → polling/credentials problem |
| `poly_exposure_usd_milli` (gauge) | within your caps | drifting up without fills → resting orders accumulating |
| `poly_order_transitions_total{from,to}` | `submitted→resting`, `resting→filled/partially_filled/cancelled` | `→unknown` growing (venue lost orders or 404s), `→failed` growing |
| `poly_fills_total{source}` | `poll` and `user_ws` both non-zero in live | only `poll` → the user channel is not connected (credentials, `use_user_websocket`) |
| `poly_cancel_total{reason}` | `ttl`/`reprice` at the rate you tuned | `shutdown`/`recon` spikes |
| `poly_recon_findings_total{kind}` | zero or `stale_order` | `orphan_venue_order` (another process on the same key, or a lost journal), `matched_size_mismatch` repeatedly (user-channel or poll gaps) |
| `poly_journal_errors_total{op}` | zero | any value: journal writes are failing — the engine keeps trading on its in-memory state but restart recovery will be incomplete; fix the database before restarting |
| `bot_duplicate_execution_prevented_total{where=poly_pipeline}` | occasional | continuous: two scanners producing the same intent (expected with several replicas) |
| `bot_reconciliation_outcomes_total{kind="polymarket_position",outcome}` | `in_sync` (live, `ctf_rpc_url` set) | `quantity_mismatch` / `unexpected_position` / `missing_position` → a `drift_flag` / `unexpected_position` / `recon_correction` risk event names the position; `external_unavailable` with `bot_external_state_read_errors_total{source="polygon_ctf"}` rising → the Polygon RPC is unreadable (positions stay entry-gated until it answers) |

### Audit / journal

* `poly.signal.<stage>` — one per decision; `detail=` explains rejections.
* `poly.order.<filled|cancelled|expired|failed>` — one per terminal order.
* `poly.recon.<kind>` and `poly.recovery.<action>`.

Useful queries:

```sql
-- open venue orders as the engine sees them
select venue_order_id, token_id, state, size_matched, size_tokens, replica_id, submitted_at
from poly_orders where closed_at is null order by submitted_at;

-- why decisions were rejected in the last hour
select reject_reason, count(*) from poly_signals
where updated_at > now() - interval '1 hour' and stage = 'REJECTED'
group by 1 order by 2 desc;

-- fills booked per source today
select source, count(*), sum(quote_usd) from poly_fills
where ts::date = current_date group by 1;

-- reconciliation findings that required an action
select ts, kind, action, venue_order_id, detail from poly_recon_findings
where action <> 'reported' order by ts desc limit 50;
```

---

## 4. Tuning

| Symptom | Knob | Notes |
|---|---|---|
| orders rest for a long time, then fill badly | `order_ttl_secs` (cancel remainder), `reprice_threshold` (cancel when the ask moves away; the next scan re-quotes) | a partial fill is always kept; only the remainder is cancelled |
| too many `STALE_QUOTE` skips | `quote_max_age_secs`, `use_websocket = true` | the market feed refreshes quotes between scans |
| too many `SPREAD_TOO_WIDE` | `max_spread` | probability units (0.02 = 2 cents) |
| entering markets that resolve soon | `min_time_to_resolution_secs` | default 1 h |
| `SIZE_TOO_SMALL` after risk resize | raise `stake_usd` / `poly_max_position_quote` or lower `min_order_size` (never below the venue's minimum) | the resize keeps the price and rounds the size to 0.01 |
| basket legs fill asymmetrically | lower `order_ttl_secs`, consider `order_type = "FOK"` (all-or-nothing per leg) | legs are independent orders by design |
| user channel reconnect storms in logs | leave it: backoff is 1 → 30 s; polling continues | check credentials if it never connects |
| `poly_max_open_orders` reached with nothing filling | lower `stake_usd`, raise the cap, or shorten TTL | resting orders count as exposure |

Everything in `[polymarket]` and `[risk]` is hot-reloadable; the loop reads a
fresh config snapshot on every tick, and intents already frozen keep the
parameters they were frozen with.

---

## 5. Handling reconciliation findings

| Finding (`kind` / `action`) | Meaning | What to do |
|---|---|---|
| `orphan_venue_order` / `reported` | the venue has an open order for our API key that this process does not track | another process on the same key? a journal lost before restart? If it is ours and unwanted, cancel it (§6) or set `reconcile_cancel_orphans = true` temporarily |
| `orphan_venue_order` / `cancelled` | policy cancelled it | verify in the venue UI; check why it was unknown locally |
| `orphan_venue_order` / `cancel_refused` | venue refused (`not_canceled`) | usually already matched or cancelled; the next pass will not see it |
| `local_order_missing_on_venue` / `resolved_filled` | the venue matched it while we were not looking; fills booked from venue truth | none — check the position |
| `local_order_missing_on_venue` / `marked_unknown` | an accepted order vanished from the open list and `GET /data/order` is 404 | wait one more pass (venue lag); if it persists, inspect the venue UI and close manually (§6) |
| `local_order_missing_on_venue` / `marked_failed` | a never-accepted order is definitively absent | none — token is free again |
| `ambiguous_submit_resolved` / `resolved_*` | an ambiguous POST turned out to exist | none; note the OMS order stays `Unknown` until terminal (engine doc §8.1) |
| `matched_size_mismatch` | venue `size_matched` differed from the booked amount; the delta was booked | frequent occurrences mean the user channel / polling missed updates — check connectivity |
| `position_without_order` | an open Polymarket position without a tracked order | legacy or manual fill; close it manually if unwanted |
| `stale_order` | TTL / GTD passed but the cancel did not go through | polling retries every tick; if it stays, cancel manually |

---

## 6. Manual procedures

* **Cancel one order.** Use the venue UI or the CLOB API with the same API
  key. The next poll sees `canceled` and books nothing; the local state moves
  to `cancelled`, the OMS to `Cancelled`. Never edit `poly_orders` by hand.
* **Cancel all.** Stop the process (`cancel_on_shutdown = true`) or, without
  stopping, set `risk.poly_emergency_disable = true` and let TTL/reprice
  drain, or cancel in the venue UI.
* **A position the engine does not know.** It appears as
  `position_without_order`; the engine never sells on its own. Redeem or
  sell manually; the position book is updated by the operator's usual
  tooling.
* **Rotating the API key.** Restart with the new `POLY_API_*` values (or let
  it re-derive). Orders placed under the old key stay visible to the old key
  only — cancel them first.

---

## 7. Failure modes and expected behaviour

| Failure | Behaviour |
|---|---|
| CLOB unreachable during `POST /order` | stage `AMBIGUOUS`, OMS `Unknown`, order journaled `unknown`, no retry; reconciliation resolves it (found → booked; 404 → failed) |
| CLOB rejects the order (`success:false`) | `VENUE_REJECTED`, OMS `Failed`, token free immediately |
| RPC down | live entries `COLLATERAL_UNAVAILABLE`; resting orders keep being polled |
| Postgres down | journal writes fail (`poly_journal_errors_total`), the engine continues on in-memory state; restart recovery would be incomplete — restore the database before restarting |
| user websocket down | fills arrive via polling only (latency = `order_poll_interval_secs`) |
| venue returns 404 for an accepted order | local `unknown`, never a fill or a cancel; reconciliation keeps checking |
| venue reports `size_matched` above the order size | observation refused (`lifecycle` error, nothing booked), logged; investigate the venue payload |
| two replicas produce the same intent | one `poly:entry:<token>` claim wins; the other is `OWNED_BY_OTHER_REPLICA`; the shared OMS idempotency key stops a second order even after the claim expires |
| process killed mid-poll | see the recovery document; no double booking because `size_matched` is journaled with every observation |

---

## 8. Daily checks

1. `poly_journal_errors_total` is zero.
2. Open orders (`poly_orders` where `closed_at is null`) match the venue UI;
   no `stale_order` findings.
3. `poly_exposure_usd_milli` and the number of open positions are within the
   caps you set.
4. `poly_recon_findings` has no `orphan_venue_order` or repeated
   `matched_size_mismatch`.
5. Rejection mix is what you expect (`NO_EDGE`/`ALREADY_IN_MARKET` — fine;
   `COLLATERAL_UNAVAILABLE`/`INSUFFICIENT_FUNDING`/`OWNERSHIP_UNAVAILABLE` — not).
6. Realized PnL in the dashboard is consistent with the venue's account view
   (positions are only closed by resolution or by you).
