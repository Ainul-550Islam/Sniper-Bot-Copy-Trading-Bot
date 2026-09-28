use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{BotError, BotResult};
use crate::maths::LAMPORTS_PER_SIGNATURE;
use crate::models::{BotModule, ExecutionMode, Venue};

/// Top-level configuration.
///
/// Resolution order (later wins):
///   1. compiled-in defaults (`Config::default()`)
///   2. `config.toml` (path from `--config` or `CONFIG_PATH`, default `./config.toml`)
///   3. environment variables (see `apply_env_overrides`)
///
/// Secrets are *only* read from the environment, never written into `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct Config {
    pub network: NetworkConfig,
    pub execution: ExecutionConfig,
    pub risk: RiskConfig,
    /// TASK 5 — portfolio-level limits, kill switches and the reference
    /// currency of the global ledger.
    pub global_risk: GlobalRiskConfig,
    pub sniper: SniperConfig,
    pub copy: CopyConfig,
    pub polymarket: PolymarketConfig,
    pub contract: ContractConfig,
    pub telegram: TelegramConfig,
    pub api: ApiConfig,
    pub storage: StorageConfig,
    pub observability: ObservabilityConfig,
    pub recovery: RecoveryConfig,
    pub ha: HaConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub auth: AuthConfig,
    pub signing: SigningConfig,
    pub secrets: SecretConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// `mainnet-beta` or `devnet`.
    pub cluster: String,
    pub rpc_url: String,
    pub rpc_url_fallbacks: Vec<String>,
    pub ws_url: String,
    /// Geyser-style websocket (Yellowstone / Triton / Helius) used for
    /// `transactionSubscribe`, which is the lowest-latency copy-trade feed.
    pub geyser_ws_url: Option<String>,
    pub commitment: String,
    pub request_timeout_ms: u64,
    pub max_retries: u32,
    /// TTL for the warm account cache (semi-static reads: pump Global, mint
    /// owners, ATA existence). `0` disables cached reads entirely.
    /// Price-bearing accounts are never cached regardless of this value.
    pub account_cache_ttl_ms: u64,
    /// Maximum number of accounts held in the warm cache (FIFO eviction).
    pub account_cache_max_entries: usize,
    /// First retry delay of the RPC retry policy (exponential from here).
    pub retry_base_backoff_ms: u64,
    /// Ceiling for one retry delay.
    pub retry_max_backoff_ms: u64,
    /// Randomise every delay in `[0, computed]` (full jitter) so a fleet of
    /// replicas never retries in lock-step against a recovering provider.
    pub retry_jitter: bool,
    /// Minimum pause after an HTTP 429 / provider quota error before that
    /// provider is used again (a `Retry-After` hint, when present, wins).
    pub rate_limit_cooldown_ms: u64,
    /// Consecutive failures after which a provider is considered unhealthy
    /// and skipped for `provider_cooldown_ms` (automatic failover).
    pub provider_failure_threshold: u32,
    /// How long an unhealthy provider is skipped before it is probed again.
    pub provider_cooldown_ms: u64,
    /// Websocket stale-connection detector: with no inbound frame (data or
    /// pong) for this long the socket is closed and re-established with all
    /// subscriptions restored. `0` disables the detector.
    pub ws_stale_after_ms: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            cluster: "mainnet-beta".into(),
            rpc_url: "https://api.mainnet-beta.solana.com".into(),
            rpc_url_fallbacks: vec![
                "https://solana-api.projectserum.com".into(),
                "https://rpc.ankr.com/solana".into(),
            ],
            // Empty by design: `Rpc` derives the websocket URL from `rpc_url`
            // (https→wss) whenever this is blank, so overriding `rpc_url` alone
            // can never leave `ws_url` pointing at a stale mainnet endpoint.
            // Set it explicitly only for providers whose WS host differs.
            ws_url: String::new(),
            geyser_ws_url: None,
            commitment: "confirmed".into(),
            request_timeout_ms: 10_000,
            max_retries: 3,
            // pump.fun's Global account changes only on protocol-admin fee
            // updates; 30 s staleness is invisible to trading decisions but
            // removes a round trip from every snipe.
            account_cache_ttl_ms: 30_000,
            account_cache_max_entries: 5_000,
            retry_base_backoff_ms: 50,
            retry_max_backoff_ms: 2_000,
            retry_jitter: true,
            rate_limit_cooldown_ms: 1_000,
            provider_failure_threshold: 3,
            provider_cooldown_ms: 5_000,
            ws_stale_after_ms: 45_000,
        }
    }
}

impl NetworkConfig {
    pub fn is_devnet(&self) -> bool {
        self.cluster.contains("devnet") || self.rpc_url.contains("devnet")
    }

    /// Every configured HTTP endpoint, primary first, blanks and duplicates
    /// removed (the RPC provider pool is built from this list).
    pub fn rpc_endpoints(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(1 + self.rpc_url_fallbacks.len());
        for url in std::iter::once(&self.rpc_url).chain(self.rpc_url_fallbacks.iter()) {
            let u = url.trim();
            if !u.is_empty() && !out.iter().any(|x| x == u) {
                out.push(u.to_string());
            }
        }
        out
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionConfig {
    /// `paper` | `simulate` | `live`.
    pub mode: ExecutionMode,
    /// Hard gate: even with `mode = "live"` nothing is broadcast unless this is
    /// true. Flip it deliberately, never by accident.
    pub allow_live_trading: bool,
    /// Route Solana transactions through a Jito bundle instead of plain RPC.
    pub use_jito: bool,
    pub jito_block_engine_url: String,
    pub jito_tip_lamports: u64,
    /// Compute-unit price for the priority-fee instruction (micro-lamports).
    pub priority_fee_micro_lamports: u64,
    pub compute_unit_limit: u32,
    /// How long to wait for confirmation after broadcasting.
    pub confirm_timeout_ms: u64,
    /// Poll interval while waiting for confirmation.
    pub confirm_poll_ms: u64,
    /// Retry the whole build+send path this many times on a transient failure.
    pub send_retries: u32,
    /// Run `simulateTransaction` before broadcasting (default: true). Turning
    /// this off is the "simulate bypass" for latency-tuned setups (e.g. a
    /// warm Geyser feed + trusted builders) — it saves a round trip but lets
    /// failing transactions reach the network.
    pub simulate_first: bool,
    /// Treat a failed simulation as fatal for the attempt (default: true).
    /// When false, a failed simulation only warns and the send proceeds.
    pub abort_on_simulation_failure: bool,
    /// Fan out the signed broadcast to the primary RPC *and* every fallback
    /// endpoint concurrently; the first acceptance wins (default: false).
    /// Duplicate delivery is harmless — the leader dedupes by signature —
    /// and it materially raises the landing rate when one node lags.
    pub broadcast_fanout: bool,
    /// Priority-fee policy: `fixed` always uses `priority_fee_micro_lamports`
    /// (clamped to the bounds below); `adaptive` samples
    /// `getRecentPrioritizationFees` and pays the configured percentile of
    /// recent fees (never below `priority_fee_micro_lamports`, never above
    /// `fee_max_micro_lamports`).
    pub fee_mode: String,
    /// Lower bound for any priority fee the executor will set.
    pub fee_min_micro_lamports: u64,
    /// Upper bound: adaptive quotes and retry escalation are clamped here.
    pub fee_max_micro_lamports: u64,
    /// Emergency limit: a request that asks for MORE than this (explicitly
    /// or through escalation) is refused outright instead of clamped — a
    /// runaway fee is a bug, not a market condition.
    pub fee_emergency_max_micro_lamports: u64,
    /// Percentile (1–100) of recent prioritization fees used by `adaptive`.
    pub fee_percentile: u8,
    /// Fee increase per retry attempt, in percent of the previous attempt's
    /// fee (a blockhash-expired rebuild pays more to land faster).
    pub fee_escalation_pct: u64,
    /// How long one `getRecentPrioritizationFees` sample stays valid.
    pub fee_oracle_ttl_ms: u64,
    /// Maximum age of a cached blockhash the executor will still sign with;
    /// older hashes are refreshed before signing.
    pub max_blockhash_age_ms: u64,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        ExecutionConfig {
            mode: ExecutionMode::Paper,
            allow_live_trading: false,
            use_jito: false,
            jito_block_engine_url: "https://mainnet.block-engine.jito.wtf".into(),
            jito_tip_lamports: 1_000_000,
            priority_fee_micro_lamports: 250_000,
            compute_unit_limit: 400_000,
            confirm_timeout_ms: 60_000,
            confirm_poll_ms: 1_000,
            send_retries: 2,
            simulate_first: true,
            abort_on_simulation_failure: true,
            broadcast_fanout: false,
            fee_mode: "fixed".into(),
            fee_min_micro_lamports: 0,
            fee_max_micro_lamports: 5_000_000,
            fee_emergency_max_micro_lamports: 20_000_000,
            fee_percentile: 75,
            fee_escalation_pct: 50,
            fee_oracle_ttl_ms: 2_000,
            max_blockhash_age_ms: 20_000,
        }
    }
}

impl ExecutionConfig {
    /// The single source of truth for "may I really broadcast?".
    pub fn live_allowed(&self) -> bool {
        self.mode.is_live() && self.allow_live_trading
    }

