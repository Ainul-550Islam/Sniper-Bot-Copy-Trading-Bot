//! Command parsing and dispatch for the Telegram control plane.
//!
//! [`parse_command`] is pure (and unit-tested); [`handle`] applies a parsed
//! command to shared state and returns the reply text. Authorization is
//! deny-by-default: if no allowlist is configured, control commands are refused.
//!
//! ## Trading commands (BATCH 04)
//!
//! This file is also the router for the consent-gated trading surface
//! (`/consent`, `/buy`, `/sell`, `/confirm`, `/cancel`, `/pending`). Those
//! commands are **not** handled here: they are handed to a
//! [`TradeConsole`] when one is attached to the bot, which keeps the existing
//! control commands byte-for-byte unchanged in behaviour. The console owns the
//! flow and delegates each concern to its single owner:
//!
//! | concern | owner |
//! |---|---|
//! | parsing / validation | [`parse`] |
//! | consent rules | [`crate::consent::policy`] |
//! | nonce generation + hashing | [`nonce`] |
//! | pending-command state machine | [`pending`] |
//! | caps + rate limiting | [`limits`] |
//! | audit + redaction | [`audit`] |
//! | user-facing text | [`reply`] |
//!
//! `src/commands.rs` + `src/commands/*.rs` is a single module: the file below
//! is the router, the directory holds the owners listed above.

use std::collections::HashMap;

use uuid::Uuid;

pub mod audit;
pub mod buy;
pub mod confirm;
pub mod consent_cmd;
pub mod limits;
pub mod nonce;
pub mod parse;
pub mod pending;
pub mod reply;
pub mod sell;

use bot_core::config::TelegramConfig;
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::state::Shared;

use crate::commands::audit::{
    AuditAmount, AuditBps, AuditDetail, AuditEvent, AuditStore, InMemoryAuditStore, TradeAuditEvent,
};
use crate::commands::limits::{LimitDeny, RateLimiter, TradeLimits};
use crate::commands::pending::{CommandStore, InMemoryCommandStore};
use crate::commands::reply::Reply;
use crate::consent::store::{ConsentStore, InMemoryConsentStore, StoreError};
use crate::consent::{ConsentBounds, ConsentError};
use crate::trade_port::{TradeAction, TradeCommandPort};

/// What a command acts on.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// Every trading module.
    All,
    /// A single module.
    Module(BotModule),
}

/// A parsed control command.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `/help` — list available commands.
    Help,
    /// `/status` — modules, PnL, kill-switch state.
    Status,
    /// `/on <module|all>` — enable a module at runtime.
    On(Target),
    /// `/off <module|all>` — disable a module at runtime.
    Off(Target),
    /// `/kill` — engage the emergency stop.
    Kill,
    /// `/resume` — clear the emergency stop.
    Resume,
    /// `/positions` — list open positions.
    Positions,
    /// `/trades` — recent fills.
    Trades,
    /// `/pnl` — realized/unrealized profit summary.
    Pnl,
    /// `/balance` — wallet balances.
    Balance,
    /// `mode` with no argument reports; with one it switches.
    Mode(Option<String>),
    /// `/config` — redacted effective configuration.
    Config,
    /// `/buy <mint> <sol> [slippage_bps]` — request a buy (never executed on
    /// the first tap: the trade desk draws an inline keyboard, the trade
    /// console issues a nonce).
    Buy {
        /// Base58 token mint.
        mint: String,
        /// SOL amount as typed by the operator.
        sol: String,
        /// Optional slippage tolerance in basis points, as typed.
        slippage_bps: Option<String>,
    },
    /// `/sell <mint> <percent|all> [slippage_bps]` — request selling a
    /// fraction of the open position.
    Sell {
        /// Base58 token mint.
        mint: String,
        /// Percentage of the open position to sell, as typed by the operator.
        pct: String,
        /// Optional slippage tolerance in basis points, as typed.
        slippage_bps: Option<String>,
    },
    /// `/snipe on|off` — shortcut for enabling/disabling the sniper module.
    Snipe(bool),
    /// `/wallets` — configured trading wallets (public addresses) + balances.
    Wallets,
    /// `/limit` — this chat's trade limits and current usage.
    Limit,
    /// `/consent grant|status|revoke` — the consent surface (BATCH 04).
    Consent(consent_cmd::ConsentAction),
    /// `/confirm <nonce>` — execute one pending command.
    Confirm(String),
    /// `/cancel <nonce>` — cancel one pending command.
    Cancel(String),
    /// `/pending` — list this chat's pending commands.
    Pending,
    /// Unrecognized command text (empty string = not addressed to us).
    Unknown(String),
}

/// Telegram command role (mirrors the API RBAC hierarchy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TgRole {
    /// Read-only commands (status, positions, trades, pnl, balance, config).
    Readonly = 0,
    /// Runtime control (on/off, kill/resume, non-live mode switches).
    Operator = 1,
    /// Everything, including switching execution mode to `live`.
    Owner = 2,
}

impl TgRole {
    /// True when this role is at least `required` in the hierarchy.
    pub fn satisfies(&self, required: TgRole) -> bool {
        (*self as u8) >= (required as u8)
    }

    /// Stable lowercase name (used in refusal messages).
    pub fn as_str(&self) -> &'static str {
        match self {
            TgRole::Readonly => "readonly",
            TgRole::Operator => "operator",
            TgRole::Owner => "owner",
        }
    }
}

/// Resolve the caller's role. `None` = not authorized at all.
///
/// Precedence: `owner_user_ids` > `readonly_user_ids` > legacy allowlists.
/// Backward compatibility: with no `owner_user_ids` configured, members of
/// `allowed_user_ids`/`allowed_chat_ids` keep FULL control (owner), exactly
/// like before roles existed. When owners ARE configured, the legacy
/// allowlists map to `operator` so a single-operator setup can grant a
/// second person control rights without handing over owner powers.
pub fn telegram_role(chat_id: i64, user_id: Option<i64>, cfg: &TelegramConfig) -> Option<TgRole> {
    if let Some(uid) = user_id {
        if cfg.owner_user_ids.contains(&uid) {
            return Some(TgRole::Owner);
        }
        if cfg.readonly_user_ids.contains(&uid) {
            return Some(TgRole::Readonly);
        }
    }
    let legacy = {
        let by_user = user_id
            .map(|uid| !cfg.allowed_user_ids.is_empty() && cfg.allowed_user_ids.contains(&uid))
            .unwrap_or(false);
        let by_chat = !cfg.allowed_chat_ids.is_empty() && cfg.allowed_chat_ids.contains(&chat_id);
        by_user || by_chat
    };
    if legacy {
        return Some(if cfg.owner_user_ids.is_empty() {
            TgRole::Owner
        } else {
            TgRole::Operator
        });
    }
    // No allowlist => deny. Secure by default; the operator must list at least
    // one chat or user id to enable remote control.
    None
}

/// Is this chat/user allowed to issue control commands?
pub fn is_authorized(chat_id: i64, user_id: Option<i64>, cfg: &TelegramConfig) -> bool {
    telegram_role(chat_id, user_id, cfg).is_some()
}

