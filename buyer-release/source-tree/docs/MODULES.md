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

Buys freshly launched pump.fun tokens and manages exits.

* **Feeds (stackable):** PumpPortal WS launch feed (`use_pumpportal`),
  pump-program `logsSubscribe` (`use_log_subscription`), and Geyser
  `transactionSubscribe` (`use_transaction_subscribe`, needs
  `network.geyser_ws_url`) — deduplicated by launch key, so running several
  feeds is redundancy, not double-buying.
* **Entry guards:** `max_entry_latency_ms` (end-to-end age budget),
  `max_launch_age_secs`, creator/keyword denylists, optional socials and
  creator-buy minimums and market-cap ceiling (in `[risk]`).
* **Routing:** pump.fun bonding curve, graduated PumpSwap (`trade_pumpswap`),
  Raydium (`trade_raydium`), Jupiter fallback (`use_jupiter_fallback`).
  Account layouts are learned from chain (`pump_learn_account_layout`) and
  cached (`pump_layout_file`).
* **Exits (`monitor_positions`):** take-profit / stop-loss / trailing stop
  (fractions: `take_profit_pct = 1.0` = +100%), partial TP via
  `take_profit_sell_fraction`, `max_hold_secs` time exit. Exit sweepers are
  shutdown-aware: on stop they keep flattening per policy instead of
  stranding positions.

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
  (`decode_*` flags) — a tracked wallet trading any of these is understood.

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