    /// `true` when the adaptive priority-fee oracle is enabled.
    pub fn fee_adaptive(&self) -> bool {
        self.fee_mode.trim().eq_ignore_ascii_case("adaptive")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RiskConfig {
    /// Global kill switch. When true, every module refuses to send anything and
    /// open positions are flattened by the risk sweeper.
    pub kill_switch: bool,
    /// Max simultaneous open positions per module.
    pub max_open_positions: usize,
    /// Max fraction of quote balance in a single position (0.0 .. 1.0).
    pub max_position_fraction: f64,
    /// Absolute per-position cap in quote units (SOL for Solana, USDC for Poly).
    pub max_position_quote: f64,
    /// Stop trading for the day after losing this much quote.
    pub daily_loss_limit_quote: f64,
    /// Halt after this many consecutive failed orders.
    pub max_consecutive_failures: u32,
    /// Stop-loss as a fraction of entry (0.25 == -25%).
    pub default_stop_loss_pct: f64,
    /// Take-profit as a fraction of entry (1.0 == +100%).
    pub default_take_profit_pct: f64,
    /// Trailing stop as a fraction below the high-water mark.
    pub trailing_stop_pct: Option<f64>,
    /// Close a position after this many seconds regardless of PnL.
    pub max_hold_secs: Option<i64>,
    /// Hard cap on slippage we are willing to accept, in basis points.
    pub max_slippage_bps: u64,
    /// Minimum SOL that must remain in the wallet after any buy.
    pub min_sol_reserve: f64,
    /// Skip any token whose creator wallet has already launched and rugged.
    pub block_repeat_offender_creators: bool,
    /// Require at least this many socials (website/twitter/telegram).
    pub min_socials: usize,
    /// Skip tokens whose creator bought less than this many SOL at launch.
    pub min_creator_buy_sol: f64,
    /// Skip tokens that already launched with a market cap above this (SOL).
    pub max_launch_market_cap_sol: f64,
    /// Do not re-enter a token we already traded within this window.
    pub reentry_cooldown_secs: i64,
    /// Do not copy the same wallet+token more often than this.
    pub copy_cooldown_secs: i64,
    /// Drop any Polymarket market with less liquidity than this (USDC).
    pub poly_min_liquidity_usd: f64,
    /// Only take Polymarket positions when the model edge exceeds this.
    pub poly_min_edge: f64,
    /// Never buy above this price or below the complement on Polymarket.
    pub poly_price_floor: f64,
    pub poly_price_ceiling: f64,
    /// Sniper exposure controls (Module 1 only; every value `0` = inherit
    /// the generic limit above or disable the check) ----------------------
    /// Per-token cap for a sniper entry in SOL (`0` = `max_position_quote`).
    pub sniper_max_position_quote: f64,
    /// Cap on the SUM of open sniper exposure in SOL (`0` = the generic
    /// `max_position_quote × max_open_positions` envelope).
    pub sniper_max_total_exposure_quote: f64,
    /// Max simultaneously open sniper positions (`0` = `max_open_positions`).
    pub sniper_max_concurrent_positions: usize,
    /// Max sniper execution intents that are live in the execution ledger
    /// (created/validated/submitted/pending) at once (`0` = unlimited).
    pub sniper_max_pending_executions: usize,
    /// Minimum seconds between two entry ATTEMPTS on the same mint,
    /// regardless of outcome (`0` = off).
    pub sniper_token_cooldown_secs: i64,
    /// After an entry attempt FAILED (rejected by the chain, expired, no
    /// fill), refuse the same mint for this many seconds (`0` = off).
    pub sniper_failed_entry_cooldown_secs: i64,
    /// Sniper-only daily realized loss cap in SOL (`0` = off). Independent
    /// of — and evaluated in addition to — `daily_loss_limit_quote`.
    pub sniper_daily_loss_limit_quote: f64,
    /// Emergency disable: refuse EVERY new sniper entry while true. Exits
    /// keep running (reducing exposure is always allowed). Unlike the kill
    /// switch it affects only Module 1 and only entries.
    pub sniper_emergency_disable: bool,
    /// Copy-trading exposure controls (Module 2 only; every value `0` =
    /// inherit the generic limit above or disable the check) ---------------
    /// Per-token cap for a mirrored entry in SOL (`0` = `max_position_quote`).
    pub copy_max_position_quote: f64,
    /// Cap on the SUM of open copy exposure in SOL (`0` = the generic
    /// `max_position_quote × max_open_positions` envelope).
    pub copy_max_total_exposure_quote: f64,
    /// Max simultaneously open copy positions (`0` = `max_open_positions`).
    pub copy_max_concurrent_positions: usize,
    /// Max copy execution intents that are live in the execution ledger
    /// (created/validated/submitted/pending) at once (`0` = unlimited).
    pub copy_max_pending_executions: usize,
    /// After a mirrored entry attempt FAILED on a mint, refuse that mint for
    /// this many seconds (`0` = off).
    pub copy_failed_entry_cooldown_secs: i64,
    /// Copy-only daily realized loss cap in SOL (`0` = off). Independent of —
    /// and evaluated in addition to — `daily_loss_limit_quote`.
    pub copy_daily_loss_limit_quote: f64,
    /// Cap on the open exposure mirrored from ONE leader, in SOL (`0` = off).
    pub copy_max_leader_exposure_quote: f64,
    /// Emergency disable: refuse EVERY new mirrored entry while true. Exits
    /// (including mirrored exits) keep running.
    pub copy_emergency_disable: bool,
    /// Polymarket exposure controls (Module 3 only, TASK 4; every value
    /// `0` = inherit the generic limit above or disable the check) ----------
    /// Per-order USDC cap for a Polymarket entry (`0` = `max_position_quote`).
    pub poly_max_position_quote: f64,
    /// Cap on the SUM of open Polymarket exposure in USDC — open positions
    /// PLUS resting (unfilled) buy orders (`0` = the generic envelope).
    pub poly_max_total_exposure_quote: f64,
    /// Cap on the open exposure inside ONE market (condition id), USDC,
    /// positions plus resting orders (`0` = off).
    pub poly_max_market_exposure_quote: f64,
    /// Max simultaneously open Polymarket positions (`0` = `max_open_positions`).
    pub poly_max_concurrent_positions: usize,
    /// Max resting (non-terminal) CLOB orders at once (`0` = unlimited).
    pub poly_max_open_orders: usize,
    /// Polymarket-only daily realized loss cap in USDC (`0` = off).
    /// Independent of — and evaluated in addition to — `daily_loss_limit_quote`.
    pub poly_daily_loss_limit_quote: f64,
    /// Emergency disable: refuse EVERY new Polymarket entry while true.
    /// Cancels, exits and reconciliation keep running.
    pub poly_emergency_disable: bool,
}

impl Default for RiskConfig {
    fn default() -> Self {
        RiskConfig {
            kill_switch: false,
            max_open_positions: 8,
            max_position_fraction: 0.10,
            max_position_quote: 0.5,
            daily_loss_limit_quote: 2.0,
            max_consecutive_failures: 5,
            default_stop_loss_pct: 0.30,
            default_take_profit_pct: 1.00,
            trailing_stop_pct: Some(0.25),
            max_hold_secs: Some(3600),
            max_slippage_bps: 3000,
            min_sol_reserve: 0.05,
            block_repeat_offender_creators: true,
            min_socials: 0,
            min_creator_buy_sol: 0.0,
            max_launch_market_cap_sol: 100.0,
            reentry_cooldown_secs: 600,
            copy_cooldown_secs: 120,
            poly_min_liquidity_usd: 1000.0,
            poly_min_edge: 0.03,
            poly_price_floor: 0.02,
            poly_price_ceiling: 0.98,
            sniper_max_position_quote: 0.0,
            sniper_max_total_exposure_quote: 0.0,
            sniper_max_concurrent_positions: 0,
            sniper_max_pending_executions: 2,
            sniper_token_cooldown_secs: 0,
            sniper_failed_entry_cooldown_secs: 120,
            sniper_daily_loss_limit_quote: 0.0,
            sniper_emergency_disable: false,
            copy_max_position_quote: 0.0,
            copy_max_total_exposure_quote: 0.0,
            copy_max_concurrent_positions: 0,
            copy_max_pending_executions: 0,
            copy_failed_entry_cooldown_secs: 0,
            copy_daily_loss_limit_quote: 0.0,
            copy_max_leader_exposure_quote: 0.0,
            copy_emergency_disable: false,
            poly_max_position_quote: 0.0,
            poly_max_total_exposure_quote: 0.0,
            poly_max_market_exposure_quote: 0.0,
            poly_max_concurrent_positions: 0,
            poly_max_open_orders: 0,
            poly_daily_loss_limit_quote: 0.0,
            poly_emergency_disable: false,
        }
    }
}

impl RiskConfig {
    /// Effective per-token cap for a sniper entry.
    pub fn sniper_position_cap(&self) -> f64 {
        if self.sniper_max_position_quote > 0.0 {
            self.sniper_max_position_quote
        } else {
            self.max_position_quote
        }
    }

    /// Effective concurrent-position cap for the sniper.
    pub fn sniper_position_limit(&self) -> usize {
        if self.sniper_max_concurrent_positions > 0 {
            self.sniper_max_concurrent_positions
        } else {
            self.max_open_positions
        }
    }

    /// Effective per-token cap for a mirrored (copy) entry.
    pub fn copy_position_cap(&self) -> f64 {
        if self.copy_max_position_quote > 0.0 {
            self.copy_max_position_quote
        } else {
            self.max_position_quote
        }
    }

    /// Effective concurrent-position cap for the copy module.
    pub fn copy_position_limit(&self) -> usize {
        if self.copy_max_concurrent_positions > 0 {
            self.copy_max_concurrent_positions
        } else {
            self.max_open_positions
        }
    }

    /// Effective per-order cap (USDC) for a Polymarket entry.
    pub fn poly_position_cap(&self) -> f64 {
        if self.poly_max_position_quote > 0.0 {
            self.poly_max_position_quote
        } else {
            self.max_position_quote
        }
    }

    /// Effective concurrent-position cap for the Polymarket module.
    pub fn poly_position_limit(&self) -> usize {
        if self.poly_max_concurrent_positions > 0 {
            self.poly_max_concurrent_positions
        } else {
            self.max_open_positions
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SniperConfig {
    pub enabled: bool,
    /// SOL to spend on each snipe.
    pub buy_sol: f64,
    /// Slippage tolerance in percent for the bonding-curve buy.
    pub slippage_pct: f64,
    /// Subscribe to PumpPortal's `subscribeNewToken` feed.
    pub use_pumpportal: bool,
    pub pumpportal_ws_url: String,
    pub pumpportal_api_key: Option<String>,
    /// Also watch native `logsSubscribe` on the pump program as a second source.
    pub use_log_subscription: bool,
    /// Also watch the pump program via Geyser `transactionSubscribe`
    /// (`network.geyser_ws_url` must be set). Lowest-latency launch source:
    /// processed-commitment notifications straight from a Geyser-enabled
    /// node, decoded through the same pump event parser as `logsSubscribe`.
    pub use_transaction_subscribe: bool,
    /// PumpSwap protocol: detect `CreatePoolEvent` launches on the AMM and
    /// route PumpSwap entries/exits directly through the AMM program (a
    /// pump.fun token that already graduated is routed here too). When
    /// false, PumpSwap launches are rejected with `INVALID_ROUTE` and
    /// graduated pump tokens use the Jupiter fallback only.
    pub trade_pumpswap: bool,
    /// Raydium AMM v4 protocol: detect `initialize2` pool creations and
    /// route entries/exits directly through the AMM v4 program. CLMM /
    /// CPMM / LaunchLab pools are recognised but never traded.
    pub trade_raydium: bool,
    /// Fall back to the Jupiter aggregator if no direct venue is found.
    pub use_jupiter_fallback: bool,
    /// Sell rules ----------------------------------------------------------
    pub take_profit_pct: Option<f64>,
    pub stop_loss_pct: Option<f64>,
    pub trailing_stop_pct: Option<f64>,
    pub max_hold_secs: Option<i64>,
    /// Sell this fraction of the position at take-profit, keep the rest.
    pub take_profit_sell_fraction: f64,
    /// Subscribe to `subscribeTokenTrade` for each open position to mark price.
    pub monitor_positions: bool,
    /// Account-layout template override for the pump `buy` instruction.
    ///
    /// Pump.fun has silently changed its required account list several times
    /// (volume accumulators, fee config, `bonding-curve-v2`). When a snipe
    /// fails with a custom program error, capture the account list of a
    /// *successful* transaction and paste the extra pubkeys here — they are
    /// appended after the derived accounts, in order.
    pub pump_extra_accounts: Vec<String>,
    /// Set false to stop appending `bonding-curve-v2` (only for old layouts).
    pub pump_append_bonding_curve_v2: bool,
    /// Learn the account layout by watching successful buys on chain.
    pub pump_learn_account_layout: bool,
    pub pump_layout_file: String,
    /// Latency budget from OBSERVING a launch to handing the transaction to
    /// the execution engine. Exceeding it rejects with `STALE_EVENT` right
    /// before submission (the edge is gone; do not chase it). `0` = off.
    pub max_entry_latency_ms: u64,
    /// Skip launches older than this many seconds by the time we see them.
    pub max_launch_age_secs: i64,
    /// Denylist of creator wallets (base58) that never get sniped.
    pub creator_denylist: Vec<String>,
    /// Denylist of keywords in the token name/symbol.
    pub keyword_denylist: Vec<String>,
    /// Slippage engine ------------------------------------------------------
    /// `"fixed"` uses `slippage_pct` as is; `"liquidity_aware"` widens the
    /// base tolerance for thin pools (up to the hard maximum) and tightens
    /// it for deep ones; `"price_impact"` sets the tolerance from the
    /// modelled price impact of the sized trade plus `slippage_pct` as a
    /// buffer. Every mode is clamped by `risk.max_slippage_bps`.
    pub slippage_mode: String,
    /// Per-protocol base slippage overrides in percent (`None` = inherit
    /// `slippage_pct`).
    pub pumpswap_slippage_pct: Option<f64>,
    pub raydium_slippage_pct: Option<f64>,
    /// Per-token slippage override in basis points, keyed by mint
    /// (`[sniper.slippage_overrides_bps]`). Still clamped by the hard max.
    pub slippage_overrides_bps: std::collections::BTreeMap<String, u64>,
    /// Reject an entry whose modelled price impact exceeds this (`0` = off).
    pub max_price_impact_bps: u64,
    /// Fee budget per entry transaction in lamports (`0` = off): the
    /// deterministic worst case of base fee (5 000 per signature) + priority
    /// fee at the ceiling the `[execution]` fee policy can settle on ×
    /// compute-unit limit + Jito tip (when `execution.use_jito`). Exceeding
    /// it rejects the entry as `FEE_LIMIT` before anything is built.
    pub max_entry_fee_lamports: u64,
    /// Safety gates ---------------------------------------------------------
    /// Minimum quote-side liquidity (SOL) in the curve/pool at decision time
    /// (`0` = off).
    pub min_liquidity_sol: f64,
    /// Require the mint authority to be revoked (nobody can inflate supply).
    /// Off by default: pump.fun revokes it, arbitrary Raydium tokens may not.
    pub require_mint_authority_revoked: bool,
    /// Require the freeze authority to be revoked (nobody can freeze your
    /// token account — the classic honeypot lever). pump.fun tokens always
    /// pass; this bites for Raydium-native launches.
    pub require_freeze_authority_revoked: bool,
    /// Concentration gate: reject when the creator's opening buy exceeds
    /// this many SOL (`0` = off). Only PumpPortal reports it.
    pub max_creator_initial_buy_sol: f64,
    /// Concentration gate for AMM launches: the fraction of the total supply
    /// that must sit inside the pool (`0` = off, e.g. `0.5` = at least half
    /// the supply is pooled; less means the rest can be dumped on you).
    pub min_pool_supply_fraction: f64,
    /// Reject when the market snapshot (curve/pool read) used for the
    /// decision is older than this at submission time.
    pub max_snapshot_age_ms: u64,
    /// Strict gates: when a gate cannot be evaluated because the protocol /
    /// feed does not expose the datum, REJECT instead of skipping the gate.
    pub strict_gates: bool,
    /// Exit hardening -------------------------------------------------------
    /// Force an exit when a position's mark price could not be refreshed for
    /// this many seconds (`0` = off). A blind exit at unknown price is the
    /// last resort, hence off by default.
    pub stale_position_exit_secs: i64,
    /// Minimum seconds between two exit attempts for the same position after
    /// a FAILED exit (prevents hammering the chain every sweep).
    pub exit_retry_backoff_secs: u64,
    /// Close (without selling) positions whose ENTRY provably never landed
    /// according to the execution ledger (failed / expired attempts).
    pub failed_entry_cleanup: bool,
}

impl Default for SniperConfig {
    fn default() -> Self {
        SniperConfig {
            enabled: false,
            buy_sol: 0.01,
            slippage_pct: 15.0,
            use_pumpportal: true,
            pumpportal_ws_url: "wss://pumpportal.fun/api/data".into(),
            pumpportal_api_key: None,
            use_log_subscription: true,
            use_transaction_subscribe: false,
            trade_pumpswap: true,
            trade_raydium: true,
            use_jupiter_fallback: true,
            take_profit_pct: Some(1.0),
            stop_loss_pct: Some(0.3),
            trailing_stop_pct: Some(0.25),
            max_hold_secs: Some(3600),
            take_profit_sell_fraction: 1.0,
            monitor_positions: true,
            pump_extra_accounts: Vec::new(),
            pump_append_bonding_curve_v2: true,
            pump_learn_account_layout: true,
            pump_layout_file: "data/pump_account_layout.json".into(),
            max_entry_latency_ms: 1000,
            max_launch_age_secs: 30,
            creator_denylist: Vec::new(),
            keyword_denylist: vec![
                "rug".into(),
                "scam".into(),
                "honeypot".into(),
                "test".into(),
            ],
            slippage_mode: "fixed".into(),
            pumpswap_slippage_pct: None,
            raydium_slippage_pct: None,
            slippage_overrides_bps: std::collections::BTreeMap::new(),
            max_price_impact_bps: 2_500,
            max_entry_fee_lamports: 0,
            min_liquidity_sol: 0.0,
            require_mint_authority_revoked: false,
            require_freeze_authority_revoked: true,
            max_creator_initial_buy_sol: 0.0,
            min_pool_supply_fraction: 0.0,
            max_snapshot_age_ms: 2_000,
            strict_gates: false,
            stale_position_exit_secs: 0,
            exit_retry_backoff_secs: 10,
            failed_entry_cleanup: true,
        }
    }
}

impl SniperConfig {
    /// Normalised slippage mode (`fixed` | `liquidity_aware` | `price_impact`).
    pub fn slippage_mode_normalized(&self) -> String {
        self.slippage_mode
            .trim()
            .to_ascii_lowercase()
            .replace('-', "_")
    }
}

/// One wallet that Module 2 mirrors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyWallet {
    pub address: String,
    /// Optional human label shown in Telegram alerts.
    pub label: Option<String>,
    /// Spend this fixed amount of SOL per mirrored buy. If `None`, size
    /// proportionally from `copy_fraction_of_their_size`.
    pub fixed_sol: Option<f64>,
    /// Fraction of the whale's SOL size to mirror (0.05 == 5%).
    pub fraction_of_their_size: f64,
    /// Ceiling for the proportional size, in SOL.
    pub max_sol: f64,
    /// Floor: ignore buys smaller than this many SOL.
    pub min_sol: f64,
    /// Only mirror buys (`true`) or also mirror their sells (`false`).
    pub buys_only: bool,
    /// Per-wallet slippage override in percent.
    pub slippage_pct: Option<f64>,
    /// Skip if our node sees the trade later than this (we would be too late).
    pub max_staleness_secs: i64,
    /// Operator pause: keep following (events are still observed, counted
    /// and reconciled) but mirror nothing new. Hot-reloadable.
    pub paused: bool,
    /// Cap on the open exposure mirrored from THIS leader, in SOL (`0` =
    /// only the global `risk.copy_max_leader_exposure_quote` applies).
    pub max_exposure_sol: f64,
    /// Max simultaneously open positions mirrored from THIS leader
    /// (`0` = no per-leader limit).
    pub max_open_positions: usize,
}

impl Default for CopyWallet {
    fn default() -> Self {
        CopyWallet {
            address: String::new(),
            label: None,
            fixed_sol: None,
            fraction_of_their_size: 0.05,
            max_sol: 0.25,
            min_sol: 0.05,
            buys_only: false,
            slippage_pct: None,
            max_staleness_secs: 20,
            paused: false,
            max_exposure_sol: 0.0,
            max_open_positions: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CopyConfig {
    pub enabled: bool,
    pub wallets: Vec<CopyWallet>,
    /// Preferred feed: `pumpportal` (easiest), `transaction_subscribe`
    /// (fastest, needs Geyser) or `logs_poll` (works on any RPC).
    pub feed: String,
    /// Poll interval for the `logs_poll` fallback feed.
    pub poll_interval_ms: u64,
    /// How many recent signatures to fetch per poll.
    pub poll_signature_limit: usize,
    pub slippage_pct: f64,
    /// Also close our position when the whale sells (only if we still hold).
    pub mirror_exits: bool,
    /// Sell our whole position when the whale sells, even if they sell part.
    pub full_exit_on_their_exit: bool,
    /// Ignore whale trades on tokens we already hold from Module 1.
    pub skip_if_sniper_holds: bool,
    /// Never mirror a buy on a token older than this (seconds since launch).
    pub max_token_age_secs: Option<i64>,
    /// Venues we know how to decode. Anything else is logged and skipped.
    pub decode_pumpfun: bool,
    pub decode_pumpswap: bool,
    pub decode_raydium: bool,
    pub decode_jupiter: bool,
    /// Global ceiling on how old a leader trade may be when the pipeline
    /// picks it up (seconds since the chain/source produced it, falling back
    /// to local observation). A wallet's `max_staleness_secs` can only
    /// tighten this. `0` = off.
    pub max_event_age_secs: i64,
    /// Reject a leader event that arrives BEHIND that leader's newest
    /// processed slot (`OUT_OF_ORDER`) instead of mirroring it late.
    pub strict_ordering: bool,
    /// Global cap on one mirrored buy in SOL, applied after the per-wallet
    /// rule (`0` = off).
    pub max_sol_per_trade: f64,
    /// Global cap on one mirrored buy as a fraction of the spendable balance
    /// (`0` = off, `1` = the whole balance).
    pub max_balance_fraction: f64,
    /// Mirrors smaller than this many SOL are not worth a transaction fee
    /// and are refused as dust (`0` = off).
    pub min_mirror_sol: f64,
    /// How often leader activity is reconciled against our mirrored
    /// positions (seconds; `0` = never).
    pub reconcile_interval_secs: u64,
    /// When reconciliation finds a leader that fully exited a mint we still
    /// hold, sell through the normal mirrored-exit path. Off = flag only.
    pub reconcile_auto_exit: bool,
    /// After a restart, re-seed the dedup facade from the durable
    /// processed-event journal this far back (hours; `0` = off).
    pub recovery_lookback_hours: i64,
}

impl Default for CopyConfig {
    fn default() -> Self {
        CopyConfig {
            enabled: false,
            wallets: Vec::new(),
            feed: "pumpportal".into(),
            poll_interval_ms: 2_000,
            poll_signature_limit: 25,
            slippage_pct: 20.0,
            mirror_exits: true,
            full_exit_on_their_exit: true,
            skip_if_sniper_holds: false,
            max_token_age_secs: None,
            decode_pumpfun: true,
            decode_pumpswap: true,
            decode_raydium: true,
            decode_jupiter: true,
            max_event_age_secs: 30,
            strict_ordering: false,
            max_sol_per_trade: 0.0,
            max_balance_fraction: 0.0,
            min_mirror_sol: 0.0,
            reconcile_interval_secs: 60,
            reconcile_auto_exit: false,
            recovery_lookback_hours: 24,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolymarketConfig {
    pub enabled: bool,
    pub clob_url: String,
    pub gamma_url: String,
    pub data_url: String,
    pub ws_url: String,
    pub chain_id: u64,
    /// EIP-712 domain version for the Exchange. Polymarket moved to `"2"`
    /// with the 2026-04-28 CLOB V2 cutover; `"1"` orders are rejected with
    /// `order_version_mismatch`.
    pub exchange_domain_version: String,
    /// V2 exchange contracts (Polygon). Override only if Polymarket moves again.
    pub exchange_address: String,
    pub neg_risk_exchange_address: String,
    pub collateral_address: String,
    pub conditional_tokens_address: String,
    /// Polygon JSON-RPC endpoint used to read on-chain CTF (ERC-1155)
    /// balances — the fill-SETTLEMENT truth behind reconciliation (§Q/§D).
    /// Empty disables the CTF check (venue API remains the order-lifecycle
    /// truth); an unreadable RPC is "could not read", never "no balance".
    pub ctf_rpc_url: String,
    /// 0 = EOA, 1 = Polymarket proxy wallet, 2 = Gnosis Safe, 3 = deposit wallet.
    pub signature_type: u8,
    /// Address that actually holds the funds (the `maker`). For `signature_type`
    /// 0 this is the EOA itself; for 1/2/3 it is the proxy/safe/deposit wallet.
    pub funder_address: Option<String>,
    pub order_type: String,
    /// USDC per order.
    pub stake_usd: f64,
    /// Order lifetime in seconds for GTD orders (ignored when order_type=GTC).
    pub expiration_secs: i64,
    /// Strategy: which signal set to run.
    pub strategy: String,
    /// Buy when the market price is at least this far below our fair value.
    pub min_edge: f64,
    /// Rescan the catalogue this often.
    pub scan_interval_secs: u64,
    /// How many markets to hold at once.
    pub max_open_markets: usize,
    /// Subscribe to the CLOB websocket for live prices.
    pub use_websocket: bool,
    /// Send the `/heartbeat` dead-man's-switch so the CLOB cancels our resting
    /// orders if this process dies.
    pub heartbeat: bool,
    pub heartbeat_interval_secs: u64,
    /// Builder code (bytes32 hex) if you route through a builder; zeros otherwise.
    pub builder_code: Option<String>,
    /// Keywords used by the simple `search` strategy.
    pub watch_keywords: Vec<String>,
    /// Order pipeline / lifecycle controls (TASK 4) ----------------------
    /// Reject a decision when the quoted spread (`ask - bid`) is wider than
    /// this (probability units, `0` = off).
    pub max_spread: f64,
    /// Reject a decision when the market's Gamma liquidity figure is below
    /// this many USDC (`0` = off).
    pub min_liquidity_usd: f64,
    /// Reject a decision whose quote is older than this many seconds (`0` =
    /// off; quotes without a timestamp are treated as fresh).
    pub quote_max_age_secs: i64,
    /// Reject a decision when the market resolves within this many seconds
    /// (`0` = off). Protects against entering minutes before settlement.
    pub min_time_to_resolution_secs: i64,
    /// Smallest order the engine will place, in outcome tokens (the CLOB's
    /// own minimum is 5 shares).
    pub min_order_size: f64,
    /// Poll the CLOB for the status of resting orders this often (seconds).
    pub order_poll_interval_secs: u64,
    /// Cancel a resting order that has not filled after this many seconds
    /// (`0` = never; GTD expiry still applies).
    pub order_ttl_secs: i64,
    /// Cancel + let the next scan re-quote a resting BUY whose price sits
    /// more than this far below the current best ask (probability units,
    /// `0` = off).
    pub reprice_threshold: f64,
    /// Compare local order state with the venue's open-order list this often
    /// (seconds, `0` = off).
    pub reconcile_interval_secs: u64,
    /// Cancel venue orders that are open for our API key but unknown
    /// locally (found by reconciliation). `false` = report only.
    pub reconcile_cancel_orphans: bool,
    /// Subscribe to the authenticated `user` websocket channel for order /
    /// trade events (needs API credentials; polling always runs as well).
    pub use_user_websocket: bool,
    /// Cancel every resting order when the module shuts down.
    pub cancel_on_shutdown: bool,
}

impl Default for PolymarketConfig {
    fn default() -> Self {
        PolymarketConfig {
            enabled: false,
            clob_url: "https://clob.polymarket.com".into(),
            gamma_url: "https://gamma-api.polymarket.com".into(),
            data_url: "https://data-api.polymarket.com".into(),
            ws_url: "wss://ws-subscriptions-clob.polymarket.com/ws/".into(),
            chain_id: 137,
            exchange_domain_version: "2".into(),
            exchange_address: "0xE111180000d2663C0091e4f400237545B87B996B".into(),
            neg_risk_exchange_address: "0xe2222d279d744050d28e00520010520000310F59".into(),
            collateral_address: "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB".into(),
            conditional_tokens_address: "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045".into(),
            ctf_rpc_url: "https://polygon-rpc.com".into(),
            signature_type: 0,
            funder_address: None,
            order_type: "GTC".into(),
            stake_usd: 5.0,
            expiration_secs: 3600,
            strategy: "value".into(),
            min_edge: 0.03,
            scan_interval_secs: 60,
            max_open_markets: 5,
            use_websocket: true,
            heartbeat: true,
            heartbeat_interval_secs: 10,
            builder_code: None,
            watch_keywords: Vec::new(),
            max_spread: 0.10,
            min_liquidity_usd: 0.0,
            quote_max_age_secs: 120,
            min_time_to_resolution_secs: 3600,
            min_order_size: 5.0,
            order_poll_interval_secs: 15,
            order_ttl_secs: 0,
            reprice_threshold: 0.0,
            reconcile_interval_secs: 60,
            reconcile_cancel_orphans: false,
            use_user_websocket: true,
            cancel_on_shutdown: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ContractConfig {
    /// Deployed program id of `programs/staking-suite`.
    pub program_id: Option<String>,
    /// Mint the program manages (created by `initialize`).
    pub token_mint: Option<String>,
    pub token_name: String,
    pub token_symbol: String,
    pub token_decimals: u8,
    pub token_supply: u64,
    /// Protocol fee taken on staking deposits, in basis points.
    pub fee_bps: u16,
    /// Where fees are swept to.
    pub fee_treasury: Option<String>,
    /// Annual reward rate in basis points (1000 == 10%).
    pub reward_apy_bps: u64,
    /// Minimum stake, in whole tokens.
    pub min_stake_amount: u64,
    /// Cooldown before an unstake settles, in seconds.
    pub unstake_delay_secs: i64,
    /// Keypair allowed to run admin instructions.
    pub admin_pubkey: Option<String>,
    /// Emit the instructions the CLI should send (no signing) when true.
    pub dry_run: bool,
}

impl Default for ContractConfig {
    fn default() -> Self {
        ContractConfig {
            program_id: None,
            token_mint: None,
            token_name: "Sniper Suite Token".into(),
            token_symbol: "SNPR".into(),
            token_decimals: 9,
            token_supply: 1_000_000_000,
            fee_bps: 100,
            fee_treasury: None,
            reward_apy_bps: 1_000,
            min_stake_amount: 1,
            unstake_delay_secs: 86_400,
            admin_pubkey: None,
            dry_run: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelegramConfig {
    pub enabled: bool,
    /// Bot token comes from `TELOXIDE_TOKEN` / `TELEGRAM_BOT_TOKEN`.
    pub bot_token_env: String,
    /// Only these chat ids may issue control commands.
    pub allowed_chat_ids: Vec<i64>,
    /// Only these user ids may issue control commands (checked first).
    /// Equivalent to the `operator` role in `[auth]`.
    pub allowed_user_ids: Vec<i64>,
    /// Telegram user ids with full owner rights (emergency stop, mode
    /// changes). Empty => `allowed_user_ids` members are treated as owners
    /// (backward compatible with single-operator setups).
    pub owner_user_ids: Vec<i64>,
    /// Telegram user ids limited to read-only commands (status, positions,
    /// health). Allowed users always keep their control rights.
    pub readonly_user_ids: Vec<i64>,
    /// Chat that receives alerts. Defaults to the first allowed chat id.
    pub alert_chat_id: Option<i64>,
    pub poll_interval_secs: u64,
    pub parse_mode: String,
    /// Minimum seconds between two alerts of the same kind (anti-spam).
    pub alert_cooldown_secs: i64,
    /// Never send more than this many alerts per minute.
    pub max_alerts_per_minute: u32,
    /// Send a trade alert for every fill.
    pub alert_on_fill: bool,
    /// Send an alert when the risk engine rejects an order.
    pub alert_on_risk_reject: bool,
    /// Send an alert when a module's websocket drops.
    pub alert_on_disconnect: bool,
    /// Send an alert when the daily loss limit trips.
    pub alert_on_loss_limit: bool,
    /// Send an hourly summary of PnL and open positions.
    pub hourly_summary: bool,
    /// Message prefix, useful when several bots share a chat.
    pub prefix: String,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        TelegramConfig {
            enabled: false,
            bot_token_env: "TELEGRAM_BOT_TOKEN".into(),
            allowed_chat_ids: Vec::new(),
            allowed_user_ids: Vec::new(),
            owner_user_ids: Vec::new(),
            readonly_user_ids: Vec::new(),
            alert_chat_id: None,
            poll_interval_secs: 2,
            parse_mode: "HTML".into(),
            alert_cooldown_secs: 5,
            max_alerts_per_minute: 20,
            alert_on_fill: true,
            alert_on_risk_reject: true,
            alert_on_disconnect: true,
            alert_on_loss_limit: true,
            hourly_summary: true,
            prefix: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiConfig {
    pub enabled: bool,
    pub bind_host: String,
    pub bind_port: u16,
    /// Shared secret required in the `x-api-key` header for mutating routes.
    /// Read from `API_KEY` when empty.
    pub api_key_env: String,
    pub cors_origins: Vec<String>,
    /// Serve the embedded single-file dashboard at `/`.
    pub serve_dashboard: bool,
    /// Per-principal (or per-IP when unauthenticated) request budget, in
    /// requests per minute. `0` disables limiting.
    pub rate_limit_rpm: u32,
}

impl Default for ApiConfig {
    fn default() -> Self {
        ApiConfig {
            enabled: true,
            // Safe default: bind to loopback only. To expose the control plane
            // on a reachable interface (e.g. "0.0.0.0" in a container) you MUST
            // also set an API key — the server refuses to start otherwise.
            bind_host: "127.0.0.1".into(),
            bind_port: 8080,
            api_key_env: "API_KEY".into(),
            cors_origins: vec!["*".into()],
            serve_dashboard: true,
            rate_limit_rpm: 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub data_dir: String,
    pub trades_file: String,
    pub positions_file: String,
    pub events_file: String,
    /// Flush the journal at least this often.
    pub flush_interval_ms: u64,
    /// Trim the in-memory event ring buffer to this many entries.
    pub max_events_in_memory: usize,
    /// Trim the in-memory trade list to this many entries.
    pub max_trades_in_memory: usize,
    /// Hard cap on the in-memory de-duplication sets and cooldown maps
    /// (seen launches, seen signatures, last-exit / last-copy timestamps).
    /// Prevents unbounded memory growth over long runs; the oldest entries
    /// are evicted once the cap is exceeded.
    pub max_dedup_entries: usize,
    /// Where deduplication keys live: `memory` (process-lifetime only),
    /// `redis` (shared window, survives restarts while Redis retains data)
    /// or `postgres` (durable, survives restarts — the production choice).
    /// With `redis`/`postgres` an in-memory L1 still fronts every lookup.
    pub dedup_backend: String,
    /// TTL for durable dedup keys (0 = keep forever). Signatures and launch
    /// mints older than this can never re-appear on a live feed.
    pub dedup_ttl_secs: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            data_dir: "data".into(),
            trades_file: "trades.jsonl".into(),
            positions_file: "positions.jsonl".into(),
            events_file: "events.jsonl".into(),
            flush_interval_ms: 1_000,
            max_events_in_memory: 2_000,
            max_trades_in_memory: 1_000,
            max_dedup_entries: 100_000,
            dedup_backend: "memory".into(),
            dedup_ttl_secs: 7 * 24 * 3600,
        }
    }
}

/// Logging, metrics and health-probe settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ObservabilityConfig {
    /// Base log level (`error`/`warn`/`info`/`debug`/`trace`) used when the
    /// `RUST_LOG` environment variable is not set. `RUST_LOG` always wins so
    /// operators can override without editing files.
    pub log_level: String,
    /// Log format: `text` (human-readable, development) or `json` (structured,
    /// production log pipelines).
    pub log_format: String,
    /// Serve `GET /metrics` (Prometheus text format) and record HTTP request
    /// metrics. Event/state/rpc instrumentation is cheap and always on; this
    /// flag controls the exposition surface.
    pub metrics_enabled: bool,
    /// How often the state sampler refreshes gauges and health components.
    pub sample_interval_ms: u64,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        ObservabilityConfig {
            log_level: "info".into(),
            log_format: "text".into(),
            metrics_enabled: true,
            sample_interval_ms: 5_000,
        }
    }
}

/// TASK 5 — the global risk / accounting layer (`docs/GLOBAL-RISK.md`).
///
/// Every limit is expressed in the REFERENCE currency (`reference_asset`,
/// default `USD`); native quote figures (SOL, USDC) are converted with the
/// operator-configured `reference_rates`. The suite fetches no prices for
/// this: a quote asset with open exposure but no rate makes every
/// reference-denominated check fail closed (`reference_rate_missing`).
/// Every limit defaults to `0` = off, so an unconfigured suite behaves as
/// before TASK 5.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalRiskConfig {
    /// Label of the reference currency the limits and the portfolio view
    /// are denominated in.
    pub reference_asset: String,
    /// Reference rate per quote asset (`quote → reference`), e.g.
    /// `SOL = 150.0`, `USDC = 1.0`. Operator-maintained; audited on reload.
    pub reference_rates: HashMap<String, f64>,
    /// Operator-declared capital base in reference units. Backs capital
    /// utilization and `max_drawdown_pct`. `0` = unknown.
    pub capital_base_ref: f64,
    /// Cap on total open exposure across every module (0 = off).
    pub max_portfolio_exposure_ref: f64,
    /// Cap on open exposure per wallet (0 = off).
    pub max_wallet_exposure_ref: f64,
    /// Cap on open exposure per venue (0 = off).
    pub max_venue_exposure_ref: f64,
    /// Cap on open exposure per strategy label (0 = off).
    pub max_strategy_exposure_ref: f64,
    /// Cap on open exposure per base asset (0 = off).
    pub max_asset_exposure_ref: f64,
    /// Cap on open aggregated positions across every module (0 = off).
    pub max_open_positions: usize,
    /// Cap on one order's notional (0 = off).
    pub max_order_notional_ref: f64,
    /// Stop opening once today's net realized loss reaches this (0 = off).
    pub max_daily_loss_ref: f64,
    /// Stop opening once the drawdown from the realized peak (incl.
    /// unrealized) reaches this absolute figure (0 = off).
    pub max_drawdown_ref: f64,
    /// Same, as a fraction of `capital_base_ref` (0 = off; needs the base).
    pub max_drawdown_pct: f64,
    /// Venues refused for NEW entries (`Venue::as_str` names, e.g.
    /// `polymarket`, `pump.fun`). Exits keep running.
    pub killed_venues: Vec<String>,
    /// Strategy labels refused for NEW entries (`sniper`, `copy:<leader>`,
    /// the Polymarket strategy name).
    pub killed_strategies: Vec<String>,
    /// How often the server reconciles module positions / trades against
    /// the global ledger (seconds; 0 = only at startup).
    pub accounting_reconcile_interval_secs: u64,
}

impl Default for GlobalRiskConfig {
    fn default() -> Self {
        GlobalRiskConfig {
            reference_asset: "USD".into(),
            reference_rates: HashMap::from([("USDC".to_string(), 1.0)]),
            capital_base_ref: 0.0,
            max_portfolio_exposure_ref: 0.0,
            max_wallet_exposure_ref: 0.0,
            max_venue_exposure_ref: 0.0,
            max_strategy_exposure_ref: 0.0,
            max_asset_exposure_ref: 0.0,
            max_open_positions: 0,
            max_order_notional_ref: 0.0,
            max_daily_loss_ref: 0.0,
            max_drawdown_ref: 0.0,
            max_drawdown_pct: 0.0,
            killed_venues: Vec::new(),
            killed_strategies: Vec::new(),
            accounting_reconcile_interval_secs: 60,
        }
    }
}

impl GlobalRiskConfig {
    /// `killed_venues` parsed (unknown names are rejected by validation).
    pub fn killed_venue_list(&self) -> Vec<Venue> {
        self.killed_venues
            .iter()
            .filter_map(|v| Venue::parse(v.trim()))
            .collect()
    }

    /// True when any limit that needs a reference conversion is on.
    pub fn needs_reference_rates(&self) -> bool {
        self.max_portfolio_exposure_ref > 0.0
            || self.max_wallet_exposure_ref > 0.0
            || self.max_venue_exposure_ref > 0.0
            || self.max_strategy_exposure_ref > 0.0
            || self.max_asset_exposure_ref > 0.0
            || self.max_order_notional_ref > 0.0
            || self.max_daily_loss_ref > 0.0
            || self.effective_drawdown_limit().is_some()
    }

    /// The drawdown limit in reference units: the tighter of the absolute
    /// figure and `capital_base_ref × max_drawdown_pct`; `None` when off.
    pub fn effective_drawdown_limit(&self) -> Option<f64> {
        let abs = (self.max_drawdown_ref > 0.0).then_some(self.max_drawdown_ref);
        let pct = (self.max_drawdown_pct > 0.0 && self.capital_base_ref > 0.0)
            .then_some(self.capital_base_ref * self.max_drawdown_pct);
        match (abs, pct) {
            (Some(a), Some(p)) => Some(a.min(p)),
            (Some(a), None) => Some(a),
            (None, Some(p)) => Some(p),
            (None, None) => None,
        }
    }
}

/// Reconciliation & crash recovery (Prompt 2 §H): startup gate behaviour.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryConfig {
    /// Seconds to run the reconciliation worker over pending claims BEFORE
    /// trading modules start (0 = do not wait; the blocking rule below still
    /// applies to whatever remains unresolved).
    pub startup_reconcile_secs: u64,
    /// Claim batch size per worker pass during the startup window.
    pub startup_batch: i64,
    /// Disable the trading modules associated with actively unresolved
    /// (pending/in_progress) reconciliation claims until an operator
    /// re-enables them. A module must never trade on state the venue has
    /// not confirmed. Claims already parked for operators (`failed` after
    /// max attempts) do not re-block on every restart — they were alerted
    /// when parked.
    pub block_modules_on_unresolved: bool,
    /// How often open LIVE Solana positions are re-verified against their
    /// aggregated on-chain balance (a safety net behind per-execution claims,
    /// not the primary control). Clamped to a 30 s floor.
    pub position_recheck_interval_secs: u64,
    /// Journal a durable intent BEFORE every money-moving broadcast (§I
    /// crash point C). One extra local INSERT per execution buys evidence
    /// when the process dies between broadcast and any other durable trace;
    /// orphaned intents are reconciled (never resubmitted) and gate their
    /// symbol. Default on: safety over a sub-millisecond of latency.
    pub intent_journal: bool,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        RecoveryConfig {
            startup_reconcile_secs: 30,
            startup_batch: 64,
            block_modules_on_unresolved: true,
            position_recheck_interval_secs: 300,
            intent_journal: true,
        }
    }
}

/// Multi-replica / high-availability behaviour (Prompt 3).
///
/// Ownership authority: when `[database]` is enabled, Postgres is the
/// AUTHORITATIVE claim store (durable, transactional, auditable); Redis is
/// used only when Postgres is absent (short-lived coordination, per the
/// existing durability rule). With neither, claims are process-local
/// (single-instance semantics) and live multi-replica operation is a
/// misconfiguration — the startup log says so loudly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HaConfig {
    /// Explicit replica identity. Empty (default) = generated at startup as
    /// `{hostname}-{pid}-{random}`: unique among concurrent replicas,
    /// observable in logs/metrics/claims, free of secret material. A restart
    /// is a NEW replica; the previous life's claims expire by lease and are
    /// taken over (never reused silently).
    pub replica_id: String,
    /// Lease duration granted per execution claim. Renewed every lease/3
    /// while the owner works; after expiry another replica may take over
    /// (epoch increments, the stale owner is fenced). Clamped to a 5 s floor.
    pub claim_lease_secs: u64,
    /// How long a `handed_off` claim (execution ended AMBIGUOUS — e.g.
    /// SendUnknown — reconciliation owns the outcome) blocks re-acquisition
    /// of the same logical execution. Prevents a second replica resubmitting
    /// a transaction that may still be in flight. Clamped to a 60 s floor.
    pub claim_handoff_grace_secs: u64,
    /// Cross-replica runtime-flag sync interval (kill switch + module
    /// enabled flags propagate through Postgres/Redis). Clamped to 1 s.
    pub flag_sync_secs: u64,
    /// Cross-replica position-book refresh interval: open positions from the
    /// shared DB are merged into the local book so risk capacity (max open
    /// positions, per-symbol exposure) converges across replicas. Clamped
    /// to a 5 s floor.
    pub book_sync_secs: u64,
    /// TASK 6 — how this worker participates in the cluster:
    /// `single` (one worker owns everything), `active_passive` (standbys
    /// wait for the singleton leases to expire) or `active_active`
    /// (several workers process concurrently; per-execution claims keep
    /// them apart). Unknown values fall back to `single` with a warning.
    pub mode: String,
    /// TASK 6 — worker heartbeat period in seconds (clamped to a 1 s floor).
    pub heartbeat_secs: u64,
    /// TASK 6 — a worker is STALE after this long without a heartbeat.
    /// Leases of a stale worker are only taken over once they EXPIRE; this
    /// value only drives detection, metrics and audit. Clamped to a 5 s
    /// floor and to at least twice `heartbeat_secs`.
    pub heartbeat_timeout_secs: u64,
    /// TASK 6 — duration granted per singleton ROLE lease (reconciliation,
    /// recovery, accounting maintenance, state sync, each feed). Renewed
    /// every third of this. Clamped to a 5 s floor. Distinct from
    /// `claim_lease_secs`, which leases ONE execution intent.
    pub role_lease_secs: u64,
    /// TASK 6 — roles this worker must hold before it reports READY. Empty
    /// (default) = readiness does not depend on any lease, which is what a
    /// single worker with no standby wants.
    pub required_roles: Vec<String>,
}

impl Default for HaConfig {
    fn default() -> Self {
        HaConfig {
            replica_id: String::new(),
            claim_lease_secs: 45,
            claim_handoff_grace_secs: 900,
            flag_sync_secs: 5,
            book_sync_secs: 30,
            mode: "single".into(),
            heartbeat_secs: 10,
            heartbeat_timeout_secs: 45,
            role_lease_secs: 45,
            required_roles: Vec::new(),
        }
    }
}

impl HaConfig {
    /// The parsed HA mode (`single` when the string is unknown —
    /// validation warns).
    pub fn ha_mode(&self) -> crate::ha::HaMode {
        crate::ha::HaMode::parse(&self.mode).unwrap_or(crate::ha::HaMode::Single)
    }

    /// Effective heartbeat period (>= 1 s).
    pub fn heartbeat(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.heartbeat_secs.max(1))
    }

    /// Effective staleness timeout (>= 5 s and >= 2 × heartbeat).
    pub fn heartbeat_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(
            self.heartbeat_timeout_secs
                .max(5)
                .max(self.heartbeat_secs.saturating_mul(2)),
        )
    }

    /// Effective role-lease duration (>= 5 s).
    pub fn role_lease(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.role_lease_secs.max(5))
    }

    /// The parsed required roles (unknown names are dropped — validation
    /// rejects them before startup).
    pub fn required_role_list(&self) -> Vec<crate::ha::LeaseRole> {
        self.required_roles
            .iter()
            .filter_map(|r| crate::ha::LeaseRole::parse(r))
            .collect()
    }
}

/// PostgreSQL — durable relational state (orders, positions, audit, dedup).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    /// When false the whole DB layer stays off and every consumer keeps its
    /// in-memory/JSONL behaviour (default deployment has no hard DB dep).
    pub enabled: bool,
    /// Name of the env var holding the connection URL (the URL itself is a
    /// secret — it embeds credentials — so it is never stored in TOML).
    pub url_env: String,
    /// When true, startup FAILS if the database is configured but cannot be
    /// reached or migrated. When false the app degrades to memory/JSONL.
    pub required: bool,
    /// Run embedded migrations automatically on connect.
    pub auto_migrate: bool,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_ms: u64,
    /// Per-statement server-side timeout (Postgres `statement_timeout`).
    pub statement_timeout_ms: u64,
    /// Per-operation client-side timeout applied around every query.
    pub query_timeout_ms: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        DatabaseConfig {
            enabled: false,
            url_env: "POSTGRES_URL".into(),
            required: false,
            auto_migrate: true,
            max_connections: 8,
            min_connections: 0,
            acquire_timeout_ms: 5_000,
            statement_timeout_ms: 10_000,
            query_timeout_ms: 5_000,
        }
    }
}

/// Redis — short-lived coordination only (locks, dedup windows, rate-limit
/// counters, ephemeral caches). Durable financial state NEVER lives here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RedisConfig {
    pub enabled: bool,
    /// Name of the env var holding the Redis URL (`redis://…`/`rediss://…`).
    pub url_env: String,
    pub required: bool,
    pub connect_timeout_ms: u64,
    pub operation_timeout_ms: u64,
    /// Default TTL for dedup-window keys written through Redis.
    pub dedup_ttl_secs: u64,
}

impl Default for RedisConfig {
    fn default() -> Self {
        RedisConfig {
            enabled: false,
            url_env: "REDIS_URL".into(),
            required: false,
            connect_timeout_ms: 3_000,
            operation_timeout_ms: 1_000,
            dedup_ttl_secs: 24 * 3600,
        }
    }
}

/// One API principal: the key itself is read from `key_env` at startup and
/// only ever handled as a SHA-256 hash afterwards.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuthKeyConfig {
    pub label: String,
    /// Name of the env var holding the plaintext key.
    pub key_env: String,
    /// `owner` | `operator` | `readonly`.
    pub role: String,
}

impl Default for AuthKeyConfig {
    fn default() -> Self {
        AuthKeyConfig {
            label: "default".into(),
            key_env: String::new(),
            role: "readonly".into(),
        }
    }
}

/// Role-aware authorization for the control plane. The legacy
/// `[api].api_key_env` key keeps working and maps to the `owner` role, so
/// existing single-key deployments are unaffected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct AuthConfig {
    pub keys: Vec<AuthKeyConfig>,
}

/// Logical identity of the wallet the whole app trades with. It is always
/// registered from the existing `SOLANA_KEYPAIR` wallet-loading path and can
/// never be redefined through `[[signing.identities]]`.
pub const PRIMARY_SIGNER_IDENTITY: &str = "primary_trading";

/// Key-custody backend used for Solana transaction signing.
///
/// Only [`SigningProvider::Local`] is implemented in this build. Selecting
/// `vault`, `kms` or `hsm` parses, but startup fails with
/// `SignerError::UnsupportedBackend` instead of silently falling back to a
/// local keypair (see `solana_kit::signer::build_signer_registry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SigningProvider {
    /// Local keypair material (env/file), the existing wallet path.
    #[default]
    Local,
    /// HashiCorp Vault transit signing (not implemented in this build).
    Vault,
    /// Cloud KMS signing — AWS/GCP/Azure (not implemented in this build).
    Kms,
    /// Hardware security module (not implemented in this build).
    Hsm,
}

impl SigningProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            SigningProvider::Local => "local",
            SigningProvider::Vault => "vault",
            SigningProvider::Kms => "kms",
            SigningProvider::Hsm => "hsm",
        }
    }