/// Minimum role a command requires.
pub fn command_requires(cmd: &Command) -> TgRole {
    match cmd {
        Command::Help
        | Command::Status
        | Command::Positions
        | Command::Trades
        | Command::Pnl
        | Command::Balance
        | Command::Config
        | Command::Wallets
        | Command::Limit => TgRole::Readonly,
        // Money-moving commands: operator or owner only, and even then the
        // trade still requires a second, nonce-bound confirmation step.
        Command::Buy { .. } | Command::Sell { .. } => TgRole::Operator,
        // BATCH 04 — granting a consent hands trading power to a chat, so it is
        // reserved for owners; revoking it (an emergency action) needs only an
        // operator; reading it is harmless.
        Command::Consent(consent_cmd::ConsentAction::Grant { .. }) => TgRole::Owner,
        Command::Consent(consent_cmd::ConsentAction::Revoke) => TgRole::Operator,
        Command::Consent(consent_cmd::ConsentAction::Status) => TgRole::Readonly,
        Command::Consent(consent_cmd::ConsentAction::Usage) => TgRole::Readonly,
        Command::Consent(consent_cmd::ConsentAction::Invalid(_)) => TgRole::Readonly,
        Command::Confirm(_) | Command::Cancel(_) => TgRole::Operator,
        Command::Pending => TgRole::Readonly,
        // Switching TO live is the one telegram action reserved for owners.
        Command::Mode(Some(m)) if m.eq_ignore_ascii_case("live") => TgRole::Owner,
        Command::Mode(_)
        | Command::On(_)
        | Command::Off(_)
        | Command::Kill
        | Command::Resume
        | Command::Snipe(_)
        | Command::Unknown(_) => TgRole::Operator,
    }
}

/// Parse raw message text into a [`Command`].
///
/// `prefix` (from config) is stripped first so several bots can share a chat.
pub fn parse_command(text: &str, prefix: &str) -> Command {
    let mut t = text.trim();
    if !prefix.trim().is_empty() {
        if let Some(stripped) = t.strip_prefix(prefix.trim()) {
            t = stripped.trim();
        } else {
            // Message is not addressed to us.
            return Command::Unknown(String::new());
        }
    }
    // Drop any @botname suffix on the command token (e.g. "/status@mybot").
    let first = t.split_whitespace().next().unwrap_or("");
    let rest = t.split_whitespace().skip(1).collect::<Vec<_>>().join(" ");
    let cmd = first.split('@').next().unwrap_or("").trim();
    let cmd = cmd.strip_prefix('/').unwrap_or(cmd).to_ascii_lowercase();

    match cmd.as_str() {
        "" => Command::Unknown(String::new()),
        "start" | "help" | "h" => Command::Help,
        "status" | "s" => Command::Status,
        "on" | "enable" | "start_module" => Command::On(parse_target(&rest)),
        "off" | "disable" | "stop_module" => Command::Off(parse_target(&rest)),
        "kill" | "halt" | "panic" | "estop" => Command::Kill,
        "resume" | "unkill" | "clear" => Command::Resume,
        "positions" | "pos" | "p" => Command::Positions,
        "trades" | "fills" => Command::Trades,
        "pnl" | "profit" => Command::Pnl,
        "balance" | "bal" => Command::Balance,
        "mode" => Command::Mode(if rest.is_empty() {
            None
        } else {
            Some(rest.to_ascii_lowercase())
        }),
        "config" | "cfg" => Command::Config,
        "buy" | "b" => {
            let mut args = rest.split_whitespace();
            Command::Buy {
                mint: args.next().unwrap_or("").to_string(),
                sol: args.next().unwrap_or("").to_string(),
                slippage_bps: args.next().map(str::to_string),
            }
        }
        "sell" => {
            let mut args = rest.split_whitespace();
            Command::Sell {
                mint: args.next().unwrap_or("").to_string(),
                pct: args.next().unwrap_or("").to_string(),
                slippage_bps: args.next().map(str::to_string),
            }
        }
        // ---------------------------------------------------------- BATCH 04 --
        // Consent-gated trading surface. Routing to it (and the fact that it
        // is only live when a TradeConsole is attached) happens in `lib.rs`.
        "consent" | "consents" => Command::Consent(consent_cmd::ConsentAction::parse(&rest)),
        "confirm" => Command::Confirm(rest.split_whitespace().next().unwrap_or("").to_string()),
        "cancel" => Command::Cancel(rest.split_whitespace().next().unwrap_or("").to_string()),
        "pending" => Command::Pending,
        "snipe" => Command::Snipe(matches!(
            rest.trim().to_ascii_lowercase().as_str(),
            "on" | "1" | "true" | "yes"
        )),
        "wallets" | "wallet" => Command::Wallets,
        "limit" | "limits" => Command::Limit,
        other => Command::Unknown(other.to_string()),
    }
}

/// Parse an `on`/`off` argument into a [`Target`].
fn parse_target(arg: &str) -> Target {
    let a = arg.trim().to_ascii_lowercase();
    match a.as_str() {
        "" | "all" | "everything" | "*" => Target::All,
        other => match other.parse::<BotModule>() {
            Ok(m) => Target::Module(m),
            Err(_) => Target::All, // unknown name => treat as all (safer default)
        },
    }
}

/// Apply a command to shared state and produce the reply text.
/// Execute a command AS the given role. Insufficient rights produce a
/// refusal string (never a silent no-op) so the operator sees WHY.
pub async fn handle(state: &Shared, command: Command, role: TgRole) -> String {
    let required = command_requires(&command);
    if !role.satisfies(required) {
        return format!(
            "⛔ Your role '{}' cannot run this command (needs '{}').",
            role.as_str(),
            required.as_str()
        );
    }
    match command {
        Command::Help => help_text(),
        Command::Status => status_text(state).await,
        Command::On(target) => set_enabled_text(state, &target, true).await,
        Command::Off(target) => set_enabled_text(state, &target, false).await,
        Command::Kill => {
            state.emergency_stop("telegram /kill").await;
            "🛑 KILL SWITCH ENGAGED — all broadcasting halted, positions flattened by risk.\nUse /resume to clear.".to_string()
        }
        Command::Resume => {
            state.set_kill_switch(false, "telegram /resume").await;
            state.clear_halt().await;
            "✅ Kill switch cleared. Modules resume per their enabled flags (/status).".to_string()
        }
        Command::Positions => positions_text(state).await,
        Command::Trades => trades_text(state).await,
        Command::Pnl => pnl_text(state).await,
        Command::Balance => balance_text(state).await,
        Command::Mode(arg) => mode_text(state, arg).await,
        Command::Config => config_text(state).await,
        Command::Snipe(on) => {
            state.set_enabled(BotModule::Sniper, on).await;
            format!(
                "{} Sniper module {} (sniping {}).",
                if on { "✅" } else { "⛔" },
                if on { "enabled" } else { "disabled" },
                if on { "ON" } else { "OFF" }
            )
        }
        // Trade commands are handled by the TradeDesk (they need the
        // session + confirmation keyboard); handle_trade_command() is the
        // entry point. Reaching them here means a caller used handle()
        // directly — refuse loudly instead of silently dropping.
        Command::Buy { .. } | Command::Sell { .. } => {
            "⚠️ Trade commands need the trade desk (handle_trade_command).".to_string()
        }
        Command::Wallets => wallets_text(state).await,
        Command::Limit => {
            "Use /limit through the trade desk to see this chat's live usage.".to_string()
        }
        // BATCH 04 — reaching these here means no TradeConsole is attached to
        // the bot (otherwise `lib.rs` routes them to the console). Refuse
        // loudly with the single owner's text instead of silently dropping.
        Command::Consent(_) | Command::Confirm(_) | Command::Cancel(_) | Command::Pending => {
            reply::not_configured().plain().to_string()
        }
        Command::Unknown(u) if u.is_empty() => String::new(),
        Command::Unknown(u) => format!("Unknown command '{u}'. Try /help."),
    }
}

