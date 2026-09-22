# Trading modules guide

All modules share one rule: **every intent passes the global risk engine
before execution**, and every fill/rejection is published on the event bus
(dashboard, WS feed, journal, Postgres). Modules can be toggled at runtime
(`/api/modules/:name/enable|disable`, Telegram `/on` `/off`) without
restarting.

Execution modes (`[execution] mode`): `paper` (default — simulated fills,
nothing leaves the box) → `simulate` (builds + simulates real transactions)
→ `live` (broadcasts; additionally requires `allow_live_trading = true`).

## Module 1 — Sniper (`[sniper]`)

Buys freshly launched tokens on pump.fun (bonding curve), PumpSwap (pool
creation) and Raydium AMM v4 (`initialize2` pool creation) and manages exits.
Full engine reference: `SNIPER-ENGINE.md` (lifecycle, reason codes, gates,
slippage engine, replay, failure-recovery map, metrics, runbook).

* **Feeds (stackable):** PumpPortal WS launch feed (`use_pumpportal`,
  pump.fun only), `logsSubscribe` (`use_log_subscription`) on the pump
  program plus the PumpSwap / Raydium AMM v4 programs when `trade_pumpswap` /
  `trade_raydium` are on, and Geyser `transactionSubscribe`
  (`use_transaction_subscribe`, needs `network.geyser_ws_url`). Every source
  is normalised into one `LaunchEvent` with a deterministic event id; the
  pipeline deduplicates through the one authoritative launch dedup, so
  running several feeds is redundancy, not double-buying.
* **Entry pipeline:** `DETECTED → VALIDATED → RISK_APPROVED →
  EXECUTION_READY → SUBMITTED → CONFIRMED`; every refusal carries a
  machine-readable reason (`STALE_EVENT`, `DUPLICATE_EVENT`,
  `INSUFFICIENT_LIQUIDITY`, `SLIPPAGE_LIMIT`, `EXPOSURE_LIMIT`,
  `RISK_REJECTED`, `EXECUTION_UNAVAILABLE`, `KILL_SWITCH`,
  `STRATEGY_DISABLED`, `INVALID_ROUTE`, …) that is logged, audited and
  counted.
* **Entry guards:** `max_entry_latency_ms` (observe → hand-off budget),
  `max_launch_age_secs`, creator/keyword denylists, optional socials and
  creator-buy minimums and market-cap ceiling (in `[risk]`); measurable
  safety gates (`min_liquidity_sol`, `require_mint_authority_revoked`,
  `require_freeze_authority_revoked`, `max_creator_initial_buy_sol`,
  `min_pool_supply_fraction`, `max_snapshot_age_ms`, `strict_gates`); the
  slippage engine (`slippage_mode`, per-protocol / per-mint overrides,
  `max_price_impact_bps`, all bounded by `risk.max_slippage_bps`); and the
  sniper exposure controls in `[risk]` (`sniper_max_position_quote`,
  `sniper_max_total_exposure_quote`, `sniper_max_concurrent_positions`,
  `sniper_max_pending_executions`, `sniper_token_cooldown_secs`,
  `sniper_failed_entry_cooldown_secs`, `sniper_daily_loss_limit_quote`,
  `sniper_emergency_disable`) — all evaluated by the shared risk engine.
* **Routing:** pump.fun bonding curve, PumpSwap direct (`trade_pumpswap`,
  including graduated pump.fun tokens), Raydium AMM v4 direct
  (`trade_raydium`), Jupiter fallback (`use_jupiter_fallback`). Every entry
  and exit goes through the hardened execution engine with a deterministic
  intent id. Account layouts are learned from chain
  (`pump_learn_account_layout`) and cached (`pump_layout_file`).
* **Exits (`monitor_positions`):** take-profit / stop-loss / trailing stop
  (fractions: `take_profit_pct = 1.0` = +100%), partial TP via
  `take_profit_sell_fraction`, `max_hold_secs` time exit, kill-switch
  flatten, failed-entry cleanup (`failed_entry_cleanup`), stale-position
  exit (`stale_position_exit_secs`), retry backoff
  (`exit_retry_backoff_secs`). Positions are marked and sold on the venue
  they were opened on. Exit sweepers are shutdown-aware: on stop they keep
  flattening per policy instead of stranding positions.
* **Replay:** recorded launch sequences under
  `crates/module-sniper/fixtures/replay/` reproduce detection, validation,
  risk decision and the would-be intent id without any network or
  transaction (`cargo test -p module-sniper --test replay`).

## Module 2 — Copy trading (`[copy]`)