    /// True when this build can actually provide signers for the backend.
    pub fn is_supported(&self) -> bool {
        matches!(self, SigningProvider::Local)
    }
}

/// One named signer identity in the registry.
///
/// Exactly one source must be set:
/// * `alias` — share an already-registered identity's signer (e.g. several
///   logical names mapping to the primary trading wallet);
/// * `keypair_env` — name of an environment variable holding a keypair spec
///   (same formats as `SOLANA_KEYPAIR`: path, base58, JSON array);
/// * `keypair_path` — filesystem path to a keypair file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SignerIdentityConfig {
    pub name: String,
    pub alias: Option<String>,
    pub keypair_env: Option<String>,
    pub keypair_path: Option<String>,
}

impl SignerIdentityConfig {
    /// How many of the three mutually exclusive sources are set.
    fn source_count(&self) -> usize {
        self.alias.is_some() as usize
            + self.keypair_env.is_some() as usize
            + self.keypair_path.is_some() as usize
    }
}

/// `[signing]` — key-custody provider plus named signer identities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SigningConfig {
    pub provider: SigningProvider,
    pub identities: Vec<SignerIdentityConfig>,
}

/// Secrets are *never* serialised to disk. They are filled from the environment.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretConfig {
    /// Solana keypair: a filesystem path, a base58 secret key, or a JSON array.
    pub solana_keypair: Option<String>,
    /// Polygon private key used to sign Polymarket orders (0x-prefixed or not).
    pub polygon_private_key: Option<String>,
    pub telegram_bot_token: Option<String>,
    pub api_key: Option<String>,
    /// Pre-derived Polymarket L2 credentials (skip the L1 derivation round-trip).
    pub poly_api_key: Option<String>,
    pub poly_api_secret: Option<String>,
    pub poly_api_passphrase: Option<String>,
}