fn help_text() -> String {
    [
        "🤖 Sniper Suite control",
        "",
        "/status — all modules, PnL, kill switch",
        "/on <module|all> — enable a module (sniper, copy, polymarket, contract, telegram)",
        "/off <module|all> — disable a module",
        "/kill — engage the kill switch (halt everything)",
        "/resume — clear the kill switch",
        "/positions — open positions",
        "/trades — recent fills",
        "/pnl — realized/unrealized + today",
        "/balance — wallet balances",
        "/mode [paper|simulate|live] — show or set execution mode",
        "/config — key configuration",
        "/buy <mint> <sol> — buy (confirm on the keyboard)",
        "/sell <mint> <percent> — sell a % of the position (confirm on the keyboard)",
        "/snipe on|off — toggle the sniper module",
        "/wallets — configured wallets + balances",
        "/limit — this chat's trade limits and usage",
        "/consent grant|status|revoke — consent-gated trading (BATCH 04)",
        "/confirm <nonce> — execute a pending trade command",
        "/cancel <nonce> — cancel a pending trade command",
        "/pending — list this chat's pending trade commands",
    ]
    .join("\n")
}

async fn status_text(state: &Shared) -> String {
    let s = state.summary().await;
    let mut out = String::new();
    out.push_str(&format!(
        "📊 Status — mode: {} | live_allowed: {} | kill: {}\n",
        s.execution_mode.as_str(),
        if s.live_allowed { "yes" } else { "no" },
        if s.kill_switch { "🛑 ON" } else { "off" }
    ));
    out.push_str(&format!(
        "open positions: {} | unrealized: {:.4} | realized: {:.4}\n",
        s.open_positions, s.unrealized_pnl, s.realized_pnl
    ));
    if s.daily.loss_limit_tripped {
        out.push_str("⚠️ daily loss limit TRIPPED\n");
    }
    // Reconciliation visibility (§R): backlog + per-symbol entry gate.
    // Read-only by design — no Telegram command may mutate claims (§Y).
    if !s.recon_unresolved.is_empty() {
        let total: i64 = s.recon_unresolved.iter().map(|(_, n)| *n).sum();
        let parts: Vec<String> = s
            .recon_unresolved
            .iter()
            .map(|(k, n)| format!("{k}={n}"))
            .collect();
        out.push_str(&format!(
            "⚠️ reconciliation backlog: {total} ({})\n",
            parts.join(", ")
        ));
    }
    if !s.blocked_symbols.is_empty() {
        out.push_str(&format!(
            "⛔ entry-gated symbols ({}): {}\n",
            s.blocked_symbols.len(),
            s.blocked_symbols.join(", ")
        ));
    }
    out.push('\n');
    for m in &s.modules {
        out.push_str(&format!(
            "{} {:<11} enabled:{} running:{} healthy:{} | sig {} ord {} fill {} fail {} rj {} | pnl {:.4}\n",
            m.module.emoji(),
            m.module.as_str(),
            yn(m.enabled),
            yn(m.running),
            yn(m.healthy),
            m.signals_generated,
            m.orders_sent,
            m.orders_filled,
            m.orders_failed,
            m.orders_rejected_by_risk,
            m.realized_pnl,
        ));
        if let Some(err) = &m.last_error {
            out.push_str(&format!("    ⚠️ {err}\n"));
        }
    }
    out
}

async fn positions_text(state: &Shared) -> String {
    let positions = state.open_positions().await;
    if positions.is_empty() {
        return "No open positions.".to_string();
    }
    let mut out = format!("📈 {} open position(s)\n\n", positions.len());
    for p in positions {
        out.push_str(&format!(
            "[{}] {} | qty {:.4} @ {:.6} | mark {:.6} | uPnL {:.4} ({:.1}%)\n",
            p.source,
            p.symbol_display,
            p.qty,
            p.avg_entry,
            p.last_mark,
            p.unrealised(),
            p.total_pnl_pct() * 100.0,
        ));
    }
    out
}

async fn trades_text(state: &Shared) -> String {
    let trades = state.trades(10).await;
    if trades.is_empty() {
        return "No trades yet.".to_string();
    }
    let mut out = String::from("🧾 Recent fills\n\n");
    for t in trades {
        let side = match t.side {
            bot_core::models::PositionSide::Long => "buy",
            bot_core::models::PositionSide::Short => "sell",
        };
        out.push_str(&format!(
            "{} {} {} {} | in {:.4} out {:.4} @ {:.6} {}\n",
            t.ts.format("%H:%M:%S"),
            t.source,
            side,
            t.symbol_display,
            t.amount_in,
            t.amount_out,
            t.price,
            t.quote_symbol,
        ));
    }
    out
}

async fn pnl_text(state: &Shared) -> String {
    let s = state.summary().await;
    format!(
        "💰 PnL\nrealized: {:.4}\nunrealized: {:.4}\ntoday: realized {:.4} | buys {} sells {} | W{} L{}\n{}",
        s.realized_pnl,
        s.unrealized_pnl,
        s.daily.realized_pnl,
        s.daily.buys,
        s.daily.sells,
        s.daily.wins,
        s.daily.losses,
        if s.daily.loss_limit_tripped { "⚠️ loss limit tripped" } else { "" },
    )
}

async fn balance_text(state: &Shared) -> String {
    let b = state.balances().await;
    format!(
        "👛 Balances\nSOL: {:.4}\nUSDC (Polygon): {:.4}\n{}",
        b.sol,
        b.usdc_polygon,
        b.checked_at
            .map(|t| format!("checked {}", t.format("%H:%M:%S")))
            .unwrap_or_else(|| "not checked".into()),
    )
}

async fn mode_text(state: &Shared, arg: Option<String>) -> String {
    let current = state.execution_mode().await;
    let Some(arg) = arg else {
        return format!(
            "Execution mode: {} (live_allowed: {}). Set with /mode paper|simulate|live.",
            current.as_str(),
            state.summary().await.live_allowed
        );
    };
    let new_mode = match arg.as_str() {
        "paper" => ExecutionMode::Paper,
        "simulate" | "sim" => ExecutionMode::Simulate,
        "live" => ExecutionMode::Live,
        other => return format!("Unknown mode '{other}'. Use paper|simulate|live."),
    };
    // Live additionally requires the allow_live_trading gate to actually send.
    state.update_config(|c| c.execution.mode = new_mode).await;
    let gate = state.summary().await.live_allowed;
    if new_mode == ExecutionMode::Live && !gate {
        "⚠️ Mode set to LIVE but execution.allow_live_trading is false — orders will simulate, not broadcast.".to_string()
    } else {
        format!("Execution mode set to {}.", new_mode.as_str())
    }
}

async fn config_text(state: &Shared) -> String {
    let cfg = state.config_snapshot().await;
    format!(
        "⚙️ Config\nmode: {} | live_allowed: {}\nsniper: buy {:.4} SOL, slip {:.1}%, feeds pp:{} logs:{}\ncopy: {} wallet(s), feed {}\npolymarket: strategy {}, stake ${:.2}, min_edge {:.3}\nrisk: max_pos {:.4} SOL, reserve {:.4}, daily_loss {:.2}, max_open {}",
        cfg.execution.mode.as_str(),
        cfg.execution.allow_live_trading,
        cfg.sniper.buy_sol,
        cfg.sniper.slippage_pct,
        yn(cfg.sniper.use_pumpportal),
        yn(cfg.sniper.use_log_subscription),
        cfg.copy.wallets.len(),
        cfg.copy.feed,
        cfg.polymarket.strategy,
        cfg.polymarket.stake_usd,
        cfg.polymarket.min_edge,
        cfg.risk.max_position_quote,
        cfg.risk.min_sol_reserve,
        cfg.risk.daily_loss_limit_quote,
        cfg.risk.max_open_positions,
    )
}