Mirrors tracked wallets ("smart money").

* **Feed:** `pumpportal` / `logs_poll` / `transaction_subscribe`
  (`poll_interval_ms`, `poll_signature_limit` for the polling modes).
* **Per-wallet sizing** (`[[copy.wallets]]`): `fixed_sol` (absolute spend),
  else `fraction_of_their_size` with `max_sol` ceiling and `min_sol` floor,
  `buys_only`, per-wallet `slippage_pct` and `max_staleness_secs` (skip
  trades seen too late — protects against replayed/stale feed data).
* **Exits:** `mirror_exits` sells when the copied wallet sells;
  `full_exit_on_their_exit` exits the whole mirrored position;
  `skip_if_sniper_holds` avoids double exposure with Module 1.
* **Decoders:** pump.fun, PumpSwap, Raydium AMM v4, Jupiter
  (`decode_*` flags) — a tracked wallet trading any of these is understood,
  and the policy stage refuses venues whose decoder is off (`VENUE_DISABLED`).
* **Engine (TASK 3, `docs/COPY-TRADING-ENGINE.md`):** every decoded trade
  becomes a `LeaderTradeEvent` with a deterministic id and walks a staged
  pipeline — shape validation, leader registry (`ACTIVE` / `PAUSED` /
  `REMOVED`, config hot-reloaded), the one authoritative dedup keyed by the
  event, per-leader ordering (`strict_ordering`), pure policy, pure sizing
  (`max_sol_per_trade`, `max_balance_fraction`, `min_mirror_sol`), the shared
  risk engine (`check_copy_coded` + `check_entry` with the `[risk] copy_*`
  controls: per-entry / total / concurrent / per-leader exposure, pending
  cap, failed-entry cooldown, copy-only daily loss, emergency disable), the
  cross-replica permit and the shared executor with a deterministic intent
  id. Outcomes are journaled (`copy_events`, `copy_links`, `copy_leaders`,
  migration 0013), audited (`copy.entry.*` / `copy.exit.*` / `copy.leader.*` /
  `copy.recon.*` / `copy.recovery.*`) and metered (`copy_*`). Leader ↔
  follower reconciliation runs every `reconcile_interval_secs`
  (`reconcile_auto_exit` optionally sells when the leader fully exited);
  restart recovery re-seeds dedup from the journal and repairs links
  (`docs/COPY-TRADING-RECOVERY.md`). Runbook:
  `docs/COPY-TRADING-OPERATIONS.md`.

## Module 3 — Polymarket (`[polymarket]`)

Prediction-market value trading on the CLOB v2 (Polygon).

* **Endpoints/addresses:** CLOB + Gamma + data-api + WS are configured;
  exchange / neg-risk-exchange / collateral (pUSD) / CTF addresses default
  to the **current live V2 deployments** (legacy V1 is deprecated).
* **Signing:** EIP-712 V2 order structs (11 fields incl. `builder`),
  `signature_type` 0–3 (EOA / proxy / safe / deposit wallet) with optional
  `funder_address`; optional pre-derived L2 creds (`poly_api_*` secrets).
* **Strategies:** `value` (basket edge scan: `min_edge`,
  `poly_min_liquidity_usd`, price floor/ceiling from `[risk]`) or `search`
  (keyword watchlist). `stake_usd` notional per order, `max_open_markets`
  cap, `scan_interval_secs`, WS market channel + heartbeat.
* **Order types:** GTC / GTD (`expiration_secs`) / FOK / FAK. Paper mode
  fills locally against the order book snapshot; live mode posts signed
  orders and tracks real status (matched/live/canceled/expired feeds the
  reconciliation truth source).
* **Live money separation:** LIVE sizing runs against a verified on-chain
  collateral read (ERC-20 `balanceOf` + `decimals` on
  `collateral_address` via `ctf_rpc_url`, freshness-bounded to 15 s); the
  cached dashboard balance and the paper demo figure are **never** used in
  live mode. Before a live order is broadcast, the funder's balance AND
  (for EOA signing) the settling exchange's ERC-20 allowance must cover the
  risk-approved notional — any read that cannot be verified REJECTS the
  entry with a typed error (`BalanceUnavailable` / `InsufficientFunding`).
  The demo balance exists only for paper/simulate, which never broadcast.
  Module 1 applies the same rule to SOL: a failed balance read falls back
  to the cached demo figure in paper mode only; simulate/live propagate the
  RPC error into a risk rejection.