/// Hand-written so that `{:?}` on `SecretConfig` (or anything embedding it,
/// like `Config`/`AppConfig`) can never emit secret material — only whether
/// each secret is present.
impl std::fmt::Debug for SecretConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn flag(v: &Option<String>) -> &'static str {
            match v {
                Some(s) if !s.is_empty() => "<set>",
                _ => "<unset>",
            }
        }
        f.debug_struct("SecretConfig")
            .field("solana_keypair", &flag(&self.solana_keypair))
            .field("polygon_private_key", &flag(&self.polygon_private_key))
            .field("telegram_bot_token", &flag(&self.telegram_bot_token))
            .field("api_key", &flag(&self.api_key))
            .field("poly_api_key", &flag(&self.poly_api_key))
            .field("poly_api_secret", &flag(&self.poly_api_secret))
            .field("poly_api_passphrase", &flag(&self.poly_api_passphrase))
            .finish()
    }
}

impl SecretConfig {
    /// Redacted view, safe to log or show on the dashboard.
    pub fn redacted(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(
            "solana_keypair".into(),
            mask(self.solana_keypair.as_deref()),
        );
        m.insert(
            "polygon_private_key".into(),
            mask(self.polygon_private_key.as_deref()),
        );
        m.insert(
            "telegram_bot_token".into(),
            mask(self.telegram_bot_token.as_deref()),
        );
        m.insert("api_key".into(), mask(self.api_key.as_deref()));
        m.insert("poly_api_key".into(), mask(self.poly_api_key.as_deref()));
        m.insert(
            "poly_api_secret".into(),
            mask(self.poly_api_secret.as_deref()),
        );
        m.insert(
            "poly_api_passphrase".into(),
            mask(self.poly_api_passphrase.as_deref()),
        );
        m
    }
}

fn mask(v: Option<&str>) -> String {
    match v {
        None | Some("") => "—".into(),
        Some(s) => {
            if s.len() <= 8 {
                "****".into()
            } else {
                format!("{}…{} ({} chars)", &s[..4], &s[s.len() - 4..], s.len())
            }
        }
    }
}

/// `Config` plus the resolved keypair/pubkey material the rest of the app needs.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub raw: Config,
    /// Resolved path of the file the config was loaded from, if any.
    pub source_path: Option<PathBuf>,
    /// Warnings collected while loading (unknown env vars, missing keys…).
    pub warnings: Vec<String>,
}

impl AppConfig {
    pub fn load() -> BotResult<Self> {
        let path = std::env::var("CONFIG_PATH")
            .ok()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("config.toml"));
        Self::load_from(path)
    }

    pub fn load_from<P: AsRef<Path>>(path: P) -> BotResult<Self> {
        dotenvy::dotenv().ok();

        let path = path.as_ref();
        let mut config = if path.exists() {
            let text = std::fs::read_to_string(path)
                .map_err(|e| BotError::config(format!("cannot read {}: {e}", path.display())))?;
            toml::from_str::<Config>(&text)
                .map_err(|e| BotError::config(format!("cannot parse {}: {e}", path.display())))?
        } else {
            Config::default()
        };

        let mut warnings = Vec::new();
        if !path.exists() {
            warnings.push(format!(
                "config file {} not found — using built-in defaults",
                path.display()
            ));
        }

        apply_env_overrides(&mut config, &mut warnings);
        validate(&config, &mut warnings)?;

        Ok(AppConfig {
            raw: config,
            source_path: if path.exists() {
                Some(path.to_path_buf())
            } else {
                None
            },
            warnings,
        })
    }

    pub fn from_defaults() -> Self {
        AppConfig {
            raw: Config::default(),
            source_path: None,
            warnings: Vec::new(),
        }
    }

    pub fn cluster(&self) -> &str {
        &self.raw.network.cluster
    }

    pub fn live_allowed(&self) -> bool {
        self.raw.execution.live_allowed()
    }

    pub fn mode(&self) -> ExecutionMode {
        self.raw.execution.mode
    }

    pub fn data_dir(&self) -> PathBuf {
        PathBuf::from(&self.raw.storage.data_dir)
    }

    /// A one-line banner describing exactly how dangerous this run is.
    pub fn safety_banner(&self) -> String {
        let mode = self.raw.execution.mode.as_str();
        let live = self.raw.execution.allow_live_trading;
        let kill = self.raw.risk.kill_switch;
        format!(
            "mode={mode} allow_live_trading={live} kill_switch={kill} cluster={} jito={}",
            self.raw.network.cluster, self.raw.execution.use_jito
        )
    }

    pub fn enabled_modules(&self) -> Vec<BotModule> {
        let mut v = Vec::new();
        if self.raw.sniper.enabled {
            v.push(BotModule::Sniper);
        }
        if self.raw.copy.enabled {
            v.push(BotModule::Copy);
        }
        if self.raw.polymarket.enabled {
            v.push(BotModule::Polymarket);
        }
        if self.raw.contract.program_id.is_some() {
            v.push(BotModule::Contract);
        }
        if self.raw.telegram.enabled {
            v.push(BotModule::Telegram);
        }
        v
    }
}

/// Pull a value out of the environment, pushing a warning if it is malformed.
fn env_str(key: &str, warnings: &mut Vec<String>) -> Option<String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        Ok(_) => None,
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => {
            warnings.push(format!("env {key}: {e}"));
            None
        }
    }
}