async fn set_enabled_text(state: &Shared, target: &Target, enabled: bool) -> String {
    let verb = if enabled { "enabled" } else { "disabled" };
    match target {
        Target::All => {
            let mut done = Vec::new();
            for m in BotModule::TRADING.iter() {
                state.set_enabled(*m, enabled).await;
                done.push(m.as_str());
            }
            format!(
                "{} all trading modules: {}",
                if enabled { "✅" } else { "⛔" },
                done.join(", ")
            ) + &format!(" ({verb})")
        }
        Target::Module(m) => {
            state.set_enabled(*m, enabled).await;
            format!(
                "{} {} {} {verb}",
                if enabled { "✅" } else { "⛔" },
                m.emoji(),
                m.as_str()
            )
        }
    }
}

async fn wallets_text(state: &Shared) -> String {
    let cfg = state.config_snapshot().await;
    let b = state.balances().await;
    let mut out = String::from("👛 Wallets\n");
    if cfg.copy.wallets.is_empty() {
        out.push_str("No copy-trade wallets configured.\n");
    } else {
        for w in &cfg.copy.wallets {
            out.push_str(&format!(
                "• {}{}\n",
                w.address,
                w.label
                    .as_ref()
                    .map(|l| format!(" ({l})"))
                    .unwrap_or_default()
            ));
        }
    }
    out.push_str(&format!("SOL balance: {:.4}\n", b.sol));
    out
}

fn yn(b: bool) -> &'static str {
    if b {
        "y"
    } else {
        "n"
    }
}

// ---------------------------------------------------------------------------
// Trade desk (GAP-MAP v2 P2): /buy and /sell behind confirmation keyboards
// ---------------------------------------------------------------------------

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use crate::callbacks::{CallbackAction, CallbackSecret};
use crate::trade_session::{
    ChatBinding, ChatLimits, TradeExecutor, TradeIntent, TradeSession, TradeSide,
    UnconfiguredTradeExecutor,
};

/// A reply to a trade command: text plus an optional inline keyboard.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeReply {
    /// Message text (always present).
    pub text: String,
    /// reply_markup JSON when the reply needs Confirm/Cancel buttons.
    pub keyboard: Option<String>,
}

impl TradeReply {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            keyboard: None,
        }
    }
}

/// The trade desk: binds the trade session, the callback secret and the
/// executor into one dispatch surface. One per bot process.
pub struct TradeDesk {
    state: Shared,
    session: Mutex<TradeSession>,
    secret: CallbackSecret,
    executor: Arc<dyn TradeExecutor>,
}

impl TradeDesk {
    /// Build a desk. With no executor wired, confirmations fail closed via
    /// [`UnconfiguredTradeExecutor`].
    pub fn new(
        state: Shared,
        limits: ChatLimits,
        secret: CallbackSecret,
    ) -> bot_core::error::BotResult<Self> {
        Ok(Self {
            state,
            session: Mutex::new(TradeSession::new(limits)?),
            secret,
            executor: Arc::new(UnconfiguredTradeExecutor),
        })
    }

    /// Attach the executor that runs confirmed trades (server wires the
    /// tenant-bound implementation).
    pub fn set_executor(&mut self, executor: Arc<dyn TradeExecutor>) {
        self.executor = executor;
    }

    /// Bind a chat to a tenant (server calls from the Telegram-link flow).
    pub async fn bind_chat(&self, chat_id: i64, binding: ChatBinding) {
        self.session.lock().await.bind(chat_id, binding);
    }

    /// Drop expired pending confirmations (poll-tick housekeeping).
    pub async fn prune(&self, now: DateTime<Utc>) {
        self.session.lock().await.prune_expired(now);
    }

    /// `/buy <mint> <sol>` — validate, and either reply with the
    /// confirmation keyboard or refuse with the explicit reason.
    pub async fn request_buy(
        &self,
        chat_id: i64,
        user_id: i64,
        mint: &str,
        sol_raw: &str,
        now: DateTime<Utc>,
    ) -> TradeReply {
        if mint.trim().is_empty() || sol_raw.trim().is_empty() {
            return TradeReply::plain("Usage: /buy <mint> <sol> — e.g. /buy So11…112 0.25");
        }
        let amount_sol: f64 = match sol_raw.trim().parse() {
            Ok(v) => v,
            Err(_) => return TradeReply::plain("⛔ SOL amount must be a number, e.g. 0.25"),
        };
        let intent = TradeIntent {
            chat_id,
            user_id,
            organization_id: String::new(),
            side: TradeSide::Buy,
            mint: mint.trim().to_string(),
            amount_sol,
            sell_fraction: 0.0,
        };
        self.prepare(chat_id, intent, now).await
    }

    /// `/sell <mint> <percent>` — same flow for sells.
    pub async fn request_sell(
        &self,
        chat_id: i64,
        user_id: i64,
        mint: &str,
        pct_raw: &str,
        now: DateTime<Utc>,
    ) -> TradeReply {
        if mint.trim().is_empty() || pct_raw.trim().is_empty() {
            return TradeReply::plain("Usage: /sell <mint> <percent> — e.g. /sell So11…112 50");
        }
        let pct: f64 = match pct_raw.trim().parse() {
            Ok(v) => v,
            Err(_) => return TradeReply::plain("⛔ Percent must be a number between 1 and 100"),
        };
        let intent = TradeIntent {
            chat_id,
            user_id,
            organization_id: String::new(),
            side: TradeSide::Sell,
            mint: mint.trim().to_string(),
            amount_sol: 0.0,
            sell_fraction: pct / 100.0,
        };
        self.prepare(chat_id, intent, now).await
    }

    async fn prepare(&self, chat_id: i64, intent: TradeIntent, now: DateTime<Utc>) -> TradeReply {
        // Kill switch first: never even DRAW a money keyboard while it's on.
        if self.state.kill_switch() {
            return TradeReply::plain(
                "🛑 Kill switch is ENGAGED — trading commands are refused until /resume.",
            );
        }
        let mut session = self.session.lock().await;
        match session.prepare_confirmation(intent.clone(), now) {
            Ok(pending) => {
                let summary = match intent.side {
                    TradeSide::Buy => format!(
                        "BUY {:.4} SOL of {}\nTap to confirm — this expires in {}s.",
                        intent.amount_sol,
                        intent.mint,
                        session.limits().confirm_ttl_secs
                    ),
                    TradeSide::Sell => format!(
                        "SELL {:.0}% of the {} position\nTap to confirm — this expires in {}s.",
                        intent.sell_fraction * 100.0,
                        intent.mint,
                        session.limits().confirm_ttl_secs
                    ),
                };
                let _ = chat_id; // keyboard attaches to the calling chat
                TradeReply {
                    text: summary,
                    keyboard: Some(self.secret.confirm_keyboard(&pending.nonce)),
                }
            }
            Err(deny) => TradeReply::plain(deny.reply_text()),
        }
    }