* **Engine (TASK 4, `docs/POLYMARKET-ENGINE.md`):** every strategy decision
  is frozen into an `OrderSignal` with a semantic intent key (market, token,
  side, price on the venue tick, size on the 0.01 grid, order type, expiry,
  mode) and walks a staged pipeline — validation, market and quote gates
  (spread, staleness, time-to-resolution, liquidity), exposure (open
  position / open order / `max_open_markets`), sizing, the shared risk
  engine (`check_polymarket_coded` + `check_entry` with the `[risk] poly_*`
  controls: per-order, total and per-market exposure incl. resting orders,
  concurrent positions, open-order cap, Polymarket-only daily loss,
  emergency disable, price band, re-entry cooldown), on-chain collateral
  verification, the OMS idempotency key, the cross-replica permit
  (`poly:entry:<token>`, fenced), signing and the POST. Live orders are
  tracked through a checked state machine fed by status polling
  (`order_poll_interval_secs`), the authenticated user websocket channel
  (`use_user_websocket`) and reconciliation, with cumulative-fill
  accounting (never a double booking), TTL / GTD / reprice cancels
  (`order_ttl_secs`, `reprice_threshold`), cancel-on-shutdown and the CLOB
  heartbeat. Local state is compared with the venue's open orders every
  `reconcile_interval_secs` (orphans reported or cancelled per
  `reconcile_cancel_orphans`); held quantity is re-verified on-chain by the
  server's `polymarket_position` claims (settled CTF balance, same policy as
  Solana positions). Outcomes are journaled (`poly_signals`,
  `poly_orders`, `poly_fills`, `poly_recon_findings`, migration 0014),
  audited (`poly.signal.*` / `poly.order.*` / `poly.recon.*` /
  `poly.recovery.*`) and metered (`poly_*`). Restart recovery re-adopts open
  orders from the journal with their booked quantity and holds ambiguous
  submits for reconciliation (`docs/POLYMARKET-RECOVERY.md`). Runbook:
  `docs/POLYMARKET-OPERATIONS.md`.

## Module 4 — Staking program (`[contract]`)

The on-chain program is managed **out of process** (deploy/initialize/
genesis via the Solana CLI or the instruction builders — see
docs/STAKING.md). The `[contract]` config block holds the operational
reference data (program id, mint, token metadata, params mirror) and
`dry_run = true` keeps any bot-side interaction read-only; `token_supply`
there is informational — the BINDING total-supply cap is the immutable
on-chain `max_supply` set at `Initialize` and enforced against every mint
(genesis fails over-cap with 6028; reward minting clamps to the remaining
headroom so withdrawals never fail). Token metadata (name/symbol/uri) is
created once, admin-only, via `CreateTokenMetadata` — an immutable
mpl-token-metadata CPI; the bot never mints and never rewrites metadata.

## Module 5 — Telegram (`[telegram]`)

Remote control + alerting (see docs/API.md §Telegram for the role model).

* **Alerts:** fills, risk rejections, feed disconnects, daily-loss trip,
  hourly PnL summary — each with `alert_cooldown_secs` anti-spam and
  `max_alerts_per_minute` budget.
* **Token:** read from the env var named by `bot_token_env`
  (`TELEGRAM_BOT_TOKEN`), never stored in the config file.
* Deny-by-default: with empty allowlists the bot answers nothing.

## Cross-module safety

* Kill switch (API/Telegram/risk) halts all new intents instantly; exits
  continue.
* Daily realized-loss limit auto-disables the trading modules until the next
  UTC day; consecutive failures auto-disable the affected module.
* SOL reserve floor (`min_sol_reserve`), slippage ceiling
  (`max_slippage_bps`), re-entry and copy cooldowns, repeat-offender creator
  blocking — all enforced centrally in `[risk]`, not per module.
* **Global risk + accounting layer (TASK 5, `[global_risk]`,
  `docs/GLOBAL-RISK.md`, `docs/ACCOUNTING-LEDGER.md`):** one portfolio-level
  decision runs in front of every module's own checks — portfolio / wallet /
  venue / strategy / asset exposure caps, an open-position cap, a per-order
  notional cap, a daily-loss cap and a drawdown cap, all in one reference
  currency, plus per-venue and per-strategy kill switches (config-pinned or
  operator-engaged at runtime). Its inputs come from the global ledger:
  every sniper, copy and Polymarket fill is reported as one typed
  accounting event, booked exactly once (deterministic event id), expanded
  into balanced double-entry postings and aggregated into one position book
  the modules never touch. Module positions and trades are reconciled
  against the ledger periodically; discrepancies are reported, never
  repaired. Every limit defaults to `0` = off. Runbook:
  `docs/RISK-OPERATIONS.md`.
