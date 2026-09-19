//! Command parsing and dispatch for the Telegram control plane.
//!
//! [`parse_command`] is pure (and unit-tested); [`handle`] applies a parsed
//! command to shared state and returns the reply text. Authorization is
//! deny-by-default: if no allowlist is configured, control commands are refused.

use bot_core::config::TelegramConfig;
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::state::Shared;

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
        | Command::Config => TgRole::Readonly,
        // Switching TO live is the one telegram action reserved for owners.
        Command::Mode(Some(m)) if m.eq_ignore_ascii_case("live") => TgRole::Owner,
        Command::Mode(_)
        | Command::On(_)
        | Command::Off(_)
        | Command::Kill
        | Command::Resume
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

fn yn(b: bool) -> &'static str {
    if b {
        "y"
    } else {
        "n"
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