    /// Resolve an inline-keyboard tap: verify the signature, enforce the
    /// requester/TTL/limit walls, then run the executor on confirm.
    /// Returns `(toast, follow-up reply)`.
    pub async fn resolve_callback(
        &self,
        data: &str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> (String, Option<TradeReply>) {
        let action = match self.secret.verify(data) {
            Ok(a) => a,
            Err(deny) => return (deny.toast().to_string(), None),
        };
        let nonce = action.nonce().to_string();
        let mut session = self.session.lock().await;
        match action {
            CallbackAction::Cancel { .. } => match session.cancel(&nonce, user_id) {
                Ok(()) => ("Trade cancelled.".to_string(), None),
                Err(deny) => (deny.reply_text().to_string(), None),
            },
            CallbackAction::Confirm { .. } => match session.confirm(&nonce, user_id, now) {
                Ok(intent) => {
                    // Drop the lock across the executor call (it may await
                    // the pipeline); the nonce is already resolved, so a
                    // concurrent tap cannot double-execute.
                    drop(session);
                    if self.state.kill_switch() {
                        return (
                            "Kill switch engaged".to_string(),
                            Some(TradeReply::plain(
                                "🛑 Kill switch engaged AFTER confirmation — the trade was NOT sent.",
                            )),
                        );
                    }
                    match self.executor.execute(&intent).await {
                        Ok(summary) => (
                            "Confirmed".to_string(),
                            Some(TradeReply::plain(format!("✅ {summary}"))),
                        ),
                        Err(e) => (
                            "Rejected".to_string(),
                            Some(TradeReply::plain(format!("⛔ Trade rejected: {e}"))),
                        ),
                    }
                }
                Err(deny) => (deny.reply_text().to_string(), None),
            },
        }
    }

    /// `/limit` — this chat's limits and live usage.
    pub async fn limits_text(&self, chat_id: i64, now: DateTime<Utc>) -> TradeReply {
        let session = self.session.lock().await;
        let l = session.limits();
        TradeReply::plain(format!(
            "🧮 Trade limits for this chat\n             • {} trades per {}s (used: {})\n             • max {:.4} SOL per trade\n             • max {:.4} SOL per day (used today: {:.4})\n             • confirmations expire after {}s",
            l.max_trades_per_window,
            l.window_secs,
            session.trades_in_window(chat_id, now),
            l.max_sol_per_trade,
            l.max_daily_sol,
            session.sol_today(chat_id, now),
            l.confirm_ttl_secs,
        ))
    }
}

/// Dispatch a trade-related command through the desk. Returns `None` when
/// the command is not trade-related (the caller falls back to [`handle`]).
pub async fn handle_trade_command(
    desk: &TradeDesk,
    state: &Shared,
    command: Command,
    role: TgRole,
    chat_id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> Option<TradeReply> {
    let required = command_requires(&command);
    if !role.satisfies(required) {
        return Some(TradeReply::plain(format!(
            "⛔ Your role '{}' cannot run this command (needs '{}').",
            role.as_str(),
            required.as_str()
        )));
    }
    match command {
        Command::Buy {
            mint,
            sol,
            slippage_bps: _,
        } => Some(desk.request_buy(chat_id, user_id, &mint, &sol, now).await),
        Command::Sell {
            mint,
            pct,
            slippage_bps: _,
        } => Some(desk.request_sell(chat_id, user_id, &mint, &pct, now).await),
        Command::Limit => Some(desk.limits_text(chat_id, now).await),
        Command::Wallets => Some(TradeReply::plain(wallets_text(state).await)),
        Command::Snipe(on) => Some(TradeReply::plain(
            handle(state, Command::Snipe(on), role).await,
        )),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// BATCH 04 — consent-gated trade console (router + shared flow)
// ---------------------------------------------------------------------------

/// A chat ↔ tenant ↔ user link.
///
/// A Telegram chat id is **not** an identity: the console only trades for a
/// chat that the control plane has linked to an organization *and* whose linked
/// user matches the Telegram user issuing the command. Both halves are checked
/// on every command (see [`TradeConsole::resolve_link`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLink {
    /// Tenant that owns this chat.
    pub organization_id: String,
    /// The Telegram user the chat is linked to.
    pub user_id: i64,
    /// `false` for alert-only links: they may read `/consent status` but never
    /// trade and never grant.
    pub trading_enabled: bool,
}

impl ChatLink {
    /// Build a trading-enabled link.
    pub fn trading(organization_id: impl Into<String>, user_id: i64) -> Self {
        Self {
            organization_id: organization_id.into(),
            user_id,
            trading_enabled: true,
        }
    }

    /// Build an alert-only link.
    pub fn alerts_only(organization_id: impl Into<String>, user_id: i64) -> Self {
        Self {
            organization_id: organization_id.into(),
            user_id,
            trading_enabled: false,
        }
    }
}

/// Why a chat/user could not be resolved to a tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDeny {
    /// The chat is not linked to any organization.
    Unlinked,
    /// The chat is linked, but to a different Telegram user.
    WrongUser,
    /// The link exists but trading is disabled for it.
    TradingDisabled,
}

impl LinkDeny {
    /// Stable machine code (audit `reason`).
    pub fn code(&self) -> &'static str {
        match self {
            LinkDeny::Unlinked => "unlinked_chat",
            LinkDeny::WrongUser => "chat_user_mismatch",
            LinkDeny::TradingDisabled => "trading_disabled",
        }
    }

    /// The operator-facing explanation.
    pub fn explain(&self) -> &'static str {
        match self {
            LinkDeny::Unlinked => {
                "this chat is not linked to an organization — link it from the control plane first"
            }
            LinkDeny::WrongUser => "this chat is linked to a different Telegram user",
            LinkDeny::TradingDisabled => "trading is disabled for this chat's link (alerts only)",
        }
    }
}

/// Concrete handles to the in-memory stores, kept alongside the trait objects
/// so tests and operator diagnostics can inspect them without downcasting.
#[derive(Clone)]
pub struct MemoryStores {
    /// The in-memory consent store.
    pub consents: Arc<InMemoryConsentStore>,
    /// The in-memory command store.
    pub commands: Arc<InMemoryCommandStore>,
    /// The in-memory audit store.
    pub audit: Arc<InMemoryAuditStore>,
}

/// The three stores the console needs, bundled so wiring stays one call.
#[derive(Clone)]
pub struct TradeStores {
    /// Consent rows (migration 0066).
    pub consents: Arc<dyn ConsentStore>,
    /// Command rows (migration 0067).
    pub commands: Arc<dyn CommandStore>,
    /// Audit rows (migration 0067).
    pub audit: Arc<dyn AuditStore>,
    memory: Option<MemoryStores>,
}

impl TradeStores {
    /// Process-local stores — the deployment default until Batch 07 wires
    /// Postgres, and what every flow test uses.
    pub fn in_memory() -> Self {
        let consents = Arc::new(InMemoryConsentStore::new());
        let commands = Arc::new(InMemoryCommandStore::new());
        let audit = Arc::new(InMemoryAuditStore::new());
        Self {
            consents: consents.clone(),
            commands: commands.clone(),
            audit: audit.clone(),
            memory: Some(MemoryStores {
                consents,
                commands,
                audit,
            }),
        }
    }

    /// Fully specified bundle (used by the server's Postgres wiring).
    pub fn new(
        consents: Arc<dyn ConsentStore>,
        commands: Arc<dyn CommandStore>,
        audit: Arc<dyn AuditStore>,
    ) -> Self {
        Self {
            consents,
            commands,
            audit,
            memory: None,
        }
    }

    /// The concrete in-memory handles, when this bundle is process-local.
    pub fn memory(&self) -> Option<&MemoryStores> {
        self.memory.as_ref()
    }
}

/// Tunables the console validates against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TradeConsoleConfig {
    /// Consent defaults and hard bounds.
    pub consent_bounds: ConsentBounds,
    /// Slippage window, rate limit and cap arithmetic.
    pub limits: TradeLimits,
}

/// The raw, **unvalidated** trade request as typed (`/buy` or `/sell`).
///
/// Validation happens after the consent gate so an unauthorized chat cannot
/// probe the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeRequestRaw {
    /// `/buy <mint> <amount_sol> [slippage_bps]`.
    Buy {
        /// Mint as typed.
        mint: String,
        /// Amount as typed.
        sol: String,
        /// Slippage as typed.
        slippage: Option<String>,
    },
    /// `/sell <mint> <percent|all> [slippage_bps]`.
    Sell {
        /// Mint as typed.
        mint: String,
        /// Percentage as typed.
        percent: String,
        /// Slippage as typed.
        slippage: Option<String>,
    },
}

impl TradeRequestRaw {
    /// The action this request performs.
    pub fn action(&self) -> TradeAction {
        match self {
            TradeRequestRaw::Buy { .. } => TradeAction::Buy,
            TradeRequestRaw::Sell { .. } => TradeAction::Sell,
        }
    }
}