fn env_bool(key: &str, warnings: &mut Vec<String>) -> Option<bool> {
    env_str(key, warnings)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

fn env_f64(key: &str, warnings: &mut Vec<String>) -> Option<f64> {
    env_str(key, warnings).and_then(|v| match v.parse::<f64>() {
        Ok(n) => Some(n),
        Err(e) => {
            warnings.push(format!("env {key} is not a number ({e})"));
            None
        }
    })
}

fn env_u64(key: &str, warnings: &mut Vec<String>) -> Option<u64> {
    env_str(key, warnings).and_then(|v| match v.parse::<u64>() {
        Ok(n) => Some(n),
        Err(e) => {
            warnings.push(format!("env {key} is not an integer ({e})"));
            None
        }
    })
}

fn env_i64(key: &str, warnings: &mut Vec<String>) -> Option<i64> {
    env_str(key, warnings).and_then(|v| match v.parse::<i64>() {
        Ok(n) => Some(n),
        Err(e) => {
            warnings.push(format!("env {key} is not an integer ({e})"));
            None
        }
    })
}

fn env_list(key: &str, warnings: &mut Vec<String>) -> Option<Vec<String>> {
    env_str(key, warnings).map(|v| {
        v.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

fn apply_env_overrides(c: &mut Config, w: &mut Vec<String>) {
    // --- network ---
    if let Some(v) = env_str("SOLANA_CLUSTER", w) {
        c.network.cluster = v.clone();
        // Keep the defaults coherent when only the cluster name is given.
        if v.contains("devnet") && c.network.rpc_url.contains("mainnet") {
            c.network.rpc_url = "https://api.devnet.solana.com".into();
            c.network.ws_url = "wss://api.devnet.solana.com".into();
        }
    }
    if let Some(v) = env_str("SOLANA_RPC_URL", w) {
        c.network.rpc_url = v;
    }
    if let Some(v) = env_str("SOLANA_WS_URL", w) {
        c.network.ws_url = v;
    }
    if let Some(v) = env_str("GEYSER_WS_URL", w) {
        c.network.geyser_ws_url = Some(v);
    }
    if let Some(v) = env_str("COMMITMENT", w) {
        c.network.commitment = v;
    }
    if let Some(v) = env_u64("ACCOUNT_CACHE_TTL_MS", w) {
        c.network.account_cache_ttl_ms = v;
    }
    if let Some(v) = env_u64("ACCOUNT_CACHE_MAX_ENTRIES", w) {
        c.network.account_cache_max_entries = v as usize;
    }
    if let Some(v) = env_u64("RPC_RETRY_BASE_BACKOFF_MS", w) {
        c.network.retry_base_backoff_ms = v;
    }
    if let Some(v) = env_u64("RPC_RETRY_MAX_BACKOFF_MS", w) {
        c.network.retry_max_backoff_ms = v;
    }
    if let Some(v) = env_bool("RPC_RETRY_JITTER", w) {
        c.network.retry_jitter = v;
    }
    if let Some(v) = env_u64("RPC_RATE_LIMIT_COOLDOWN_MS", w) {
        c.network.rate_limit_cooldown_ms = v;
    }
    if let Some(v) = env_u64("RPC_PROVIDER_FAILURE_THRESHOLD", w) {
        c.network.provider_failure_threshold = v.min(u32::MAX as u64) as u32;
    }
    if let Some(v) = env_u64("RPC_PROVIDER_COOLDOWN_MS", w) {
        c.network.provider_cooldown_ms = v;
    }
    if let Some(v) = env_u64("WS_STALE_AFTER_MS", w) {
        c.network.ws_stale_after_ms = v;
    }

    // --- execution ---
    if let Some(v) = env_str("EXECUTION_MODE", w) {
        match v.parse::<ExecutionMode>() {
            Ok(m) => c.execution.mode = m,
            Err(e) => w.push(format!("EXECUTION_MODE ignored: {e}")),
        }
    }
    if let Some(v) = env_bool("ALLOW_LIVE_TRADING", w) {
        c.execution.allow_live_trading = v;
    }
    if let Some(v) = env_bool("USE_JITO", w) {
        c.execution.use_jito = v;
    }
    if let Some(v) = env_str("JITO_BLOCK_ENGINE_URL", w) {
        c.execution.jito_block_engine_url = v;
    }
    if let Some(v) = env_u64("JITO_TIP_LAMPORTS", w) {
        c.execution.jito_tip_lamports = v;
    }
    if let Some(v) = env_u64("PRIORITY_FEE_MICRO_LAMPORTS", w) {
        c.execution.priority_fee_micro_lamports = v;
    }
    if let Some(v) = env_str("COMPUTE_UNIT_LIMIT", w) {
        if let Ok(n) = v.parse::<u32>() {
            c.execution.compute_unit_limit = n;
        } else {
            w.push("COMPUTE_UNIT_LIMIT is not a u32".into());
        }
    }
    if let Some(v) = env_bool("SIMULATE_FIRST", w) {
        c.execution.simulate_first = v;
    }
    if let Some(v) = env_bool("ABORT_ON_SIMULATION_FAILURE", w) {
        c.execution.abort_on_simulation_failure = v;
    }
    if let Some(v) = env_bool("BROADCAST_FANOUT", w) {
        c.execution.broadcast_fanout = v;
    }
    if let Some(v) = env_str("FEE_MODE", w) {
        c.execution.fee_mode = v;
    }
    if let Some(v) = env_u64("FEE_MIN_MICRO_LAMPORTS", w) {
        c.execution.fee_min_micro_lamports = v;
    }
    if let Some(v) = env_u64("FEE_MAX_MICRO_LAMPORTS", w) {
        c.execution.fee_max_micro_lamports = v;
    }
    if let Some(v) = env_u64("FEE_EMERGENCY_MAX_MICRO_LAMPORTS", w) {
        c.execution.fee_emergency_max_micro_lamports = v;
    }
    if let Some(v) = env_u64("FEE_PERCENTILE", w) {
        c.execution.fee_percentile = v.min(100) as u8;
    }
    if let Some(v) = env_u64("FEE_ESCALATION_PCT", w) {
        c.execution.fee_escalation_pct = v;
    }
    if let Some(v) = env_u64("MAX_BLOCKHASH_AGE_MS", w) {
        c.execution.max_blockhash_age_ms = v;
    }

    // --- persistence / dedup ---
    if let Some(v) = env_bool("DATABASE_ENABLED", w) {
        c.database.enabled = v;
    }
    if let Some(v) = env_bool("DATABASE_REQUIRED", w) {
        c.database.required = v;
    }
    if let Some(v) = env_bool("DATABASE_AUTO_MIGRATE", w) {
        c.database.auto_migrate = v;
    }
    if let Some(v) = env_u64("DATABASE_MAX_CONNECTIONS", w) {
        c.database.max_connections = v.max(1) as u32;
    }
    if let Some(v) = env_bool("REDIS_ENABLED", w) {
        c.redis.enabled = v;
    }
    if let Some(v) = env_bool("REDIS_REQUIRED", w) {
        c.redis.required = v;
    }
    if let Some(v) = env_str("DEDUP_BACKEND", w) {
        c.storage.dedup_backend = v;
    }
    if let Some(v) = env_u64("DEDUP_TTL_SECS", w) {
        c.storage.dedup_ttl_secs = v;
    }
    if let Some(v) = env_u64("RATE_LIMIT_RPM", w) {
        c.api.rate_limit_rpm = v.min(u32::MAX as u64) as u32;
    }

    // --- risk ---
    if let Some(v) = env_bool("KILL_SWITCH", w) {
        c.risk.kill_switch = v;
    }
    if let Some(v) = env_f64("MAX_POSITION_QUOTE", w) {
        c.risk.max_position_quote = v;
    }
    if let Some(v) = env_f64("MAX_POSITION_FRACTION", w) {
        c.risk.max_position_fraction = v;
    }
    if let Some(v) = env_f64("DAILY_LOSS_LIMIT_QUOTE", w) {
        c.risk.daily_loss_limit_quote = v;
    }
    if let Some(v) = env_f64("DEFAULT_STOP_LOSS_PCT", w) {
        c.risk.default_stop_loss_pct = v;
    }
    if let Some(v) = env_f64("DEFAULT_TAKE_PROFIT_PCT", w) {
        c.risk.default_take_profit_pct = v;
    }
    if let Some(v) = env_f64("MIN_SOL_RESERVE", w) {
        c.risk.min_sol_reserve = v;
    }
    if let Some(v) = env_u64("MAX_SLIPPAGE_BPS", w) {
        c.risk.max_slippage_bps = v;
    }
    if let Some(v) = env_i64("MAX_HOLD_SECS", w) {
        c.risk.max_hold_secs = if v <= 0 { None } else { Some(v) };
    }
    if let Some(v) = env_f64("TRAILING_STOP_PCT", w) {
        c.risk.trailing_stop_pct = if v <= 0.0 { None } else { Some(v) };
    }
    if let Some(v) = env_i64("REENTRY_COOLDOWN_SECS", w) {
        c.risk.reentry_cooldown_secs = v;
    }
    if let Some(v) = env_i64("COPY_COOLDOWN_SECS", w) {
        c.risk.copy_cooldown_secs = v;
    }
    if let Some(v) = env_u64("MAX_OPEN_POSITIONS", w) {
        c.risk.max_open_positions = v as usize;
    }

    // --- module 1 ---
    if let Some(v) = env_bool("SNIPER_ENABLED", w) {
        c.sniper.enabled = v;
    }
    if let Some(v) = env_f64("SNIPER_BUY_SOL", w) {
        c.sniper.buy_sol = v;
    }
    if let Some(v) = env_f64("SNIPER_SLIPPAGE_PCT", w) {
        c.sniper.slippage_pct = v;
    }
    if let Some(v) = env_str("PUMPPORTAL_WS_URL", w) {
        c.sniper.pumpportal_ws_url = v;
    }
    if let Some(v) = env_str("PUMPPORTAL_API_KEY", w) {
        c.sniper.pumpportal_api_key = Some(v);
    }
    if let Some(v) = env_list("SNIPER_CREATOR_DENYLIST", w) {
        c.sniper.creator_denylist = v;
    }
    if let Some(v) = env_list("SNIPER_KEYWORD_DENYLIST", w) {
        c.sniper.keyword_denylist = v;
    }
    if let Some(v) = env_list("PUMP_EXTRA_ACCOUNTS", w) {
        c.sniper.pump_extra_accounts = v;
    }
    if let Some(v) = env_bool("PUMP_APPEND_BONDING_CURVE_V2", w) {
        c.sniper.pump_append_bonding_curve_v2 = v;
    }
    if let Some(v) = env_i64("SNIPER_MAX_HOLD_SECS", w) {
        c.sniper.max_hold_secs = if v <= 0 { None } else { Some(v) };
    }
    if let Some(v) = env_i64("SNIPER_MAX_LAUNCH_AGE_SECS", w) {
        c.sniper.max_launch_age_secs = v;
    }
    if let Some(v) = env_u64("SNIPER_MAX_ENTRY_LATENCY_MS", w) {
        c.sniper.max_entry_latency_ms = v;
    }
    if let Some(v) = env_f64("SNIPER_TAKE_PROFIT_PCT", w) {
        c.sniper.take_profit_pct = if v <= 0.0 { None } else { Some(v) };
    }
    if let Some(v) = env_f64("SNIPER_STOP_LOSS_PCT", w) {
        c.sniper.stop_loss_pct = if v <= 0.0 { None } else { Some(v) };
    }
    if let Some(v) = env_bool("SNIPER_TRADE_PUMPSWAP", w) {
        c.sniper.trade_pumpswap = v;
    }
    if let Some(v) = env_bool("SNIPER_TRADE_RAYDIUM", w) {
        c.sniper.trade_raydium = v;
    }
    if let Some(v) = env_str("SNIPER_SLIPPAGE_MODE", w) {
        c.sniper.slippage_mode = v;
    }
    if let Some(v) = env_u64("SNIPER_MAX_PRICE_IMPACT_BPS", w) {
        c.sniper.max_price_impact_bps = v;
    }
    if let Some(v) = env_u64("SNIPER_MAX_ENTRY_FEE_LAMPORTS", w) {
        c.sniper.max_entry_fee_lamports = v;
    }
    if let Some(v) = env_f64("SNIPER_MIN_LIQUIDITY_SOL", w) {
        c.sniper.min_liquidity_sol = v;
    }
    if let Some(v) = env_bool("SNIPER_REQUIRE_MINT_AUTHORITY_REVOKED", w) {
        c.sniper.require_mint_authority_revoked = v;
    }
    if let Some(v) = env_bool("SNIPER_REQUIRE_FREEZE_AUTHORITY_REVOKED", w) {
        c.sniper.require_freeze_authority_revoked = v;
    }
    if let Some(v) = env_bool("SNIPER_STRICT_GATES", w) {
        c.sniper.strict_gates = v;
    }
    if let Some(v) = env_i64("SNIPER_STALE_POSITION_EXIT_SECS", w) {
        c.sniper.stale_position_exit_secs = v;
    }
    // Sniper exposure controls live in [risk] but are Module-1 specific.
    if let Some(v) = env_bool("SNIPER_EMERGENCY_DISABLE", w) {
        c.risk.sniper_emergency_disable = v;
    }
    if let Some(v) = env_f64("SNIPER_MAX_POSITION_SOL", w) {
        c.risk.sniper_max_position_quote = v;
    }
    if let Some(v) = env_f64("SNIPER_MAX_TOTAL_EXPOSURE_SOL", w) {
        c.risk.sniper_max_total_exposure_quote = v;
    }
    if let Some(v) = env_u64("SNIPER_MAX_CONCURRENT_POSITIONS", w) {
        c.risk.sniper_max_concurrent_positions = v as usize;
    }
    if let Some(v) = env_u64("SNIPER_MAX_PENDING_EXECUTIONS", w) {
        c.risk.sniper_max_pending_executions = v as usize;
    }
    if let Some(v) = env_i64("SNIPER_TOKEN_COOLDOWN_SECS", w) {
        c.risk.sniper_token_cooldown_secs = v;
    }
    if let Some(v) = env_i64("SNIPER_FAILED_ENTRY_COOLDOWN_SECS", w) {
        c.risk.sniper_failed_entry_cooldown_secs = v;
    }
    if let Some(v) = env_f64("SNIPER_DAILY_LOSS_LIMIT_SOL", w) {
        c.risk.sniper_daily_loss_limit_quote = v;
    }

    // --- module 2 ---
    if let Some(v) = env_bool("COPY_ENABLED", w) {
        c.copy.enabled = v;
    }
    if let Some(v) = env_list("COPY_WALLETS", w) {
        // Preserve existing per-wallet tuning if the address already appears.
        let mut wallets: Vec<CopyWallet> = v
            .into_iter()
            .map(|addr| {
                c.copy
                    .wallets
                    .iter()
                    .find(|w| w.address == addr)
                    .cloned()
                    .unwrap_or(CopyWallet {
                        address: addr,
                        ..Default::default()
                    })
            })
            .collect();
        wallets.sort_by(|a, b| a.address.cmp(&b.address));
        wallets.dedup_by(|a, b| a.address == b.address);
        c.copy.wallets = wallets;
    }
    if let Some(v) = env_f64("COPY_FRACTION", w) {
        for wallet in &mut c.copy.wallets {
            wallet.fraction_of_their_size = v;
        }
    }
    if let Some(v) = env_f64("COPY_MAX_SOL", w) {
        for wallet in &mut c.copy.wallets {
            wallet.max_sol = v;
        }
    }
    if let Some(v) = env_str("COPY_FEED", w) {
        c.copy.feed = v;
    }
    if let Some(v) = env_i64("COPY_MAX_EVENT_AGE_SECS", w) {
        c.copy.max_event_age_secs = v;
    }
    if let Some(v) = env_bool("COPY_STRICT_ORDERING", w) {
        c.copy.strict_ordering = v;
    }
    if let Some(v) = env_f64("COPY_MAX_SOL_PER_TRADE", w) {
        c.copy.max_sol_per_trade = v;
    }
    if let Some(v) = env_f64("COPY_MAX_BALANCE_FRACTION", w) {
        c.copy.max_balance_fraction = v;
    }
    if let Some(v) = env_f64("COPY_MIN_MIRROR_SOL", w) {
        c.copy.min_mirror_sol = v;
    }
    if let Some(v) = env_u64("COPY_RECONCILE_INTERVAL_SECS", w) {
        c.copy.reconcile_interval_secs = v;
    }
    if let Some(v) = env_bool("COPY_RECONCILE_AUTO_EXIT", w) {
        c.copy.reconcile_auto_exit = v;
    }
    if let Some(v) = env_i64("COPY_RECOVERY_LOOKBACK_HOURS", w) {
        c.copy.recovery_lookback_hours = v;
    }
    // Copy exposure controls live in [risk] but are Module-2 specific.
    if let Some(v) = env_bool("COPY_EMERGENCY_DISABLE", w) {
        c.risk.copy_emergency_disable = v;
    }
    if let Some(v) = env_f64("COPY_MAX_POSITION_SOL", w) {
        c.risk.copy_max_position_quote = v;
    }
    if let Some(v) = env_f64("COPY_MAX_TOTAL_EXPOSURE_SOL", w) {
        c.risk.copy_max_total_exposure_quote = v;
    }
    if let Some(v) = env_u64("COPY_MAX_CONCURRENT_POSITIONS", w) {
        c.risk.copy_max_concurrent_positions = v as usize;
    }
    if let Some(v) = env_u64("COPY_MAX_PENDING_EXECUTIONS", w) {
        c.risk.copy_max_pending_executions = v as usize;
    }
    if let Some(v) = env_i64("COPY_FAILED_ENTRY_COOLDOWN_SECS", w) {
        c.risk.copy_failed_entry_cooldown_secs = v;
    }
    if let Some(v) = env_f64("COPY_DAILY_LOSS_LIMIT_SOL", w) {
        c.risk.copy_daily_loss_limit_quote = v;
    }
    if let Some(v) = env_f64("COPY_MAX_LEADER_EXPOSURE_SOL", w) {
        c.risk.copy_max_leader_exposure_quote = v;
    }

    // --- module 3 ---
    if let Some(v) = env_bool("POLYMARKET_ENABLED", w) {
        c.polymarket.enabled = v;
    }
    if let Some(v) = env_f64("POLYMARKET_STAKE_USD", w) {
        c.polymarket.stake_usd = v;
    }
    if let Some(v) = env_str("POLYMARKET_STRATEGY", w) {
        c.polymarket.strategy = v;
    }
    if let Some(v) = env_str("CLOB_URL", w) {
        c.polymarket.clob_url = v;
    }
    if let Some(v) = env_str("GAMMA_URL", w) {
        c.polymarket.gamma_url = v;
    }
    if let Some(v) = env_str("EXCHANGE_ADDRESS", w) {
        c.polymarket.exchange_address = v;
    }
    if let Some(v) = env_str("NEG_RISK_EXCHANGE_ADDRESS", w) {
        c.polymarket.neg_risk_exchange_address = v;
    }
    if let Some(v) = env_str("POLYMARKET_DOMAIN_VERSION", w) {
        c.polymarket.exchange_domain_version = v;
    }
    if let Some(v) = env_str("POLYMARKET_FUNDER", w) {
        c.polymarket.funder_address = Some(v);
    }
    if let Some(v) = env_str("POLYMARKET_SIGNATURE_TYPE", w) {
        match v.parse::<u8>() {
            Ok(n @ 0..=3) => c.polymarket.signature_type = n,
            Ok(n) => w.push(format!("POLYMARKET_SIGNATURE_TYPE={n} out of range 0..=3")),
            Err(e) => w.push(format!("POLYMARKET_SIGNATURE_TYPE: {e}")),
        }
    }
    if let Some(v) = env_f64("POLYMARKET_MAX_SPREAD", w) {
        c.polymarket.max_spread = v;
    }
    if let Some(v) = env_f64("POLYMARKET_MIN_LIQUIDITY_USD", w) {
        c.polymarket.min_liquidity_usd = v;
    }
    if let Some(v) = env_i64("POLYMARKET_QUOTE_MAX_AGE_SECS", w) {
        c.polymarket.quote_max_age_secs = v;
    }
    if let Some(v) = env_i64("POLYMARKET_MIN_TIME_TO_RESOLUTION_SECS", w) {
        c.polymarket.min_time_to_resolution_secs = v;
    }
    if let Some(v) = env_f64("POLYMARKET_MIN_ORDER_SIZE", w) {
        c.polymarket.min_order_size = v;
    }
    if let Some(v) = env_u64("POLYMARKET_ORDER_POLL_INTERVAL_SECS", w) {
        c.polymarket.order_poll_interval_secs = v;
    }
    if let Some(v) = env_i64("POLYMARKET_ORDER_TTL_SECS", w) {
        c.polymarket.order_ttl_secs = v;
    }
    if let Some(v) = env_f64("POLYMARKET_REPRICE_THRESHOLD", w) {
        c.polymarket.reprice_threshold = v;
    }
    if let Some(v) = env_u64("POLYMARKET_RECONCILE_INTERVAL_SECS", w) {
        c.polymarket.reconcile_interval_secs = v;
    }
    if let Some(v) = env_bool("POLYMARKET_RECONCILE_CANCEL_ORPHANS", w) {
        c.polymarket.reconcile_cancel_orphans = v;
    }
    if let Some(v) = env_bool("POLYMARKET_USE_USER_WEBSOCKET", w) {
        c.polymarket.use_user_websocket = v;
    }
    if let Some(v) = env_bool("POLYMARKET_CANCEL_ON_SHUTDOWN", w) {
        c.polymarket.cancel_on_shutdown = v;
    }
    if let Some(v) = env_bool("POLY_EMERGENCY_DISABLE", w) {
        c.risk.poly_emergency_disable = v;
    }
    if let Some(v) = env_f64("POLY_MAX_POSITION_USD", w) {
        c.risk.poly_max_position_quote = v;
    }
    if let Some(v) = env_f64("POLY_MAX_TOTAL_EXPOSURE_USD", w) {
        c.risk.poly_max_total_exposure_quote = v;
    }
    if let Some(v) = env_f64("POLY_MAX_MARKET_EXPOSURE_USD", w) {
        c.risk.poly_max_market_exposure_quote = v;
    }
    if let Some(v) = env_u64("POLY_MAX_CONCURRENT_POSITIONS", w) {
        c.risk.poly_max_concurrent_positions = v as usize;
    }
    if let Some(v) = env_u64("POLY_MAX_OPEN_ORDERS", w) {
        c.risk.poly_max_open_orders = v as usize;
    }
    if let Some(v) = env_f64("POLY_DAILY_LOSS_LIMIT_USD", w) {
        c.risk.poly_daily_loss_limit_quote = v;
    }

    // --- module 4 ---
    if let Some(v) = env_str("STAKING_PROGRAM_ID", w) {
        c.contract.program_id = Some(v);
    }
    if let Some(v) = env_str("STAKING_TOKEN_MINT", w) {
        c.contract.token_mint = Some(v);
    }
    if let Some(v) = env_str("FEE_TREASURY", w) {
        c.contract.fee_treasury = Some(v);
    }

    // --- module 5 ---
    if let Some(v) = env_bool("TELEGRAM_ENABLED", w) {
        c.telegram.enabled = v;
    }
    if let Some(v) = env_list("TELEGRAM_ALLOWED_CHAT_IDS", w) {
        c.telegram.allowed_chat_ids = v
            .iter()
            .filter_map(|s| match s.parse::<i64>() {
                Ok(n) => Some(n),
                Err(_) => {
                    w.push(format!(
                        "TELEGRAM_ALLOWED_CHAT_IDS: '{s}' is not an integer"
                    ));
                    None
                }
            })
            .collect();
    }
    if let Some(v) = env_str("TELEGRAM_ALERT_CHAT_ID", w) {
        match v.parse::<i64>() {
            Ok(n) => c.telegram.alert_chat_id = Some(n),
            Err(e) => w.push(format!("TELEGRAM_ALERT_CHAT_ID: {e}")),
        }
    }

    // --- api / storage ---
    if let Some(v) = env_str("API_BIND_HOST", w) {
        c.api.bind_host = v;
    }
    if let Some(v) = env_str("API_BIND_PORT", w) {
        match v.parse::<u16>() {
            Ok(n) => c.api.bind_port = n,
            Err(e) => w.push(format!("API_BIND_PORT: {e}")),
        }
    }
    if let Some(v) = env_str("DATA_DIR", w) {
        c.storage.data_dir = v;
    }
    if let Some(v) = env_str("LOG_LEVEL", w) {
        c.observability.log_level = v;
    }
    if let Some(v) = env_str("LOG_FORMAT", w) {
        c.observability.log_format = v;
    }
    if let Some(v) = env_str("METRICS_ENABLED", w) {
        match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => c.observability.metrics_enabled = true,
            "0" | "false" | "no" | "off" => c.observability.metrics_enabled = false,
            _ => w.push(format!(
                "METRICS_ENABLED: invalid boolean {v:?} (expected true/false), ignored"
            )),
        }
    }
    if let Some(v) = env_str("SAMPLE_INTERVAL_MS", w) {
        match v.parse::<u64>() {
            Ok(n) => c.observability.sample_interval_ms = n,
            Err(e) => w.push(format!("SAMPLE_INTERVAL_MS: {e}")),
        }
    }

    // --- recovery / reconciliation startup gate ---
    if let Some(v) = env_u64("RECOVERY_STARTUP_RECONCILE_SECS", w) {
        c.recovery.startup_reconcile_secs = v;
    }
    if let Some(v) = env_i64("RECOVERY_STARTUP_BATCH", w) {
        c.recovery.startup_batch = v.max(1);
    }
    if let Some(v) = env_bool("RECOVERY_BLOCK_MODULES_ON_UNRESOLVED", w) {
        c.recovery.block_modules_on_unresolved = v;
    }
    if let Some(v) = env_bool("RECOVERY_INTENT_JOURNAL", w) {
        c.recovery.intent_journal = v;
    }
    if let Some(v) = env_str("HA_REPLICA_ID", w) {
        c.ha.replica_id = v;
    }
    if let Some(v) = env_u64("HA_CLAIM_LEASE_SECS", w) {
        c.ha.claim_lease_secs = v.max(5);
    }
    if let Some(v) = env_u64("HA_CLAIM_HANDOFF_GRACE_SECS", w) {
        c.ha.claim_handoff_grace_secs = v.max(60);
    }
    if let Some(v) = env_u64("HA_FLAG_SYNC_SECS", w) {
        c.ha.flag_sync_secs = v.max(1);
    }
    if let Some(v) = env_u64("HA_BOOK_SYNC_SECS", w) {
        c.ha.book_sync_secs = v.max(5);
    }
    if let Some(v) = env_u64("RECOVERY_POSITION_RECHECK_SECS", w) {
        c.recovery.position_recheck_interval_secs = v.max(30);
    }

    // --- signing (key custody) ---
    if let Some(v) = env_str("SIGNING_PROVIDER", w) {
        match v.to_ascii_lowercase().as_str() {
            "local" => c.signing.provider = SigningProvider::Local,
            "vault" => c.signing.provider = SigningProvider::Vault,
            "kms" => c.signing.provider = SigningProvider::Kms,
            "hsm" => c.signing.provider = SigningProvider::Hsm,
            other => w.push(format!(
                "SIGNING_PROVIDER: unknown provider '{other}' (expected local/vault/kms/hsm) — \
                 keeping '{}'",
                c.signing.provider.as_str()
            )),
        }
    }

    // --- secrets (env only) ---
    c.secrets.solana_keypair = env_str("SOLANA_KEYPAIR", w)
        .or_else(|| env_str("KEYPAIR_PATH", w))
        .or_else(|| env_str("WALLET_PATH", w));
    c.secrets.polygon_private_key =
        env_str("POLYGON_PRIVATE_KEY", w).or_else(|| env_str("POLYMARKET_PRIVATE_KEY", w));
    c.secrets.telegram_bot_token =
        env_str(&c.telegram.bot_token_env.clone(), w).or_else(|| env_str("TELOXIDE_TOKEN", w));
    c.secrets.api_key = env_str(&c.api.api_key_env.clone(), w);
    c.secrets.poly_api_key = env_str("POLY_API_KEY", w);
    c.secrets.poly_api_secret = env_str("POLY_API_SECRET", w);
    c.secrets.poly_api_passphrase = env_str("POLY_API_PASSPHRASE", w);
}

/// Resolve the legacy control-plane API key: `secrets.api_key` first (it is
/// filled from the env var named by `[api].api_key_env` during load), then
/// the env var directly. `None` when no key is configured (open dev mode).
pub fn resolve_api_key(c: &Config) -> Option<String> {
    if let Some(k) = &c.secrets.api_key {
        if !k.trim().is_empty() {
            return Some(k.clone());
        }
    }
    std::env::var(&c.api.api_key_env)
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// Sniper-engine specific validation (slippage engine, safety gates, exit
/// hardening, exposure controls). Split out so the rules stay readable.
fn validate_sniper_engine(c: &Config, w: &mut Vec<String>) -> BotResult<()> {
    let sn = &c.sniper;
    let mode = sn.slippage_mode_normalized();
    if !matches!(mode.as_str(), "fixed" | "liquidity_aware" | "price_impact") {
        return Err(BotError::config(format!(
            "sniper.slippage_mode = \"{}\" — expected \"fixed\", \"liquidity_aware\" or \"price_impact\"",
            sn.slippage_mode
        )));
    }
    for (label, v) in [
        ("sniper.pumpswap_slippage_pct", sn.pumpswap_slippage_pct),
        ("sniper.raydium_slippage_pct", sn.raydium_slippage_pct),
    ] {
        if let Some(v) = v {
            if !v.is_finite() || !(0.0..=100.0).contains(&v) {
                return Err(BotError::config(format!("{label} must be in [0, 100]")));
            }
        }
    }
    for (mint, bps) in &sn.slippage_overrides_bps {
        if mint.trim().is_empty() {
            return Err(BotError::config(
                "sniper.slippage_overrides_bps contains an empty mint key",
            ));
        }
        if *bps > 10_000 {
            return Err(BotError::config(format!(
                "sniper.slippage_overrides_bps[{mint}] = {bps} must be <= 10000"
            )));
        }
        if *bps > c.risk.max_slippage_bps {
            w.push(format!(
                "sniper.slippage_overrides_bps[{mint}] = {bps} exceeds risk.max_slippage_bps = {} — it will be clamped",
                c.risk.max_slippage_bps
            ));
        }
    }
    if sn.max_price_impact_bps > 10_000 {
        return Err(BotError::config(
            "sniper.max_price_impact_bps must be <= 10000",
        ));
    }
    if sn.max_entry_fee_lamports > 0 {
        if sn.max_entry_fee_lamports < LAMPORTS_PER_SIGNATURE {
            return Err(BotError::config(format!(
                "sniper.max_entry_fee_lamports must be 0 (off) or >= {LAMPORTS_PER_SIGNATURE} \
                 (the base fee of one signature)"
            )));
        }
        // The configured first-attempt fee alone must fit, or every entry is
        // FEE_LIMIT before it starts (the pipeline also prices retries and
        // the adaptive ceiling; this is the floor of that estimate).
        let ex = &c.execution;
        let priority = (ex.priority_fee_micro_lamports as u128 * ex.compute_unit_limit as u128)
            .div_ceil(1_000_000);
        let tip = if ex.use_jito { ex.jito_tip_lamports } else { 0 };
        let floor = (LAMPORTS_PER_SIGNATURE as u128)
            .saturating_add(priority)
            .saturating_add(tip as u128);
        if floor > sn.max_entry_fee_lamports as u128 {
            w.push(format!(
                "sniper.max_entry_fee_lamports = {} is below the fee of the configured first \
                 attempt ({floor} lamports = {LAMPORTS_PER_SIGNATURE} base + \
                 priority_fee_micro_lamports × compute_unit_limit{}) — every sniper entry will \
                 be rejected as FEE_LIMIT",
                sn.max_entry_fee_lamports,
                if ex.use_jito {
                    " + jito_tip_lamports"
                } else {
                    ""
                }
            ));
        }
    }
    if !sn.min_liquidity_sol.is_finite() || sn.min_liquidity_sol < 0.0 {
        return Err(BotError::config("sniper.min_liquidity_sol must be >= 0"));
    }
    if !sn.max_creator_initial_buy_sol.is_finite() || sn.max_creator_initial_buy_sol < 0.0 {
        return Err(BotError::config(
            "sniper.max_creator_initial_buy_sol must be >= 0",
        ));
    }
    if !sn.min_pool_supply_fraction.is_finite()
        || !(0.0..=1.0).contains(&sn.min_pool_supply_fraction)
    {
        return Err(BotError::config(
            "sniper.min_pool_supply_fraction must be in [0, 1]",
        ));
    }
    if sn.max_snapshot_age_ms == 0 {
        return Err(BotError::config("sniper.max_snapshot_age_ms must be > 0"));
    }
    if sn.stale_position_exit_secs < 0 {
        return Err(BotError::config(
            "sniper.stale_position_exit_secs must be >= 0 (0 = off)",
        ));
    }
    if sn.exit_retry_backoff_secs == 0 {
        w.push(
            "sniper.exit_retry_backoff_secs = 0 — a failed exit is retried on every sweep".into(),
        );
    }
    if !(0.0..=1.0).contains(&sn.take_profit_sell_fraction) {
        return Err(BotError::config(
            "sniper.take_profit_sell_fraction must be in [0, 1]",
        ));
    }
    if sn.trade_raydium && !sn.use_log_subscription && !sn.use_transaction_subscribe {
        w.push(
            "sniper.trade_raydium is on but neither use_log_subscription nor use_transaction_subscribe is — Raydium pools are only detectable through a Solana websocket feed"
                .into(),
        );
    }
    if sn.trade_pumpswap && !sn.use_log_subscription && !sn.use_transaction_subscribe {
        w.push(
            "sniper.trade_pumpswap is on but neither use_log_subscription nor use_transaction_subscribe is — PumpSwap pool creations are only detectable through a Solana websocket feed"
                .into(),
        );
    }
    let r = &c.risk;
    for (label, v) in [
        (
            "risk.sniper_max_position_quote",
            r.sniper_max_position_quote,
        ),
        (
            "risk.sniper_max_total_exposure_quote",
            r.sniper_max_total_exposure_quote,
        ),
        (
            "risk.sniper_daily_loss_limit_quote",
            r.sniper_daily_loss_limit_quote,
        ),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(BotError::config(format!("{label} must be >= 0 (0 = off)")));
        }
    }
    if r.sniper_token_cooldown_secs < 0 || r.sniper_failed_entry_cooldown_secs < 0 {
        return Err(BotError::config(
            "risk.sniper_token_cooldown_secs / sniper_failed_entry_cooldown_secs must be >= 0",
        ));
    }
    if r.sniper_max_position_quote > r.max_position_quote && r.max_position_quote > 0.0 {
        w.push(format!(
            "risk.sniper_max_position_quote = {} exceeds risk.max_position_quote = {} — the generic cap still applies",
            r.sniper_max_position_quote, r.max_position_quote
        ));
    }
    if r.sniper_emergency_disable {
        w.push("risk.sniper_emergency_disable is set — every sniper ENTRY will be refused".into());
    }
    Ok(())
}

/// Copy-trading engine settings (TASK 3): the `[copy]` pipeline knobs and
/// the Module-2 exposure controls in `[risk]`.
fn validate_copy_engine(c: &Config, w: &mut Vec<String>) -> BotResult<()> {
    let cp = &c.copy;
    if cp.max_event_age_secs < 0 {
        return Err(BotError::config(
            "copy.max_event_age_secs must be >= 0 (0 = off)",
        ));
    }
    for (label, v) in [
        ("copy.max_sol_per_trade", cp.max_sol_per_trade),
        ("copy.min_mirror_sol", cp.min_mirror_sol),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(BotError::config(format!("{label} must be >= 0 (0 = off)")));
        }
    }
    if !cp.max_balance_fraction.is_finite() || !(0.0..=1.0).contains(&cp.max_balance_fraction) {
        return Err(BotError::config(
            "copy.max_balance_fraction must be in [0, 1] (0 = off)",
        ));
    }
    if cp.recovery_lookback_hours < 0 {
        return Err(BotError::config(
            "copy.recovery_lookback_hours must be >= 0 (0 = off)",
        ));
    }
    for wallet in &cp.wallets {
        if !wallet.max_exposure_sol.is_finite() || wallet.max_exposure_sol < 0.0 {
            return Err(BotError::config(format!(
                "copy wallet {} max_exposure_sol must be >= 0 (0 = off)",
                wallet.address
            )));
        }
        if wallet.max_staleness_secs < 0 {
            return Err(BotError::config(format!(
                "copy wallet {} max_staleness_secs must be >= 0 (0 = off)",
                wallet.address
            )));
        }
    }
    if cp.enabled && cp.reconcile_auto_exit {
        w.push(
            "copy.reconcile_auto_exit is on — reconciliation may SELL a mirrored position whose leader fully exited"
                .into(),
        );
    }
    if cp.enabled && cp.reconcile_interval_secs == 0 {
        w.push("copy.reconcile_interval_secs = 0 — leader activity is never reconciled against mirrored positions".into());
    }
    if cp.enabled && cp.wallets.iter().all(|wl| wl.paused) && !cp.wallets.is_empty() {
        w.push(
            "copy trading enabled but every configured wallet is paused — nothing will be mirrored"
                .into(),
        );
    }
    let r = &c.risk;
    for (label, v) in [
        ("risk.copy_max_position_quote", r.copy_max_position_quote),
        (
            "risk.copy_max_total_exposure_quote",
            r.copy_max_total_exposure_quote,
        ),
        (
            "risk.copy_daily_loss_limit_quote",
            r.copy_daily_loss_limit_quote,
        ),
        (
            "risk.copy_max_leader_exposure_quote",
            r.copy_max_leader_exposure_quote,
        ),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(BotError::config(format!("{label} must be >= 0 (0 = off)")));
        }
    }
    if r.copy_failed_entry_cooldown_secs < 0 {
        return Err(BotError::config(
            "risk.copy_failed_entry_cooldown_secs must be >= 0",
        ));
    }
    if r.copy_max_position_quote > r.max_position_quote && r.max_position_quote > 0.0 {
        w.push(format!(
            "risk.copy_max_position_quote = {} exceeds risk.max_position_quote = {} — the generic cap still applies",
            r.copy_max_position_quote, r.max_position_quote
        ));
    }
    if r.copy_emergency_disable {
        w.push("risk.copy_emergency_disable is set — every mirrored ENTRY will be refused".into());
    }
    Ok(())
}

/// TASK 4: Polymarket order-pipeline and exposure-control invariants.
fn validate_polymarket_engine(c: &Config, w: &mut Vec<String>) -> BotResult<()> {
    let p = &c.polymarket;
    for (label, v) in [
        ("polymarket.stake_usd", p.stake_usd),
        ("polymarket.min_order_size", p.min_order_size),
    ] {
        if !v.is_finite() || v <= 0.0 {
            return Err(BotError::config(format!("{label} must be > 0")));
        }
    }
    for (label, v) in [
        ("polymarket.min_edge", p.min_edge),
        ("polymarket.min_liquidity_usd", p.min_liquidity_usd),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(BotError::config(format!("{label} must be >= 0")));
        }
    }
    for (label, v) in [
        ("polymarket.max_spread", p.max_spread),
        ("polymarket.reprice_threshold", p.reprice_threshold),
    ] {
        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
            return Err(BotError::config(format!(
                "{label} must be in [0, 1] (probability units, 0 = off)"
            )));
        }
    }
    for (label, v) in [
        ("polymarket.quote_max_age_secs", p.quote_max_age_secs),
        (
            "polymarket.min_time_to_resolution_secs",
            p.min_time_to_resolution_secs,
        ),
        ("polymarket.order_ttl_secs", p.order_ttl_secs),
        ("polymarket.expiration_secs", p.expiration_secs),
    ] {
        if v < 0 {
            return Err(BotError::config(format!("{label} must be >= 0 (0 = off)")));
        }
    }
    if p.order_poll_interval_secs == 0 {
        return Err(BotError::config(
            "polymarket.order_poll_interval_secs must be > 0",
        ));
    }
    let order_type = p.order_type.trim().to_ascii_uppercase();
    if !matches!(order_type.as_str(), "GTC" | "GTD" | "FOK" | "FAK") {
        return Err(BotError::config(format!(
            "polymarket.order_type = \"{}\" — expected GTC, GTD, FOK or FAK",
            p.order_type
        )));
    }
    if order_type == "GTD" && p.expiration_secs == 0 {
        return Err(BotError::config(
            "polymarket.order_type = GTD requires expiration_secs > 0",
        ));
    }
    if p.max_open_markets == 0 {
        return Err(BotError::config("polymarket.max_open_markets must be > 0"));
    }
    if p.enabled && p.reconcile_interval_secs == 0 {
        w.push(
            "polymarket.reconcile_interval_secs = 0 — local orders are never compared with the venue"
                .into(),
        );
    }
    if p.enabled && p.reconcile_cancel_orphans {
        w.push(
            "polymarket.reconcile_cancel_orphans is on — reconciliation will CANCEL venue orders it does not recognise"
                .into(),
        );
    }
    if p.enabled && !p.cancel_on_shutdown && !p.heartbeat {
        w.push(
            "polymarket.cancel_on_shutdown and heartbeat are both off — resting orders survive a crash unattended"
                .into(),
        );
    }
    let r = &c.risk;
    for (label, v) in [
        ("risk.poly_max_position_quote", r.poly_max_position_quote),
        (
            "risk.poly_max_total_exposure_quote",
            r.poly_max_total_exposure_quote,
        ),
        (
            "risk.poly_max_market_exposure_quote",
            r.poly_max_market_exposure_quote,
        ),
        (
            "risk.poly_daily_loss_limit_quote",
            r.poly_daily_loss_limit_quote,
        ),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(BotError::config(format!("{label} must be >= 0 (0 = off)")));
        }
    }
    if r.poly_max_position_quote > r.max_position_quote && r.max_position_quote > 0.0 {
        w.push(format!(
            "risk.poly_max_position_quote = {} exceeds risk.max_position_quote = {} — the generic cap still applies",
            r.poly_max_position_quote, r.max_position_quote
        ));
    }
    if r.poly_emergency_disable {
        w.push(
            "risk.poly_emergency_disable is set — every Polymarket ENTRY will be refused".into(),
        );
    }
    Ok(())
}

