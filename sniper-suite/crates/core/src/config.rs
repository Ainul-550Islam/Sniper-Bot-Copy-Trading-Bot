use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{BotError, BotResult};
use crate::models::{BotModule, ExecutionMode};

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
        }
    }
}

impl NetworkConfig {
    pub fn is_devnet(&self) -> bool {
        self.cluster.contains("devnet") || self.rpc_url.contains("devnet")
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
        }
    }
}

impl ExecutionConfig {
    /// The single source of truth for "may I really broadcast?".
    pub fn live_allowed(&self) -> bool {
        self.mode.is_live() && self.allow_live_trading
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
    /// Trade graduated tokens on PumpSwap instead of skipping them.
    pub trade_pumpswap: bool,
    /// Route through Raydium AMM v4 when a pool exists.
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
    /// Maximum time between seeing a launch and sending the buy.
    pub max_entry_latency_ms: u64,
    /// Skip launches older than this many seconds by the time we see them.
    pub max_launch_age_secs: i64,
    /// Denylist of creator wallets (base58) that never get sniped.
    pub creator_denylist: Vec<String>,
    /// Denylist of keywords in the token name/symbol.
    pub keyword_denylist: Vec<String>,
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
        }
    }
}

/// One wallet that Module 2 mirrors.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

impl Default for HaConfig {
    fn default() -> Self {
        HaConfig {
            replica_id: String::new(),
            claim_lease_secs: 45,
            claim_handoff_grace_secs: 900,
            flag_sync_secs: 5,
            book_sync_secs: 30,
        }
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
}