/// Is this command owned by the trade console (as opposed to the pre-existing
/// control commands)?
pub fn is_console_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Buy { .. }
            | Command::Sell { .. }
            | Command::Consent(_)
            | Command::Confirm(_)
            | Command::Cancel(_)
            | Command::Pending
    )
}

/// The consent-gated trading surface.
///
/// One per bot process. Attached to [`crate::TelegramBot`] with
/// `with_trade_console`; when it is absent the new commands answer "not
/// configured" and every pre-existing command behaves exactly as before.
pub struct TradeConsole {
    state: Shared,
    port: Arc<dyn TradeCommandPort>,
    consent_store: Arc<dyn ConsentStore>,
    command_store: Arc<dyn CommandStore>,
    audit_store: Arc<dyn AuditStore>,
    consent_bounds: ConsentBounds,
    limits: TradeLimits,
    rate: RateLimiter,
    links: tokio::sync::RwLock<HashMap<i64, ChatLink>>,
}

impl std::fmt::Debug for TradeConsole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TradeConsole")
            .field("consent_bounds", &self.consent_bounds)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl TradeConsole {
    /// Build a console. Fails closed on an incoherent limit configuration.
    pub fn new(
        state: Shared,
        port: Arc<dyn TradeCommandPort>,
        stores: TradeStores,
        config: TradeConsoleConfig,
    ) -> Result<Self, LimitDeny> {
        config.limits.validate()?;
        config.consent_bounds.validate().map_err(|e| {
            LimitDeny::InvalidConfig(format!("consent bounds rejected: {}", e.code()))
        })?;
        let rate = RateLimiter::new(config.limits.rate)?;
        Ok(Self {
            state,
            port,
            consent_store: stores.consents,
            command_store: stores.commands,
            audit_store: stores.audit,
            consent_bounds: config.consent_bounds,
            limits: config.limits,
            rate,
            links: tokio::sync::RwLock::new(HashMap::new()),
        })
    }

    /// The parse mode every console reply must be sent with.
    pub fn parse_mode(&self) -> &'static str {
        reply::PARSE_MODE
    }

    /// The limits in force (for tests and operator diagnostics).
    pub fn limits(&self) -> &TradeLimits {
        &self.limits
    }

    /// The consent bounds in force.
    pub fn consent_bounds(&self) -> &ConsentBounds {
        &self.consent_bounds
    }

    /// Bind a chat to a tenant (the control plane calls this from its link
    /// flow; tests call it directly).
    pub async fn link_chat(&self, chat_id: i64, link: ChatLink) {
        self.links.write().await.insert(chat_id, link);
    }

    /// Remove a link. Pending commands of that chat are cancelled: an unlinked
    /// chat must not keep a live money button.
    pub async fn unlink_chat(&self, chat_id: i64, now: DateTime<Utc>) -> Vec<Uuid> {
        let removed = self.links.write().await.remove(&chat_id);
        let Some(link) = removed else {
            return Vec::new();
        };
        self.command_store
            .cancel_pending(&link.organization_id, chat_id, now)
            .await
            .unwrap_or_default()
    }

    /// The link for `(chat, user)`, or the reason it cannot be used.
    pub async fn resolve_link(&self, chat_id: i64, user_id: i64) -> Result<ChatLink, LinkDeny> {
        let links = self.links.read().await;
        let Some(link) = links.get(&chat_id) else {
            return Err(LinkDeny::Unlinked);
        };
        if link.user_id != user_id {
            return Err(LinkDeny::WrongUser);
        }
        if !link.trading_enabled {
            return Err(LinkDeny::TradingDisabled);
        }
        Ok(link.clone())
    }

    /// Route one command. `None` means "not a console command" and the caller
    /// must fall through to its own handler.
    pub async fn handle(
        &self,
        command: Command,
        role: TgRole,
        chat_id: i64,
        user_id: Option<i64>,
        now: DateTime<Utc>,
    ) -> Option<Reply> {
        if !is_console_command(&command) {
            return None;
        }
        let required = command_requires(&command);
        if !role.satisfies(required) {
            return Some(reply::denied_with_hint(
                "insufficient_role",
                &format!(
                    "your role '{}' cannot run this command (needs '{}')",
                    role.as_str(),
                    required.as_str()
                ),
                "Ask an owner to run it.",
            ));
        }
        Some(match command {
            Command::Buy {
                mint,
                sol,
                slippage_bps,
            } => {
                self.handle_buy(chat_id, user_id, &mint, &sol, slippage_bps.as_deref(), now)
                    .await
            }
            Command::Sell {
                mint,
                pct,
                slippage_bps,
            } => {
                self.handle_sell(chat_id, user_id, &mint, &pct, slippage_bps.as_deref(), now)
                    .await
            }
            Command::Consent(action) => self.handle_consent(action, chat_id, user_id, now).await,
            Command::Confirm(nonce) => self.handle_confirm(chat_id, user_id, &nonce, now).await,
            Command::Cancel(nonce) => self.handle_cancel(chat_id, user_id, &nonce, now).await,
            Command::Pending => self.handle_pending(chat_id, user_id, now).await,
            // `is_console_command` guarantees this is unreachable.
            // Unreachable: `is_console_command` gates the match above.
            _ => reply::not_configured(),
        })
    }

    // ------------------------------------------------------------ the flow --

    /// `/buy` flow, step by step (see `commands/buy.rs` for the grammar).
    pub(crate) async fn submit_buy(
        &self,
        chat_id: i64,
        user_id: i64,
        mint: &str,
        sol: &str,
        slippage: Option<&str>,
        now: DateTime<Utc>,
    ) -> Reply {
        self.submit(
            chat_id,
            user_id,
            TradeRequestRaw::Buy {
                mint: mint.to_string(),
                sol: sol.to_string(),
                slippage: slippage.map(str::to_string),
            },
            now,
        )
        .await
    }

    /// `/sell` flow.
    pub(crate) async fn submit_sell(
        &self,
        chat_id: i64,
        user_id: i64,
        mint: &str,
        percent: &str,
        slippage: Option<&str>,
        now: DateTime<Utc>,
    ) -> Reply {
        self.submit(
            chat_id,
            user_id,
            TradeRequestRaw::Sell {
                mint: mint.to_string(),
                percent: percent.to_string(),
                slippage: slippage.map(str::to_string),
            },
            now,
        )
        .await
    }

    /// The shared request pipeline: link → consent → validate → caps → mode →
    /// pending + nonce → audit → reply. Every branch that refuses writes one
    /// `denied` audit row.
    async fn submit(
        &self,
        chat_id: i64,
        user_id: i64,
        request: TradeRequestRaw,
        now: DateTime<Utc>,
    ) -> Reply {
        let action = request.action();
        // 1. chat → linked user → organization.
        let link = match self.resolve_link(chat_id, user_id).await {
            Ok(link) => link,
            Err(deny) => return self.deny_link(chat_id, None, deny, now).await,
        };
        let org = link.organization_id.clone();

        // Housekeeping first, so a stale row can never be confirmed later.
        let _ = self.command_store.expire_overdue(now).await;

        // 5a. kill switch (cheap, local) before anything else expensive.
        if self.state.kill_switch() {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "halted",
                    "the kill switch is engaged",
                    now,
                )
                .await;
        }
        // 5b. mode gates: paper/simulate are fine, live needs BOTH gates.
        let mode = match self.port.current_mode().await {
            Ok(mode) => mode,
            Err(err) => {
                return self
                    .deny(Some(&org), chat_id, err.code(), err.detail(), now)
                    .await
            }
        };
        if mode.live_but_gated() {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "live_gates_closed",
                    "execution.mode is LIVE but allow_live_trading is false",
                    now,
                )
                .await;
        }
        if self.port.is_halted().await {
            return self
                .deny(Some(&org), chat_id, "halted", "the executor is halted", now)
                .await;
        }

        // 2. an ACTIVE consent covering this action.
        let consent = match self.consent_store.active(&org, chat_id, now).await {
            Ok(consent) => consent,
            Err(err) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        "consent_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        let consent = match crate::consent::policy::require_active(consent.as_ref(), action, now) {
            Ok(consent) => consent.clone(),
            Err(err) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        err.code(),
                        &consent_refusal_hint(&err),
                        now,
                    )
                    .await
            }
        };

        // 3. validate the typed input (never clamp, never guess).
        let (mint, amount_lamports, percent_bps, slippage_bps) = match &request {
            TradeRequestRaw::Buy {
                mint,
                sol,
                slippage,
            } => match buy::parse_buy_args(mint, sol, slippage.as_deref(), &self.limits) {
                Ok(order) => (
                    order.mint,
                    Some(order.amount_lamports),
                    None,
                    order.slippage_bps,
                ),
                Err(err) => {
                    return self
                        .deny(Some(&org), chat_id, err.code(), &err.message(), now)
                        .await
                }
            },
            TradeRequestRaw::Sell {
                mint,
                percent,
                slippage,
            } => match sell::parse_sell_args(mint, percent, slippage.as_deref(), &self.limits) {
                Ok(order) => (
                    order.mint,
                    None,
                    Some(order.percent_bps),
                    order.slippage_bps,
                ),
                Err(err) => {
                    return self
                        .deny(Some(&org), chat_id, err.code(), &err.message(), now)
                        .await
                }
            },
        };

        // 4. caps: min(consent, org) per order, plus the rolling 24h cap.
        let org_limits = match self.port.org_risk_limits(&org).await {
            Ok(limits) => limits,
            Err(err) => {
                return self
                    .deny(Some(&org), chat_id, err.code(), err.detail(), now)
                    .await
            }
        };
        let (order_cap, daily_cap) = limits::applicable_caps(&consent, &org_limits);
        if let Some(amount) = amount_lamports {
            if let Err(err) = limits::check_order_cap(amount, order_cap) {
                return self
                    .deny_limit(
                        Some(&org),
                        chat_id,
                        &err,
                        amount,
                        order_cap,
                        daily_cap,
                        0,
                        now,
                    )
                    .await;
            }
            let window_start = limits::rolling_window_start(now, limits::ROLLING_CAP_WINDOW_SECS);
            let spent = match self
                .command_store
                .spent_since(&org, chat_id, window_start)
                .await
            {
                Ok(spent) => spent,
                Err(err) => {
                    return self
                        .deny(
                            Some(&org),
                            chat_id,
                            "command_store_unavailable",
                            err.detail(),
                            now,
                        )
                        .await
                }
            };
            if let Err(err) = limits::check_daily_cap(spent, amount, daily_cap) {
                return self
                    .deny_limit(
                        Some(&org),
                        chat_id,
                        &err,
                        amount,
                        order_cap,
                        daily_cap,
                        spent,
                        now,
                    )
                    .await;
            }
        }

        // 4b. rate limit: in-memory token bucket AND the durable ledger count.
        let rate_window = limits::rolling_window_start(now, self.limits.rate.window_secs);
        let durable = match self
            .command_store
            .count_since(&org, chat_id, rate_window)
            .await
        {
            Ok(count) => count,
            Err(err) => {
                return self
                    .deny(
                        Some(&org),
                        chat_id,
                        "command_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        if let Err(err) = self.rate.check(&org, chat_id, now, durable) {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    err.code(),
                    "rate limit reached for this chat",
                    now,
                )
                .await;
        }

        // 6. create the pending command with a fresh CSPRNG nonce.
        let spec = pending::PendingSpec {
            organization_id: org.clone(),
            user_id,
            chat_id,
            action,
            mint: mint.clone(),
            amount_lamports,
            percent_bps,
            slippage_bps,
        };
        let (row, nonce) = pending::TradeCommand::open(spec, now);
        // The audit row is written BEFORE the command becomes confirmable: a
        // command that cannot be audited must not exist.
        if let Err(err) = self
            .audit(
                Some(row.command_id),
                Some(&org),
                chat_id,
                AuditEvent::Requested,
                AuditDetail::new()
                    .action(action)
                    .mint(&mint)
                    .lamports(AuditAmount::AmountLamports, row.amount_lamports_or_zero())
                    .bps(AuditBps::PercentBps, row.percent_bps_or_zero())
                    .bps(AuditBps::SlippageBps, slippage_bps)
                    .lamports(AuditAmount::OrderCapLamports, order_cap)
                    .lamports(AuditAmount::DailyCapLamports, daily_cap)
                    .mode(mode.mode.as_str()),
                now,
            )
            .await
        {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "audit_store_unavailable",
                    err.detail(),
                    now,
                )
                .await;
        }
        if let Err(err) = self.command_store.insert(&row).await {
            return self
                .deny(
                    Some(&org),
                    chat_id,
                    "command_store_unavailable",
                    err.detail(),
                    now,
                )
                .await;
        }

        // 7. hand the one-time nonce to the chat.
        reply::pending_created(
            action,
            &mint,
            amount_lamports,
            percent_bps,
            slippage_bps,
            nonce.expose(),
            pending::COMMAND_TTL_SECS,
            mode.mode.as_str(),
        )
    }

    // ------------------------------------------------------------- helpers --

    /// Append one audit row.
    pub(crate) async fn audit(
        &self,
        command_id: Option<Uuid>,
        organization_id: Option<&str>,
        chat_id: i64,
        event: AuditEvent,
        detail: AuditDetail,
        now: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        let row = TradeAuditEvent::new(
            command_id,
            organization_id.unwrap_or_default(),
            chat_id,
            event,
            detail,
            now,
        );
        self.audit_store.append(&row).await
    }

    /// Audit a denial (best effort: a denial must be delivered even when the
    /// ledger is down) and answer the chat.
    pub(crate) async fn deny(
        &self,
        organization_id: Option<&str>,
        chat_id: i64,
        code: &str,
        explanation: &str,
        now: DateTime<Utc>,
    ) -> Reply {
        if let Err(err) = self
            .audit(
                None,
                organization_id,
                chat_id,
                AuditEvent::Denied,
                AuditDetail::new().reason(code).failure_detail(explanation),
                now,
            )
            .await
        {
            tracing::warn!(
                chat_id,
                code,
                error = %err.detail(),
                "telegram trade denial could not be audited"
            );
        }
        reply::denied(code, explanation)
    }

    /// Audit and answer a link refusal (no tenant is known, so the row carries
    /// an empty organization: it is a platform-level record).
    pub(crate) async fn deny_link(
        &self,
        chat_id: i64,
        organization_id: Option<&str>,
        deny: LinkDeny,
        now: DateTime<Utc>,
    ) -> Reply {
        self.deny(organization_id, chat_id, deny.code(), deny.explain(), now)
            .await
    }

    /// Audit and answer an unlinked / sender-less message.
    pub(crate) async fn deny_unlinked(
        &self,
        chat_id: i64,
        code: &str,
        explanation: &str,
        now: DateTime<Utc>,
    ) -> Reply {
        self.deny(None, chat_id, code, explanation, now).await
    }

    /// Audit and answer a limit refusal, with the numbers that mattered.
    #[allow(clippy::too_many_arguments)]
    async fn deny_limit(
        &self,
        organization_id: Option<&str>,
        chat_id: i64,
        denial: &LimitDeny,
        amount_lamports: u64,
        order_cap: u64,
        daily_cap: u64,
        spent: u64,
        now: DateTime<Utc>,
    ) -> Reply {
        let explanation = match denial {
            LimitDeny::OrderCapExceeded { cap_lamports, .. } => format!(
                "that amount exceeds the per-order cap of {} SOL",
                reply::format_sol(*cap_lamports)
            ),
            LimitDeny::DailyCapExceeded { cap_lamports, .. } => format!(
                "the rolling 24h cap of {} SOL is exhausted (spent {} SOL)",
                reply::format_sol(*cap_lamports),
                reply::format_sol(spent)
            ),
            LimitDeny::CapOverflow => "the amount is too large to account for".to_string(),
            LimitDeny::RateLimited {
                capacity,
                window_secs,
            } => {
                format!("at most {capacity} commands per {window_secs}s per chat")
            }
            LimitDeny::InvalidConfig(detail) => detail.clone(),
        };
        if let Err(err) = self
            .audit(
                None,
                organization_id,
                chat_id,
                AuditEvent::Denied,
                AuditDetail::new()
                    .reason(denial.code())
                    .lamports(AuditAmount::AmountLamports, amount_lamports)
                    .lamports(AuditAmount::OrderCapLamports, order_cap)
                    .lamports(AuditAmount::DailyCapLamports, daily_cap)
                    .lamports(AuditAmount::SpentLamports, spent),
                now,
            )
            .await
        {
            tracing::warn!(
                chat_id,
                code = denial.code(),
                error = %err.detail(),
                "telegram trade limit denial could not be audited"
            );
        }
        reply::denied(denial.code(), &explanation)
    }
}