/// TASK 6 `[ha]` sanity: a known mode, sane heartbeat/lease windows and
/// only known role names in `required_roles`.
fn validate_ha(c: &Config, w: &mut Vec<String>) -> BotResult<()> {
    let ha = &c.ha;
    if crate::ha::HaMode::parse(&ha.mode).is_none() {
        return Err(BotError::config(format!(
            "ha.mode '{}' is unknown (use single | active_passive | active_active)",
            ha.mode
        )));
    }
    if ha.heartbeat_secs == 0 {
        return Err(BotError::config("ha.heartbeat_secs must be >= 1"));
    }
    if ha.heartbeat_timeout_secs > 0 && ha.heartbeat_timeout_secs < ha.heartbeat_secs * 2 {
        w.push(format!(
            "ha.heartbeat_timeout_secs ({}) is less than twice ha.heartbeat_secs ({}); a single missed beat would mark this worker stale — the effective timeout is raised to {}s",
            ha.heartbeat_timeout_secs,
            ha.heartbeat_secs,
            ha.heartbeat_timeout().as_secs()
        ));
    }
    if ha.role_lease_secs > 0 && ha.role_lease_secs < 5 {
        w.push(format!(
            "ha.role_lease_secs ({}) is below the 5s floor; the floor is used",
            ha.role_lease_secs
        ));
    }
    for r in &ha.required_roles {
        if crate::ha::LeaseRole::parse(r).is_none() {
            return Err(BotError::config(format!(
                "ha.required_roles: unknown role '{r}' (use reconciliation | recovery | accounting_maintenance | state_sync | feed:<name>)"
            )));
        }
    }
    if ha.ha_mode().is_clustered() && !c.database.enabled {
        return Err(BotError::config(format!(
            "ha.mode = '{}' needs [database].enabled = true: worker registration, leases and cursors must be durable and shared, and the in-memory store is process-local",
            ha.mode
        )));
    }
    Ok(())
}

/// TASK 5 `[global_risk]` sanity: finite non-negative limits, positive
/// rates, known venue names, a fraction for the drawdown percentage.
fn validate_global_risk(c: &Config, w: &mut Vec<String>) -> BotResult<()> {
    let g = &c.global_risk;
    if g.reference_asset.trim().is_empty() {
        return Err(BotError::config(
            "global_risk.reference_asset must not be empty",
        ));
    }
    for (label, v) in [
        ("global_risk.capital_base_ref", g.capital_base_ref),
        (
            "global_risk.max_portfolio_exposure_ref",
            g.max_portfolio_exposure_ref,
        ),
        (
            "global_risk.max_wallet_exposure_ref",
            g.max_wallet_exposure_ref,
        ),
        (
            "global_risk.max_venue_exposure_ref",
            g.max_venue_exposure_ref,
        ),
        (
            "global_risk.max_strategy_exposure_ref",
            g.max_strategy_exposure_ref,
        ),
        (
            "global_risk.max_asset_exposure_ref",
            g.max_asset_exposure_ref,
        ),
        (
            "global_risk.max_order_notional_ref",
            g.max_order_notional_ref,
        ),
        ("global_risk.max_daily_loss_ref", g.max_daily_loss_ref),
        ("global_risk.max_drawdown_ref", g.max_drawdown_ref),
    ] {
        if !v.is_finite() || v < 0.0 {
            return Err(BotError::config(format!("{label} must be >= 0 (0 = off)")));
        }
    }
    if !g.max_drawdown_pct.is_finite() || !(0.0..=1.0).contains(&g.max_drawdown_pct) {
        return Err(BotError::config(
            "global_risk.max_drawdown_pct must be in [0, 1] (0 = off)",
        ));
    }
    if g.max_drawdown_pct > 0.0 && g.capital_base_ref <= 0.0 {
        w.push(
            "global_risk.max_drawdown_pct is set but capital_base_ref is 0 — the percentage limit is inactive"
                .into(),
        );
    }
    for (asset, rate) in &g.reference_rates {
        if asset.trim().is_empty() {
            return Err(BotError::config(
                "global_risk.reference_rates: asset name must not be empty",
            ));
        }
        if !rate.is_finite() || *rate <= 0.0 {
            return Err(BotError::config(format!(
                "global_risk.reference_rates.{asset} must be a positive number"
            )));
        }
    }
    for v in &g.killed_venues {
        if Venue::parse(v.trim()).is_none() {
            return Err(BotError::config(format!(
                "global_risk.killed_venues: unknown venue '{v}' (use Venue names such as polymarket, pump.fun, pumpswap, raydium-amm-v4, raydium-clmm, jupiter, paper)"
            )));
        }
    }
    for st in &g.killed_strategies {
        if st.trim().is_empty() {
            return Err(BotError::config(
                "global_risk.killed_strategies: strategy label must not be empty",
            ));
        }
    }
    if g.needs_reference_rates() && !g.reference_rates.contains_key("SOL") {
        w.push(
            "global_risk: reference-denominated limits are on but reference_rates has no SOL entry — Solana entries will be refused (reference_rate_missing) until one is configured"
                .into(),
        );
    }
    Ok(())
}

fn validate(c: &Config, w: &mut Vec<String>) -> BotResult<()> {
    // --- signing identities (key-custody boundary) ---
    let mut seen: Vec<&str> = Vec::new();
    seen.push(PRIMARY_SIGNER_IDENTITY);
    for id in &c.signing.identities {
        let name = id.name.trim();
        if name.is_empty() {
            return Err(BotError::config(
                "signing.identities: name must not be empty",
            ));
        }
        if name == PRIMARY_SIGNER_IDENTITY {
            return Err(BotError::config(format!(
                "signing.identities: '{PRIMARY_SIGNER_IDENTITY}' is reserved for the main wallet \
                 and cannot be redefined"
            )));
        }
        if seen.contains(&name) {
            return Err(BotError::config(format!(
                "signing.identities: duplicate identity '{name}'"
            )));
        }
        match id.source_count() {
            1 => {}
            0 => {
                return Err(BotError::config(format!(
                    "signing.identities[{name}]: exactly one of alias / keypair_env / \
                     keypair_path must be set"
                )))
            }
            _ => {
                return Err(BotError::config(format!(
                    "signing.identities[{name}]: alias / keypair_env / keypair_path are \
                     mutually exclusive (exactly one must be set)"
                )))
            }
        }
        if let Some(alias) = &id.alias {
            let alias = alias.trim();
            if !seen.contains(&alias) {
                return Err(BotError::config(format!(
                    "signing.identities[{name}]: alias '{alias}' must reference \
                     '{PRIMARY_SIGNER_IDENTITY}' or an identity defined earlier in the list"
                )));
            }
        }
        if let Some(env) = &id.keypair_env {
            if env.trim().is_empty() {
                return Err(BotError::config(format!(
                    "signing.identities[{name}]: keypair_env must not be empty"
                )));
            }
        }
        if let Some(path) = &id.keypair_path {
            if path.trim().is_empty() {
                return Err(BotError::config(format!(
                    "signing.identities[{name}]: keypair_path must not be empty"
                )));
            }
        }
        seen.push(name);
    }

    if c.execution.mode.is_live() && !c.execution.allow_live_trading {
        w.push(
            "EXECUTION_MODE=live but ALLOW_LIVE_TRADING is not true — falling back to simulate"
                .into(),
        );
    }
    // --- execution reliability: fee policy + retry policy ---
    if !matches!(
        c.execution.fee_mode.trim().to_ascii_lowercase().as_str(),
        "fixed" | "adaptive"
    ) {
        return Err(BotError::config(format!(
            "execution.fee_mode = {:?} — must be \"fixed\" or \"adaptive\"",
            c.execution.fee_mode
        )));
    }
    if c.execution.fee_min_micro_lamports > c.execution.fee_max_micro_lamports {
        return Err(BotError::config(
            "execution.fee_min_micro_lamports must be <= execution.fee_max_micro_lamports",
        ));
    }
    if c.execution.fee_max_micro_lamports > c.execution.fee_emergency_max_micro_lamports {
        return Err(BotError::config(
            "execution.fee_max_micro_lamports must be <= execution.fee_emergency_max_micro_lamports",
        ));
    }
    if c.execution.priority_fee_micro_lamports > c.execution.fee_emergency_max_micro_lamports {
        return Err(BotError::config(
            "execution.priority_fee_micro_lamports exceeds execution.fee_emergency_max_micro_lamports",
        ));
    }
    if c.execution.priority_fee_micro_lamports > c.execution.fee_max_micro_lamports {
        w.push(format!(
            "execution.priority_fee_micro_lamports ({}) is above fee_max_micro_lamports ({}) — \
             the executor clamps every fee to the maximum",
            c.execution.priority_fee_micro_lamports, c.execution.fee_max_micro_lamports
        ));
    }
    if c.execution.fee_percentile == 0 || c.execution.fee_percentile > 100 {
        return Err(BotError::config(
            "execution.fee_percentile must be in [1, 100]",
        ));
    }
    if c.execution.fee_escalation_pct > 1_000 {
        return Err(BotError::config(
            "execution.fee_escalation_pct must be <= 1000",
        ));
    }
    if c.execution.max_blockhash_age_ms == 0 || c.execution.max_blockhash_age_ms > 90_000 {
        return Err(BotError::config(
            "execution.max_blockhash_age_ms must be in (0, 90000] (a blockhash lives ~60-90 s)",
        ));
    }
    if c.network.retry_base_backoff_ms == 0 {
        return Err(BotError::config(
            "network.retry_base_backoff_ms must be > 0",
        ));
    }
    if c.network.retry_max_backoff_ms < c.network.retry_base_backoff_ms {
        return Err(BotError::config(
            "network.retry_max_backoff_ms must be >= network.retry_base_backoff_ms",
        ));
    }
    if c.network.provider_failure_threshold == 0 {
        return Err(BotError::config(
            "network.provider_failure_threshold must be >= 1",
        ));
    }
    if c.network.rpc_endpoints().is_empty() {
        return Err(BotError::config("network.rpc_url must not be empty"));
    }
    if c.network.ws_stale_after_ms != 0 && c.network.ws_stale_after_ms < 5_000 {
        w.push(format!(
            "network.ws_stale_after_ms = {} is very aggressive — the client pings every 20 s, \
             values under 5000 will reconnect constantly",
            c.network.ws_stale_after_ms
        ));
    }
    if c.risk.max_position_fraction <= 0.0 || c.risk.max_position_fraction > 1.0 {
        return Err(BotError::config(
            "risk.max_position_fraction must be in (0, 1]",
        ));
    }
    if c.risk.default_stop_loss_pct < 0.0 || c.risk.default_stop_loss_pct > 1.0 {
        return Err(BotError::config(
            "risk.default_stop_loss_pct must be in [0, 1]",
        ));
    }
    if c.sniper.slippage_pct < 0.0 || c.sniper.slippage_pct > 100.0 {
        return Err(BotError::config("sniper.slippage_pct must be in [0, 100]"));
    }
    if c.sniper.buy_sol <= 0.0 {
        return Err(BotError::config("sniper.buy_sol must be > 0"));
    }
    if c.risk.max_slippage_bps > 10_000 {
        return Err(BotError::config("risk.max_slippage_bps must be <= 10000"));
    }
    validate_sniper_engine(c, w)?;
    validate_copy_engine(c, w)?;
    validate_polymarket_engine(c, w)?;
    validate_global_risk(c, w)?;
    validate_ha(c, w)?;
    if c.polymarket.signature_type > 3 {
        return Err(BotError::config(
            "polymarket.signature_type must be 0, 1, 2 or 3",
        ));
    }
    if c.polymarket.signature_type != 0 && c.polymarket.funder_address.is_none() {
        w.push(
            "polymarket.signature_type != 0 but funder_address is empty — orders will be rejected"
                .into(),
        );
    }
    if c.polymarket.exchange_domain_version != "2" {
        w.push(format!(
            "polymarket.exchange_domain_version = \"{}\" — Polymarket CLOB V2 requires \"2\"",
            c.polymarket.exchange_domain_version
        ));
    }
    if c.polymarket.enabled && c.secrets.polygon_private_key.is_none() {
        w.push("polymarket enabled but POLYGON_PRIVATE_KEY is not set".into());
    }
    if (c.sniper.enabled || c.copy.enabled) && c.secrets.solana_keypair.is_none() {
        w.push(
            "sniper/copy enabled but SOLANA_KEYPAIR is not set — running in paper mode only".into(),
        );
    }
    if c.telegram.enabled {
        if c.secrets.telegram_bot_token.is_none() {
            w.push(format!(
                "telegram enabled but {} / TELOXIDE_TOKEN is not set",
                c.telegram.bot_token_env
            ));
        }
        if c.telegram.allowed_chat_ids.is_empty() && c.telegram.allowed_user_ids.is_empty() {
            w.push(
                "telegram has no allowed_chat_ids / allowed_user_ids — control commands will be refused"
                    .into(),
            );
        }
    }
    if c.copy.enabled && c.copy.wallets.is_empty() {
        w.push("copy trading enabled but no wallets are configured".into());
    }
    for wallet in &c.copy.wallets {
        if wallet.address.trim().is_empty() {
            return Err(BotError::config("copy.wallets contains an empty address"));
        }
        if wallet.fraction_of_their_size <= 0.0 {
            return Err(BotError::config(
                "copy wallet fraction_of_their_size must be > 0",
            ));
        }
    }
    if c.api.bind_port == 0 {
        return Err(BotError::config("api.bind_port must not be 0"));
    }
    if c.observability.log_level.trim().is_empty() {
        return Err(BotError::config(
            "observability.log_level must not be empty",
        ));
    }
    if !matches!(
        c.observability.log_level.to_ascii_lowercase().as_str(),
        "error" | "warn" | "warning" | "info" | "debug" | "trace" | "off"
    ) {
        w.push(format!(
            "observability.log_level = {:?} is not a recognised level \
             (error/warn/info/debug/trace) — EnvFilter may reject it at startup",
            c.observability.log_level
        ));
    }
    match c.observability.log_format.as_str() {
        "text" | "json" => {}
        other => {
            return Err(BotError::config(format!(
                "observability.log_format = {other:?} is invalid — expected \"text\" or \"json\""
            )));
        }
    }
    if c.observability.sample_interval_ms < 100 {
        return Err(BotError::config(
            "observability.sample_interval_ms must be >= 100",
        ));
    }
    match c.storage.dedup_backend.as_str() {
        "memory" | "redis" | "postgres" => {}
        other => {
            return Err(BotError::config(format!(
                "storage.dedup_backend = {other:?} is invalid — expected \"memory\", \"redis\" or \"postgres\""
            )))
        }
    }
    if c.storage.dedup_backend == "postgres" && !c.database.enabled {
        return Err(BotError::config(
            "storage.dedup_backend = \"postgres\" requires [database] enabled = true",
        ));
    }
    if c.storage.dedup_backend == "redis" && !c.redis.enabled {
        return Err(BotError::config(
            "storage.dedup_backend = \"redis\" requires [redis] enabled = true",
        ));
    }
    if c.database.enabled {
        if c.database.url_env.trim().is_empty() {
            return Err(BotError::config("database.url_env must not be empty"));
        }
        if c.database.max_connections == 0 {
            return Err(BotError::config("database.max_connections must be >= 1"));
        }
        if c.database.min_connections > c.database.max_connections {
            return Err(BotError::config(
                "database.min_connections must be <= max_connections",
            ));
        }
        if c.database.acquire_timeout_ms == 0 || c.database.query_timeout_ms == 0 {
            return Err(BotError::config(
                "database timeouts must be > 0 (fail fast, never hang)",
            ));
        }
    }
    if c.redis.enabled && c.redis.url_env.trim().is_empty() {
        return Err(BotError::config("redis.url_env must not be empty"));
    }
    for key in &c.auth.keys {
        match key.role.as_str() {
            "owner" | "operator" | "readonly" => {}
            other => {
                return Err(BotError::config(format!(
                    "auth.keys: role {other:?} is invalid — expected \"owner\", \"operator\" or \"readonly\""
                )))
            }
        }
        if key.key_env.trim().is_empty() {
            return Err(BotError::config(format!(
                "auth.keys[{}]: key_env must not be empty",
                key.label
            )));
        }
    }
    Ok(())
}