/// Short, secret-free explanation for a consent refusal.
fn consent_refusal_hint(error: &ConsentError) -> String {
    match error {
        ConsentError::Missing => format!(
            "no active consent for this chat — grant one with /consent grant ... {}",
            crate::consent::policy::CONSENT_PHRASE
        ),
        ConsentError::Expired => {
            "the consent expired — grant a new one with /consent grant".to_string()
        }
        ConsentError::Revoked => {
            "the consent was revoked — grant a new one with /consent grant".to_string()
        }
        ConsentError::ScopeDenied(action) => {
            format!("the consent does not cover {}", action.as_str())
        }
        other => other.code().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TelegramConfig {
        TelegramConfig {
            allowed_chat_ids: vec![111],
            allowed_user_ids: vec![222],
            ..Default::default()
        }
    }

    #[test]
    fn auth_by_chat() {
        assert!(is_authorized(111, None, &cfg()));
        assert!(!is_authorized(999, None, &cfg()));
    }

    #[test]
    fn auth_by_user() {
        assert!(is_authorized(999, Some(222), &cfg()));
        assert!(!is_authorized(999, Some(888), &cfg()));
    }

    #[test]
    fn auth_denied_when_no_allowlist() {
        let empty = TelegramConfig::default();
        assert!(!is_authorized(1, Some(2), &empty));
    }

    #[test]
    fn legacy_allowlist_is_owner_when_no_owners_configured() {
        // Backward compatibility: pre-role setups keep full control.
        assert_eq!(telegram_role(111, None, &cfg()), Some(TgRole::Owner));
        assert_eq!(telegram_role(999, Some(222), &cfg()), Some(TgRole::Owner));
    }

    #[test]
    fn roles_split_when_owners_configured() {
        let c = TelegramConfig {
            owner_user_ids: vec![1],
            readonly_user_ids: vec![3],
            allowed_user_ids: vec![2],
            allowed_chat_ids: vec![111],
            ..Default::default()
        };
        assert_eq!(telegram_role(0, Some(1), &c), Some(TgRole::Owner));
        assert_eq!(telegram_role(0, Some(2), &c), Some(TgRole::Operator));
        assert_eq!(telegram_role(0, Some(3), &c), Some(TgRole::Readonly));
        assert_eq!(telegram_role(111, None, &c), Some(TgRole::Operator));
        assert_eq!(telegram_role(9, Some(9), &c), None);
        // Owner list wins over readonly even for the same id.
        let both = TelegramConfig {
            owner_user_ids: vec![7],
            readonly_user_ids: vec![7],
            ..Default::default()
        };
        assert_eq!(telegram_role(0, Some(7), &both), Some(TgRole::Owner));
    }

    #[test]
    fn command_role_requirements() {
        assert_eq!(command_requires(&Command::Status), TgRole::Readonly);
        assert_eq!(command_requires(&Command::Positions), TgRole::Readonly);
        assert_eq!(command_requires(&Command::Kill), TgRole::Operator);
        assert_eq!(
            command_requires(&Command::Mode(Some("live".into()))),
            TgRole::Owner
        );
        assert_eq!(
            command_requires(&Command::Mode(Some("LIVE".into()))),
            TgRole::Owner
        );
        assert_eq!(
            command_requires(&Command::Mode(Some("paper".into()))),
            TgRole::Operator
        );
        assert_eq!(command_requires(&Command::Mode(None)), TgRole::Operator);
    }

    #[tokio::test]
    async fn handle_refuses_insufficient_roles_loudly() {
        let state = bot_core::state::AppState::new(bot_core::config::AppConfig::from_defaults());
        let reply = handle(&state, Command::Kill, TgRole::Readonly).await;
        assert!(reply.contains("cannot run this command"), "{reply}");
        assert!(!state.kill_switch(), "refused command had NO effect");
        // Operator CAN kill.
        let reply = handle(&state, Command::Kill, TgRole::Operator).await;
        assert!(reply.contains("KILL SWITCH"), "{reply}");
        assert!(state.kill_switch());
        // Readonly reads work.
        let reply = handle(&state, Command::Status, TgRole::Readonly).await;
        assert!(!reply.contains("cannot run"), "{reply}");
        // Operator cannot switch to live.
        let reply = handle(&state, Command::Mode(Some("live".into())), TgRole::Operator).await;
        assert!(reply.contains("cannot run this command"), "{reply}");
    }

    #[test]
    fn parses_basic_commands() {
        assert_eq!(parse_command("/status", ""), Command::Status);
        assert_eq!(parse_command("  /help  ", ""), Command::Help);
        assert_eq!(parse_command("/kill", ""), Command::Kill);
        assert_eq!(parse_command("/resume", ""), Command::Resume);
        assert_eq!(parse_command("/positions", ""), Command::Positions);
        assert_eq!(parse_command("/pnl", ""), Command::Pnl);
    }

    #[test]
    fn parses_on_off_targets() {
        assert_eq!(parse_command("/on all", ""), Command::On(Target::All));
        assert_eq!(
            parse_command("/on sniper", ""),
            Command::On(Target::Module(BotModule::Sniper))
        );
        assert_eq!(
            parse_command("/off polymarket", ""),
            Command::Off(Target::Module(BotModule::Polymarket))
        );
        // unknown module name falls back to All
        assert_eq!(parse_command("/off wat", ""), Command::Off(Target::All));
    }

    #[test]
    fn parses_mode_with_and_without_arg() {
        assert_eq!(parse_command("/mode", ""), Command::Mode(None));
        assert_eq!(
            parse_command("/mode live", ""),
            Command::Mode(Some("live".into()))
        );
        assert_eq!(
            parse_command("/mode PAPER", ""),
            Command::Mode(Some("paper".into()))
        );
    }

    #[test]
    fn strips_botname_suffix() {
        assert_eq!(parse_command("/status@mybot", ""), Command::Status);
    }

    #[test]
    fn honours_prefix() {
        assert_eq!(parse_command("sb /status", "sb"), Command::Status);
        // Without the prefix, it is not for us.
        assert_eq!(
            parse_command("/status", "sb"),
            Command::Unknown(String::new())
        );
    }

    #[test]
    fn unknown_command_carries_token() {
        assert_eq!(
            parse_command("/frobnicate", ""),
            Command::Unknown("frobnicate".into())
        );
    }
}