/// Produce a commented `config.toml` template with all current values.
pub fn default_config_toml() -> String {
    let cfg = Config::default();
    let body = toml::to_string_pretty(&cfg).unwrap_or_else(|_| "# serialisation failed".into());
    format!(
        r#"# ---------------------------------------------------------------------------
# Sniper Suite configuration.
#
# Everything here can be overridden by environment variables (see README).
# SECRETS MUST NEVER GO IN THIS FILE — they are read from the environment only.
#
# SAFETY: execution.mode defaults to "paper". To trade for real you must set
#   execution.mode = "live"  AND  execution.allow_live_trading = true
# ---------------------------------------------------------------------------

{body}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_and_ctf_defaults_are_safe() {
        // Intent journal defaults ON (safety over sub-ms latency) and the
        // Polymarket CTF reader defaults to the public Polygon RPC; an
        // operator can disable either explicitly.
        let r = RecoveryConfig::default();
        assert!(r.intent_journal);
        assert!(r.block_modules_on_unresolved);
        assert_eq!(
            PolymarketConfig::default().ctf_rpc_url,
            "https://polygon-rpc.com"
        );
    }

    #[test]
    fn default_config_round_trips_and_carries_dedup_cap() {
        let text = default_config_toml();
        let cfg: Config = toml::from_str(&text).expect("generated default config must re-parse");
        assert_eq!(cfg.storage.max_dedup_entries, 100_000);
        assert_eq!(cfg.storage.max_trades_in_memory, 1_000);
        assert_eq!(cfg.observability.log_level, "info");
        assert_eq!(cfg.observability.log_format, "text");
        assert!(cfg.observability.metrics_enabled);
        assert_eq!(cfg.observability.sample_interval_ms, 5_000);
    }

    #[test]
    fn execution_reliability_defaults_are_coherent_and_validated() {
        let cfg = Config::default();
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("defaults must validate");
        assert!(!cfg.execution.fee_adaptive());
        assert!(cfg.execution.fee_min_micro_lamports <= cfg.execution.priority_fee_micro_lamports);
        assert!(cfg.execution.priority_fee_micro_lamports <= cfg.execution.fee_max_micro_lamports);
        assert!(
            cfg.execution.fee_max_micro_lamports <= cfg.execution.fee_emergency_max_micro_lamports
        );
        assert_eq!(
            cfg.network.rpc_endpoints().len(),
            3,
            "primary + 2 fallbacks"
        );
        assert!(cfg.network.retry_jitter);

        let mut bad = Config::default();
        bad.execution.fee_mode = "yolo".into();
        assert!(validate(&bad, &mut Vec::new()).is_err(), "unknown fee mode");

        let mut bad = Config::default();
        bad.execution.fee_max_micro_lamports = bad.execution.fee_emergency_max_micro_lamports + 1;
        assert!(
            validate(&bad, &mut Vec::new()).is_err(),
            "max above emergency"
        );

        let mut bad = Config::default();
        bad.execution.priority_fee_micro_lamports =
            bad.execution.fee_emergency_max_micro_lamports + 1;
        assert!(
            validate(&bad, &mut Vec::new()).is_err(),
            "base fee above emergency"
        );

        let mut bad = Config::default();
        bad.execution.fee_percentile = 0;
        assert!(validate(&bad, &mut Vec::new()).is_err(), "percentile 0");

        let mut bad = Config::default();
        bad.network.retry_max_backoff_ms = 1;
        assert!(
            validate(&bad, &mut Vec::new()).is_err(),
            "max backoff below base"
        );

        let mut bad = Config::default();
        bad.execution.max_blockhash_age_ms = 0;
        assert!(validate(&bad, &mut Vec::new()).is_err(), "blockhash age 0");

        // Duplicate / blank endpoints collapse; order is preserved.
        let net = NetworkConfig {
            rpc_url: "https://a".into(),
            rpc_url_fallbacks: vec!["".into(), "https://a".into(), " https://b ".into()],
            ..Default::default()
        };
        assert_eq!(net.rpc_endpoints(), vec!["https://a", "https://b"]);
    }

    #[test]
    fn legacy_config_without_reliability_keys_still_parses() {
        // Existing config files predate the fee/retry keys: serde(default)
        // must fill them in and validation must pass unchanged.
        let text = r#"
[network]
rpc_url = "https://api.mainnet-beta.solana.com"
[execution]
mode = "paper"
priority_fee_micro_lamports = 250000
"#;
        let cfg: Config = toml::from_str(text).expect("legacy config parses");
        assert_eq!(cfg.execution.fee_mode, "fixed");
        assert_eq!(cfg.network.provider_failure_threshold, 3);
        validate(&cfg, &mut Vec::new()).expect("legacy config validates");
    }

    #[test]
    fn observability_config_rejects_invalid_values() {
        let mut cfg = Config::default();
        cfg.observability.log_format = "yaml".into();
        let mut w = Vec::new();
        assert!(validate(&cfg, &mut w).is_err(), "bad log_format must fail");

        let mut cfg = Config::default();
        cfg.observability.sample_interval_ms = 10;
        let mut w = Vec::new();
        assert!(
            validate(&cfg, &mut w).is_err(),
            "interval below 100ms must fail"
        );

        let mut cfg = Config::default();
        cfg.observability.log_level = "".into();
        let mut w = Vec::new();
        assert!(validate(&cfg, &mut w).is_err(), "empty level must fail");
    }

    #[test]
    fn observability_config_warns_on_unknown_level_but_accepts_it() {
        let mut cfg = Config::default();
        cfg.observability.log_level = "chatty".into();
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("unknown level is a warning, not an error");
        assert!(
            w.iter().any(|m| m.contains("log_level")),
            "expected a warning about the unrecognised level, got {w:?}"
        );
    }

    #[test]
    fn bundled_example_config_parses() {
        // config.toml.example lives at the workspace root; this crate is at
        // <root>/crates/core, so walk up two levels. Guards against drift
        // between the schema and the documented example (deny_unknown_fields).
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config.toml.example");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let cfg: Config = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("config.toml.example failed to parse: {e}"));
        assert!(cfg.storage.max_dedup_entries > 0);
        // API must default to loopback so an unkeyed control plane is not
        // reachable off-box (fail-closed hardening).
        assert_eq!(cfg.api.bind_host, "127.0.0.1");
        // Every TASK 4 key is documented in the example (serde defaults would
        // otherwise hide a missing line).
        for key in [
            "poly_max_position_quote",
            "poly_max_total_exposure_quote",
            "poly_max_market_exposure_quote",
            "poly_max_concurrent_positions",
            "poly_max_open_orders",
            "poly_daily_loss_limit_quote",
            "poly_emergency_disable",
            "max_spread",
            "min_liquidity_usd",
            "quote_max_age_secs",
            "min_time_to_resolution_secs",
            "min_order_size",
            "order_poll_interval_secs",
            "order_ttl_secs",
            "reprice_threshold",
            "use_user_websocket",
            "cancel_on_shutdown",
            "reconcile_interval_secs",
            "reconcile_cancel_orphans",
            // TASK 5 `[global_risk]` keys.
            "reference_asset",
            "capital_base_ref",
            "max_portfolio_exposure_ref",
            "max_wallet_exposure_ref",
            "max_venue_exposure_ref",
            "max_strategy_exposure_ref",
            "max_asset_exposure_ref",
            "max_order_notional_ref",
            "max_daily_loss_ref",
            "max_drawdown_ref",
            "max_drawdown_pct",
            "killed_venues",
            "killed_strategies",
            "accounting_reconcile_interval_secs",
            // TASK 6 `[ha]` keys.
            "mode",
            "heartbeat_secs",
            "heartbeat_timeout_secs",
            "role_lease_secs",
            "required_roles",
        ] {
            assert!(
                text.lines()
                    .any(|l| l.trim_start().starts_with(&format!("{key} ="))),
                "config.toml.example is missing `{key}`"
            );
        }
        // TASK 6: the example's `[ha]` section is the compiled default —
        // single-worker mode, no required roles, so an unconfigured suite
        // behaves exactly as before TASK 6.
        let h = HaConfig::default();
        assert_eq!(cfg.ha.mode, h.mode);
        assert_eq!(cfg.ha.ha_mode(), crate::ha::HaMode::Single);
        assert_eq!(cfg.ha.heartbeat_secs, h.heartbeat_secs);
        assert_eq!(cfg.ha.heartbeat_timeout_secs, h.heartbeat_timeout_secs);
        assert_eq!(cfg.ha.role_lease_secs, h.role_lease_secs);
        assert!(cfg.ha.required_roles.is_empty());
        // TASK 5: the example's global section is the compiled default —
        // every limit off, USDC pinned at 1.0, no kill lists — so an
        // unconfigured suite behaves exactly as before TASK 5.
        let g = GlobalRiskConfig::default();
        assert_eq!(cfg.global_risk.reference_asset, g.reference_asset);
        assert_eq!(cfg.global_risk.reference_rates, g.reference_rates);
        assert_eq!(cfg.global_risk.max_open_positions, 0);
        assert!(!cfg.global_risk.needs_reference_rates());
        assert!(cfg.global_risk.killed_venues.is_empty());
        assert!(cfg.global_risk.killed_strategies.is_empty());
        // The example's values equal the compiled defaults for the lifecycle
        // knobs, so a fresh deployment behaves exactly as documented.
        let d = PolymarketConfig::default();
        assert_eq!(cfg.polymarket.min_order_size, d.min_order_size);
        assert_eq!(
            cfg.polymarket.order_poll_interval_secs,
            d.order_poll_interval_secs
        );
        assert_eq!(
            cfg.polymarket.reconcile_interval_secs,
            d.reconcile_interval_secs
        );
        assert_eq!(
            cfg.polymarket.reconcile_cancel_orphans,
            d.reconcile_cancel_orphans
        );
        assert_eq!(cfg.polymarket.use_user_websocket, d.use_user_websocket);
        assert_eq!(cfg.polymarket.cancel_on_shutdown, d.cancel_on_shutdown);
        assert_eq!(cfg.risk.poly_max_open_orders, 0);
        assert!(!cfg.risk.poly_emergency_disable);
    }

    // ------------------------------------------------------------------
    // Signing / key-custody configuration boundary
    // ------------------------------------------------------------------

    fn validate_err(cfg: &Config) -> String {
        let mut w = Vec::new();
        validate(cfg, &mut w)
            .expect_err("config must fail validation")
            .to_string()
    }

    // ------------------------------------------------------------------
    // TASK 5 `[global_risk]`
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // TASK 6 `[ha]`
    // ------------------------------------------------------------------

    #[test]
    fn ha_defaults_are_single_worker_and_validate() {
        let cfg = Config::default();
        assert_eq!(cfg.ha.ha_mode(), crate::ha::HaMode::Single);
        assert_eq!(cfg.ha.heartbeat().as_secs(), 10);
        assert_eq!(cfg.ha.heartbeat_timeout().as_secs(), 45);
        assert_eq!(cfg.ha.role_lease().as_secs(), 45);
        assert!(cfg.ha.required_role_list().is_empty());
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("defaults validate");
        assert!(w.iter().all(|x| !x.contains("ha.")), "{w:?}");
    }

    #[test]
    fn ha_rejects_unknown_modes_roles_and_clustered_without_a_database() {
        let mut cfg = Config::default();
        cfg.ha.mode = "clustered".into();
        assert!(validate_err(&cfg).contains("ha.mode"));

        let mut cfg = Config::default();
        cfg.ha.required_roles = vec!["nonsense".into()];
        assert!(validate_err(&cfg).contains("required_roles"));

        let mut cfg = Config::default();
        cfg.ha.heartbeat_secs = 0;
        assert!(validate_err(&cfg).contains("heartbeat_secs"));

        // A clustered mode needs durable shared state.
        let mut cfg = Config::default();
        cfg.ha.mode = "active_active".into();
        cfg.database.enabled = false;
        assert!(validate_err(&cfg).contains("database"));
        cfg.database.enabled = true;
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("clustered + database validates");
    }

    #[test]
    fn ha_clamps_and_warns_on_tight_windows() {
        let mut cfg = Config::default();
        cfg.ha.heartbeat_secs = 20;
        cfg.ha.heartbeat_timeout_secs = 25; // < 2 x heartbeat
        cfg.ha.role_lease_secs = 2; // below the floor
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("valid but warned");
        assert!(
            w.iter().any(|x| x.contains("heartbeat_timeout_secs")),
            "{w:?}"
        );
        assert!(w.iter().any(|x| x.contains("role_lease_secs")), "{w:?}");
        // The effective values are the clamped ones.
        assert_eq!(cfg.ha.heartbeat_timeout().as_secs(), 40);
        assert_eq!(cfg.ha.role_lease().as_secs(), 5);
    }

    #[test]
    fn ha_role_names_parse_into_lease_roles() {
        let mut cfg = Config::default();
        cfg.ha.required_roles = vec![
            "reconciliation".into(),
            "accounting_maintenance".into(),
            "feed:polymarket_user".into(),
        ];
        cfg.database.enabled = true;
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("known roles validate");
        let roles = cfg.ha.required_role_list();
        assert_eq!(roles.len(), 3);
        assert!(roles.contains(&crate::ha::LeaseRole::Reconciliation));
        assert!(roles.contains(&crate::ha::LeaseRole::Feed("polymarket_user".into())));
    }

    #[test]
    fn global_risk_defaults_are_off_and_validate() {
        let cfg = Config::default();
        let g = &cfg.global_risk;
        assert_eq!(g.reference_asset, "USD");
        assert_eq!(g.reference_rates.get("USDC"), Some(&1.0));
        assert!(!g.needs_reference_rates());
        assert_eq!(g.effective_drawdown_limit(), None);
        assert!(g.killed_venue_list().is_empty());
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("defaults validate");
        assert!(w.iter().all(|x| !x.contains("global_risk")));
    }

    #[test]
    fn global_risk_rejects_bad_limits_rates_and_venues() {
        let mut cfg = Config::default();
        cfg.global_risk.max_portfolio_exposure_ref = -1.0;
        assert!(validate_err(&cfg).contains("max_portfolio_exposure_ref"));
        let mut cfg = Config::default();
        cfg.global_risk.max_drawdown_pct = 1.5;
        assert!(validate_err(&cfg).contains("max_drawdown_pct"));
        let mut cfg = Config::default();
        cfg.global_risk.reference_rates.insert("SOL".into(), 0.0);
        assert!(validate_err(&cfg).contains("reference_rates.SOL"));
        let mut cfg = Config::default();
        cfg.global_risk.killed_venues = vec!["binance".into()];
        assert!(validate_err(&cfg).contains("killed_venues"));
        let mut cfg = Config::default();
        cfg.global_risk.killed_strategies = vec!["  ".into()];
        assert!(validate_err(&cfg).contains("killed_strategies"));
        let mut cfg = Config::default();
        cfg.global_risk.reference_asset = " ".into();
        assert!(validate_err(&cfg).contains("reference_asset"));
    }

    #[test]
    fn global_risk_drawdown_limit_takes_the_tighter_of_abs_and_pct() {
        let mut g = GlobalRiskConfig {
            max_drawdown_ref: 500.0,
            ..Default::default()
        };
        assert_eq!(g.effective_drawdown_limit(), Some(500.0));
        g.capital_base_ref = 10_000.0;
        g.max_drawdown_pct = 0.02; // 200
        assert_eq!(g.effective_drawdown_limit(), Some(200.0));
        g.max_drawdown_ref = 0.0;
        assert_eq!(g.effective_drawdown_limit(), Some(200.0));
        g.capital_base_ref = 0.0;
        assert_eq!(g.effective_drawdown_limit(), None);
        assert!(!g.needs_reference_rates());
        g.max_daily_loss_ref = 1.0;
        assert!(g.needs_reference_rates());
        // Turning a reference-denominated limit on without a SOL rate warns.
        let mut cfg = Config::default();
        cfg.global_risk.max_daily_loss_ref = 1.0;
        let mut w = Vec::new();
        validate(&cfg, &mut w).expect("valid");
        assert!(w.iter().any(|x| x.contains("no SOL entry")), "{w:?}");
        cfg.global_risk.killed_venues = vec!["polymarket".into(), "pump.fun".into()];
        assert_eq!(cfg.global_risk.killed_venue_list().len(), 2);
    }

    #[test]
    fn signing_defaults_to_local_with_no_extra_identities() {
        let cfg = Config::default();
        assert_eq!(cfg.signing.provider, SigningProvider::Local);
        assert!(cfg.signing.provider.is_supported());
        assert!(cfg.signing.identities.is_empty());
        let mut w = Vec::new();
        assert!(validate(&cfg, &mut w).is_ok());
    }

    #[test]
    fn signing_toml_parses_all_providers_and_identity_sources() {
        let text = r#"
[signing]
provider = "vault"

[[signing.identities]]
name = "treasury"
keypair_env = "TREASURY_KEYPAIR"

[[signing.identities]]
name = "sniper"
alias = "primary_trading"

[[signing.identities]]
name = "staking_admin"
keypair_path = "/run/secrets/staking_admin.json"

[[signing.identities]]
name = "copy_trading"
alias = "sniper"
"#;
        let cfg: Config = toml::from_str(text).expect("signing section must parse");
        assert_eq!(cfg.signing.provider, SigningProvider::Vault);
        assert!(!cfg.signing.provider.is_supported());
        assert_eq!(cfg.signing.identities.len(), 4);
        // Aliases may point at primary or any EARLIER identity (deterministic
        // registration order), so this config is structurally valid.
        let mut w = Vec::new();
        assert!(validate(&cfg, &mut w).is_ok());
    }

    #[test]
    fn signing_rejects_unknown_provider_at_parse_time() {
        let text = "[signing]\nprovider = \"yubikey\"\n";
        assert!(toml::from_str::<Config>(text).is_err());
    }

    fn cfg_with_identities(identities: Vec<SignerIdentityConfig>) -> Config {
        Config {
            signing: SigningConfig {
                provider: SigningProvider::Local,
                identities,
            },
            ..Config::default()
        }
    }

    #[test]
    fn signing_rejects_duplicate_and_reserved_identities() {
        let cfg = cfg_with_identities(vec![
            SignerIdentityConfig {
                name: "treasury".into(),
                keypair_env: Some("A".into()),
                ..Default::default()
            },
            SignerIdentityConfig {
                name: "treasury".into(),
                keypair_env: Some("B".into()),
                ..Default::default()
            },
        ]);
        assert!(validate_err(&cfg).contains("duplicate identity"));

        let cfg = cfg_with_identities(vec![SignerIdentityConfig {
            name: PRIMARY_SIGNER_IDENTITY.into(),
            alias: Some(PRIMARY_SIGNER_IDENTITY.into()),
            ..Default::default()
        }]);
        assert!(validate_err(&cfg).contains("reserved"));
    }

    #[test]
    fn signing_identity_needs_exactly_one_source() {
        let cfg = cfg_with_identities(vec![SignerIdentityConfig {
            name: "treasury".into(),
            ..Default::default()
        }]);
        assert!(validate_err(&cfg).contains("exactly one"));

        let cfg = cfg_with_identities(vec![SignerIdentityConfig {
            name: "treasury".into(),
            alias: Some(PRIMARY_SIGNER_IDENTITY.into()),
            keypair_env: Some("TREASURY_KEYPAIR".into()),
            ..Default::default()
        }]);
        assert!(validate_err(&cfg).contains("mutually exclusive"));
    }

    #[test]
    fn signing_alias_must_reference_a_known_earlier_identity() {
        let cfg = cfg_with_identities(vec![
            SignerIdentityConfig {
                name: "sniper".into(),
                alias: Some("treasury".into()),
                ..Default::default()
            },
            SignerIdentityConfig {
                name: "treasury".into(),
                keypair_env: Some("T".into()),
                ..Default::default()
            },
        ]);
        // Forward references are rejected: registration order must be
        // deterministic and an alias can only share an already-known signer.
        assert!(validate_err(&cfg).contains("must reference"));
    }

    #[test]
    fn secret_config_debug_never_emits_values() {
        let secrets = SecretConfig {
            solana_keypair: Some("[1,2,3,4,5,6,7,8,9,10,11,12]".into()),
            polygon_private_key: Some("0xdeadbeefdeadbeefdeadbeef".into()),
            telegram_bot_token: Some("123456:ABC-DEF".into()),
            ..SecretConfig::default()
        };
        let dbg = format!("{secrets:?}");
        assert!(!dbg.contains("1,2,3"), "debug leaked keypair bytes: {dbg}");
        assert!(!dbg.contains("deadbeef"), "debug leaked private key: {dbg}");
        assert!(!dbg.contains("ABC-DEF"), "debug leaked bot token: {dbg}");
        assert!(dbg.contains("<set>"));
        assert!(
            dbg.contains("<unset>"),
            "unset fields stay visible as unset"
        );
        // The whole Config (and AppConfig.raw) inherits this protection.
        let cfg = Config {
            secrets,
            ..Config::default()
        };
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("deadbeef"));
    }

    // ------------------------------------------------------------------
    // Sniper engine configuration (slippage engine, gates, exposure)
    // ------------------------------------------------------------------

    #[test]
    fn sniper_engine_defaults_preserve_legacy_behaviour() {
        // Every new knob defaults to "inherit the generic limit" or "off"
        // except the two that are safe for the pump.fun path by construction.
        let c = Config::default();
        assert_eq!(c.sniper.slippage_mode_normalized(), "fixed");
        assert!(c.sniper.slippage_overrides_bps.is_empty());
        assert_eq!(c.sniper.min_liquidity_sol, 0.0);
        assert!(!c.sniper.require_mint_authority_revoked);
        assert!(c.sniper.require_freeze_authority_revoked);
        assert_eq!(c.sniper.stale_position_exit_secs, 0);
        assert!(c.sniper.failed_entry_cleanup);
        assert_eq!(c.sniper.max_entry_fee_lamports, 0);
        assert_eq!(c.risk.sniper_position_cap(), c.risk.max_position_quote);
        assert_eq!(c.risk.sniper_position_limit(), c.risk.max_open_positions);
        assert_eq!(c.risk.sniper_max_total_exposure_quote, 0.0);
        assert!(!c.risk.sniper_emergency_disable);
        let mut w = Vec::new();
        assert!(validate(&c, &mut w).is_ok());
    }

    #[test]
    fn sniper_engine_rejects_unknown_slippage_mode_and_bad_bounds() {
        let mut c = Config::default();
        c.sniper.slippage_mode = "yolo".into();
        assert!(validate_err(&c).contains("slippage_mode"));

        let mut c = Config::default();
        c.sniper.slippage_mode = "Liquidity-Aware".into(); // normalised
        let mut w = Vec::new();
        assert!(validate(&c, &mut w).is_ok());
        assert_eq!(c.sniper.slippage_mode_normalized(), "liquidity_aware");

        let mut c = Config::default();
        c.sniper
            .slippage_overrides_bps
            .insert("So11111111111111111111111111111111111111112".into(), 10_001);
        assert!(validate_err(&c).contains("slippage_overrides_bps"));

        let mut c = Config::default();
        c.sniper.max_price_impact_bps = 20_000;
        assert!(validate_err(&c).contains("max_price_impact_bps"));

        let mut c = Config::default();
        c.sniper.min_pool_supply_fraction = 1.5;
        assert!(validate_err(&c).contains("min_pool_supply_fraction"));

        let mut c = Config::default();
        c.sniper.max_snapshot_age_ms = 0;
        assert!(validate_err(&c).contains("max_snapshot_age_ms"));

        // Fee budget: below one signature's base fee is an error; a budget
        // the configured first attempt cannot meet validates but warns.
        let mut c = Config::default();
        c.sniper.max_entry_fee_lamports = 4_999;
        assert!(validate_err(&c).contains("max_entry_fee_lamports"));

        let mut c = Config::default();
        c.sniper.max_entry_fee_lamports = 5_000;
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        assert!(w.iter().any(|m| m.contains("FEE_LIMIT")), "{w:?}");

        let mut c = Config::default();
        // default execution: 250 000 µlamports/CU × 400 000 CU = 100 000 + 5 000 base
        c.sniper.max_entry_fee_lamports = 105_000;
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        assert!(!w.iter().any(|m| m.contains("FEE_LIMIT")), "{w:?}");
        c.sniper.max_entry_fee_lamports = 104_999;
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        assert!(w.iter().any(|m| m.contains("FEE_LIMIT")), "{w:?}");
        c.execution.use_jito = true;
        c.sniper.max_entry_fee_lamports = 105_000 + c.execution.jito_tip_lamports;
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        assert!(!w.iter().any(|m| m.contains("FEE_LIMIT")), "{w:?}");

        let mut c = Config::default();
        c.risk.sniper_token_cooldown_secs = -1;
        assert!(validate_err(&c).contains("cooldown"));

        let mut c = Config::default();
        c.risk.sniper_daily_loss_limit_quote = -0.5;
        assert!(validate_err(&c).contains("sniper_daily_loss_limit_quote"));
    }

    #[test]
    fn sniper_engine_overrides_inherit_when_zero_and_warn_when_odd() {
        let mut c = Config::default();
        c.risk.sniper_max_position_quote = 0.2;
        c.risk.sniper_max_concurrent_positions = 3;
        assert_eq!(c.risk.sniper_position_cap(), 0.2);
        assert_eq!(c.risk.sniper_position_limit(), 3);

        // Emergency disable is legal but loud.
        let mut c = Config::default();
        c.risk.sniper_emergency_disable = true;
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        assert!(w.iter().any(|m| m.contains("sniper_emergency_disable")));

        // An override above the hard max is clamped at runtime — warned here.
        let mut c = Config::default();
        c.risk.max_slippage_bps = 1_000;
        c.sniper
            .slippage_overrides_bps
            .insert("So11111111111111111111111111111111111111112".into(), 5_000);
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        assert!(w.iter().any(|m| m.contains("will be clamped")));
    }

    // ------------------------------------------------------------------
    // Copy-trading engine (TASK 3)
    // ------------------------------------------------------------------

    #[test]
    fn copy_engine_defaults_preserve_legacy_behaviour() {
        let c = Config::default();
        assert_eq!(c.copy.max_event_age_secs, 30);
        assert!(!c.copy.strict_ordering);
        assert_eq!(c.copy.max_sol_per_trade, 0.0);
        assert_eq!(c.copy.max_balance_fraction, 0.0);
        assert_eq!(c.copy.min_mirror_sol, 0.0);
        assert_eq!(c.copy.reconcile_interval_secs, 60);
        assert!(!c.copy.reconcile_auto_exit);
        assert_eq!(c.copy.recovery_lookback_hours, 24);
        let w = CopyWallet::default();
        assert!(!w.paused);
        assert_eq!(w.max_exposure_sol, 0.0);
        assert_eq!(w.max_open_positions, 0);
        assert_eq!(c.risk.copy_position_cap(), c.risk.max_position_quote);
        assert_eq!(c.risk.copy_position_limit(), c.risk.max_open_positions);
        assert_eq!(c.risk.copy_max_pending_executions, 0);
        assert_eq!(c.risk.copy_failed_entry_cooldown_secs, 0);
        assert_eq!(c.risk.copy_daily_loss_limit_quote, 0.0);
        assert_eq!(c.risk.copy_max_leader_exposure_quote, 0.0);
        assert!(!c.risk.copy_emergency_disable);
        let mut warnings = Vec::new();
        assert!(validate(&c, &mut warnings).is_ok());
    }

    #[test]
    fn copy_engine_rejects_bad_bounds_and_warns_loudly() {
        let mut c = Config::default();
        c.copy.max_event_age_secs = -1;
        assert!(validate_err(&c).contains("max_event_age_secs"));

        let mut c = Config::default();
        c.copy.max_balance_fraction = 1.5;
        assert!(validate_err(&c).contains("max_balance_fraction"));

        let mut c = Config::default();
        c.copy.max_sol_per_trade = f64::NAN;
        assert!(validate_err(&c).contains("max_sol_per_trade"));

        let mut c = Config::default();
        c.copy.min_mirror_sol = -0.1;
        assert!(validate_err(&c).contains("min_mirror_sol"));

        let mut c = Config::default();
        c.copy.recovery_lookback_hours = -2;
        assert!(validate_err(&c).contains("recovery_lookback_hours"));

        let mut c = Config::default();
        c.copy.wallets = vec![CopyWallet {
            address: "whale".into(),
            max_exposure_sol: -1.0,
            ..CopyWallet::default()
        }];
        assert!(validate_err(&c).contains("max_exposure_sol"));

        let mut c = Config::default();
        c.copy.wallets = vec![CopyWallet {
            address: "whale".into(),
            max_staleness_secs: -5,
            ..CopyWallet::default()
        }];
        assert!(validate_err(&c).contains("max_staleness_secs"));

        for (label, mutate) in [
            (
                "copy_max_position_quote",
                (|r: &mut RiskConfig| r.copy_max_position_quote = -1.0) as fn(&mut RiskConfig),
            ),
            ("copy_max_total_exposure_quote", |r| {
                r.copy_max_total_exposure_quote = f64::INFINITY
            }),
            ("copy_daily_loss_limit_quote", |r| {
                r.copy_daily_loss_limit_quote = -0.5
            }),
            ("copy_max_leader_exposure_quote", |r| {
                r.copy_max_leader_exposure_quote = -0.5
            }),
            ("copy_failed_entry_cooldown_secs", |r| {
                r.copy_failed_entry_cooldown_secs = -1
            }),
        ] {
            let mut c = Config::default();
            mutate(&mut c.risk);
            assert!(validate_err(&c).contains(label), "{label}");
        }

        // Legal but loud.
        let mut c = Config::default();
        c.copy.enabled = true;
        c.copy.reconcile_auto_exit = true;
        c.copy.reconcile_interval_secs = 0;
        c.copy.wallets = vec![CopyWallet {
            address: "whale".into(),
            paused: true,
            ..CopyWallet::default()
        }];
        c.risk.copy_emergency_disable = true;
        c.risk.copy_max_position_quote = 5.0;
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        for needle in [
            "reconcile_auto_exit",
            "reconcile_interval_secs = 0",
            "every configured wallet is paused",
            "copy_emergency_disable",
            "copy_max_position_quote",
        ] {
            assert!(
                w.iter().any(|m| m.contains(needle)),
                "missing warning {needle}: {w:?}"
            );
        }
        assert_eq!(c.risk.copy_position_cap(), 5.0);
    }

    // ------------------------------------------------------------------
    // Polymarket engine (TASK 4)
    // ------------------------------------------------------------------

    #[test]
    fn polymarket_engine_defaults_preserve_legacy_behaviour() {
        let c = Config::default();
        let p = &c.polymarket;
        assert_eq!(p.max_spread, 0.10);
        assert_eq!(p.min_liquidity_usd, 0.0);
        assert_eq!(p.quote_max_age_secs, 120);
        assert_eq!(p.min_time_to_resolution_secs, 3600);
        assert_eq!(p.min_order_size, 5.0);
        assert_eq!(p.order_poll_interval_secs, 15);
        assert_eq!(p.order_ttl_secs, 0);
        assert_eq!(p.reprice_threshold, 0.0);
        assert_eq!(p.reconcile_interval_secs, 60);
        assert!(!p.reconcile_cancel_orphans);
        assert!(p.use_user_websocket);
        assert!(p.cancel_on_shutdown);
        assert_eq!(c.risk.poly_position_cap(), c.risk.max_position_quote);
        assert_eq!(c.risk.poly_position_limit(), c.risk.max_open_positions);
        assert_eq!(c.risk.poly_max_open_orders, 0);
        assert_eq!(c.risk.poly_max_market_exposure_quote, 0.0);
        assert_eq!(c.risk.poly_daily_loss_limit_quote, 0.0);
        assert!(!c.risk.poly_emergency_disable);
        let mut warnings = Vec::new();
        assert!(validate(&c, &mut warnings).is_ok());
    }

    #[test]
    fn polymarket_engine_rejects_bad_bounds_and_warns_loudly() {
        for (label, mutate) in [
            (
                "stake_usd",
                (|p: &mut PolymarketConfig| p.stake_usd = 0.0) as fn(&mut PolymarketConfig),
            ),
            ("min_order_size", |p| p.min_order_size = -1.0),
            ("min_edge", |p| p.min_edge = -0.01),
            ("min_liquidity_usd", |p| p.min_liquidity_usd = f64::NAN),
            ("max_spread", |p| p.max_spread = 1.5),
            ("reprice_threshold", |p| p.reprice_threshold = -0.1),
            ("quote_max_age_secs", |p| p.quote_max_age_secs = -1),
            ("min_time_to_resolution_secs", |p| {
                p.min_time_to_resolution_secs = -1
            }),
            ("order_ttl_secs", |p| p.order_ttl_secs = -5),
            ("expiration_secs", |p| p.expiration_secs = -5),
            ("order_poll_interval_secs", |p| {
                p.order_poll_interval_secs = 0
            }),
            ("order_type", |p| p.order_type = "IOC".into()),
            ("GTD requires expiration_secs", |p| {
                p.order_type = "gtd".into();
                p.expiration_secs = 0;
            }),
            ("max_open_markets", |p| p.max_open_markets = 0),
        ] {
            let mut c = Config::default();
            mutate(&mut c.polymarket);
            assert!(validate_err(&c).contains(label), "{label}");
        }
        for (label, mutate) in [
            (
                "poly_max_position_quote",
                (|r: &mut RiskConfig| r.poly_max_position_quote = -1.0) as fn(&mut RiskConfig),
            ),
            ("poly_max_total_exposure_quote", |r| {
                r.poly_max_total_exposure_quote = f64::INFINITY
            }),
            ("poly_max_market_exposure_quote", |r| {
                r.poly_max_market_exposure_quote = -0.5
            }),
            ("poly_daily_loss_limit_quote", |r| {
                r.poly_daily_loss_limit_quote = f64::NAN
            }),
        ] {
            let mut c = Config::default();
            mutate(&mut c.risk);
            assert!(validate_err(&c).contains(label), "{label}");
        }

        // Legal but loud.
        let mut c = Config::default();
        c.polymarket.enabled = true;
        c.polymarket.reconcile_interval_secs = 0;
        c.polymarket.reconcile_cancel_orphans = true;
        c.polymarket.cancel_on_shutdown = false;
        c.polymarket.heartbeat = false;
        c.risk.poly_emergency_disable = true;
        c.risk.poly_max_position_quote = 50.0;
        let mut w = Vec::new();
        validate(&c, &mut w).unwrap();
        for needle in [
            "reconcile_interval_secs = 0",
            "reconcile_cancel_orphans",
            "cancel_on_shutdown",
            "poly_emergency_disable",
            "poly_max_position_quote",
        ] {
            assert!(
                w.iter().any(|m| m.contains(needle)),
                "missing warning {needle}: {w:?}"
            );
        }
        assert_eq!(c.risk.poly_position_cap(), 50.0);
        c.risk.poly_max_concurrent_positions = 2;
        assert_eq!(c.risk.poly_position_limit(), 2);
    }

    #[test]
    fn polymarket_toml_accepts_the_new_keys() {
        let text = r#"
[polymarket]
enabled = false
max_spread = 0.05
min_liquidity_usd = 250.0
quote_max_age_secs = 30
min_time_to_resolution_secs = 7200
min_order_size = 5.0
order_poll_interval_secs = 10
order_ttl_secs = 900
reprice_threshold = 0.02
reconcile_interval_secs = 45
reconcile_cancel_orphans = true
use_user_websocket = false
cancel_on_shutdown = true

[risk]
poly_max_position_quote = 25.0
poly_max_total_exposure_quote = 100.0
poly_max_market_exposure_quote = 40.0
poly_max_concurrent_positions = 3
poly_max_open_orders = 4
poly_daily_loss_limit_quote = 20.0
poly_emergency_disable = false
"#;
        let c: Config = toml::from_str(text).expect("parses");
        assert_eq!(c.polymarket.max_spread, 0.05);
        assert_eq!(c.polymarket.min_liquidity_usd, 250.0);
        assert_eq!(c.polymarket.quote_max_age_secs, 30);
        assert_eq!(c.polymarket.min_time_to_resolution_secs, 7200);
        assert_eq!(c.polymarket.order_poll_interval_secs, 10);
        assert_eq!(c.polymarket.order_ttl_secs, 900);
        assert_eq!(c.polymarket.reprice_threshold, 0.02);
        assert_eq!(c.polymarket.reconcile_interval_secs, 45);
        assert!(c.polymarket.reconcile_cancel_orphans);
        assert!(!c.polymarket.use_user_websocket);
        assert_eq!(c.risk.poly_max_position_quote, 25.0);
        assert_eq!(c.risk.poly_max_total_exposure_quote, 100.0);
        assert_eq!(c.risk.poly_max_market_exposure_quote, 40.0);
        assert_eq!(c.risk.poly_max_concurrent_positions, 3);
        assert_eq!(c.risk.poly_max_open_orders, 4);
        assert_eq!(c.risk.poly_daily_loss_limit_quote, 20.0);
        let mut w = Vec::new();
        validate(&c, &mut w).expect("valid");
    }

    #[test]
    fn copy_wallet_toml_accepts_the_new_keys() {
        let text = r#"
[copy]
enabled = true
strict_ordering = true
max_sol_per_trade = 0.1
reconcile_auto_exit = false

[[copy.wallets]]
address = "WhalePubkey"
paused = true
max_exposure_sol = 0.5
max_open_positions = 2
"#;
        let cfg: Config = toml::from_str(text).expect("copy section parses");
        assert!(cfg.copy.strict_ordering);
        assert_eq!(cfg.copy.max_sol_per_trade, 0.1);
        assert!(cfg.copy.wallets[0].paused);
        assert_eq!(cfg.copy.wallets[0].max_exposure_sol, 0.5);
        assert_eq!(cfg.copy.wallets[0].max_open_positions, 2);
        assert_eq!(
            cfg.copy.wallets[0].max_staleness_secs, 20,
            "unset keys keep their defaults"
        );
    }

    #[test]
    fn sniper_engine_config_round_trips_through_toml() {
        let mut c = Config::default();
        c.sniper.slippage_mode = "price_impact".into();
        c.sniper
            .slippage_overrides_bps
            .insert("So11111111111111111111111111111111111111112".into(), 800);
        c.sniper.pumpswap_slippage_pct = Some(20.0);
        c.risk.sniper_max_pending_executions = 4;
        let text = toml::to_string(&c).expect("serialises");
        let back: Config =
            toml::from_str(&text).expect("deny_unknown_fields accepts its own output");
        assert_eq!(back.sniper.slippage_mode, "price_impact");
        assert_eq!(
            back.sniper
                .slippage_overrides_bps
                .get("So11111111111111111111111111111111111111112"),
            Some(&800)
        );
        assert_eq!(back.sniper.pumpswap_slippage_pct, Some(20.0));
        assert_eq!(back.risk.sniper_max_pending_executions, 4);
        // A config written BEFORE these fields existed still parses.
        let legacy = r#"
[sniper]
enabled = false
buy_sol = 0.02
[risk]
max_open_positions = 4
"#;
        let old: Config = toml::from_str(legacy).expect("legacy config parses");
        assert_eq!(old.sniper.buy_sol, 0.02);
        assert_eq!(old.sniper.slippage_mode, "fixed");
        assert_eq!(old.risk.sniper_position_limit(), 4);
    }
}
