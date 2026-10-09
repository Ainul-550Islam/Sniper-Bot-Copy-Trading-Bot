//! Telegram trade sessions (GAP-MAP v2, P2).
//!
//! Turns the Telegram control bot from read-only into a TRADING surface
//! (`/buy <mint> <sol>`, `/sell <mint> <pct>`), and does so behind three
//! safety walls:
//!
//! 1. **Chat → tenant binding.** A chat may trade only after it is bound to
//!    an organization. Unbound chats are refused — a Telegram chat id is
//!    NOT an identity by itself. The server binds chats when a tenant
//!    links Telegram from the control plane; operator mode binds the
//!    operator org explicitly.
//! 2. **Per-chat limits.** Trades per time window, SOL per single trade and
//!    SOL per UTC day are enforced BEFORE confirmation and RE-CHECKED at
//!    confirmation time (limits must hold at the moment money moves, not
//!    at the moment the keyboard was drawn). All checks fail closed.
//! 3. **Confirmation TTL.** Every trade becomes a pending confirmation
//!    (rendered as an inline keyboard by [`crate::callbacks`]) with a
//!    nonce, an expiry and a bound requester. Stale, replayed, expired or
//!    foreign-user confirmations are rejected explicitly.
//!
//! Nothing here executes a trade: the confirmed [`TradeIntent`] is handed
//! to a [`TradeExecutor`], which the server implements tenant-bound (the
//! same split as `module-sniper`'s `TenantExecutionSink`). This keeps the
//! crate free of venue knowledge and every rule above unit-testable.

use std::collections::{HashMap, VecDeque};

use async_trait::async_trait;
use chrono::{DateTime, Datelike, Utc};

use bot_core::error::{BotError, BotResult};

/// Which side of the trade the operator asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeSide {
    /// Buy with a fixed SOL amount.
    Buy,
    /// Sell a fraction of the open position in the mint.
    Sell,
}

impl TradeSide {
    /// Stable label for journaling / reply text.
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeSide::Buy => "buy",
            TradeSide::Sell => "sell",
        }
    }
}

/// One confirmed trade request, ready for the executor.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeIntent {
    /// Chat the trade was requested in.
    pub chat_id: i64,
    /// Telegram user who requested it (only this user may confirm).
    pub user_id: i64,
    /// The bound tenant the trade executes for.
    pub organization_id: String,
    pub side: TradeSide,
    /// Base58 mint.
    pub mint: String,
    /// Buy only: SOL to spend.
    pub amount_sol: f64,
    /// Sell only: fraction of the open position in (0, 1].
    pub sell_fraction: f64,
}

impl TradeIntent {
    /// Validate the intent's own fields (independent of limits). Fails
    /// closed on anything degenerate.
    pub fn validate(&self) -> BotResult<()> {
        if self.mint.trim().is_empty() {
            return Err(BotError::invalid("trade: mint is required"));
        }
        // Base58 sanity: alphabet check only — full decoding happens in
        // the execution pipeline; rejecting the obvious junk here keeps
        // error text at the chat instead of deep in a venue call.
        if !self
            .mint
            .chars()
            .all(|c| c.is_ascii_alphanumeric() && !"0OIl".contains(c))
        {
            return Err(BotError::invalid(
                "trade: mint is not base58 (0, O, I, l are not allowed)",
            ));
        }
        if self.mint.len() < 32 || self.mint.len() > 44 {
            return Err(BotError::invalid("trade: mint has an invalid length"));
        }
        match self.side {
            TradeSide::Buy => {
                if !self.amount_sol.is_finite() || self.amount_sol <= 0.0 {
                    return Err(BotError::invalid("trade: SOL amount must be positive"));
                }
            }
            TradeSide::Sell => {
                if !self.sell_fraction.is_finite()
                    || self.sell_fraction <= 0.0
                    || self.sell_fraction > 1.0
                {
                    return Err(BotError::invalid(
                        "trade: sell percent must be within (0, 100]",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Why a trade request was refused. Closed vocabulary, all user-visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeDeny {
    /// The chat is not bound to a tenant.
    UnboundChat,
    /// Trading is disabled for this binding.
    TradingDisabled,
    /// Mint failed validation.
    InvalidMint,
    /// Amount/fraction failed validation.
    InvalidAmount,
    /// More trades than allowed in the rolling window.
    TooManyTrades,
    /// The single-trade SOL cap is exceeded.
    SolPerTradeExceeded,
    /// The daily SOL cap is exceeded.
    DailySolExceeded,
    /// The confirmation was not found (expired/pruned or forged nonce).
    UnknownConfirmation,
    /// The confirmation expired.
    ConfirmationExpired,
    /// A different user tried to confirm/cancel.
    WrongUser,
    /// The confirmation was already resolved.
    AlreadyResolved,
}

impl TradeDeny {
    /// Stable machine label.
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeDeny::UnboundChat => "unbound_chat",
            TradeDeny::TradingDisabled => "trading_disabled",
            TradeDeny::InvalidMint => "invalid_mint",
            TradeDeny::InvalidAmount => "invalid_amount",
            TradeDeny::TooManyTrades => "too_many_trades",
            TradeDeny::SolPerTradeExceeded => "sol_per_trade_exceeded",
            TradeDeny::DailySolExceeded => "daily_sol_exceeded",
            TradeDeny::UnknownConfirmation => "unknown_confirmation",
            TradeDeny::ConfirmationExpired => "confirmation_expired",
            TradeDeny::WrongUser => "wrong_user",
            TradeDeny::AlreadyResolved => "already_resolved",
        }
    }

    /// Operator-friendly explanation (sent as the chat reply).
    pub fn reply_text(&self) -> &'static str {
        match self {
            TradeDeny::UnboundChat => {
                "⛔ This chat is not linked to an organization. Link it from the control plane first."
            }
            TradeDeny::TradingDisabled => {
                "⛔ Telegram trading is disabled for this organization."
            }
            TradeDeny::InvalidMint => "⛔ That does not look like a valid mint address.",
            TradeDeny::InvalidAmount => "⛔ Invalid amount — /buy <mint> <sol> or /sell <mint> <percent>.",
            TradeDeny::TooManyTrades => "⛔ Trade rate limit reached for this chat — slow down.",
            TradeDeny::SolPerTradeExceeded => "⛔ That amount exceeds the per-trade SOL limit for this chat.",
            TradeDeny::DailySolExceeded => "⛔ The daily SOL limit for this chat is exhausted.",
            TradeDeny::UnknownConfirmation => "⛔ This confirmation is unknown or already handled.",
            TradeDeny::ConfirmationExpired => "⌛ Confirmation expired — send the trade command again.",
            TradeDeny::WrongUser => "⛔ Only the user who requested this trade can confirm or cancel it.",
            TradeDeny::AlreadyResolved => "ℹ️ This confirmation was already resolved.",
        }
    }
}

/// A chat↔tenant binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatBinding {
    /// The tenant this chat trades for.
    pub organization_id: String,
    /// Whether trading commands are active for this binding (orgs can
    /// link a chat for ALERTS only).
    pub trading_enabled: bool,
}

/// Per-chat trading limits. Every bound fails closed (a zero or negative
/// bound blocks trading rather than allowing unlimited volume).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatLimits {
    /// Max confirmed trades per rolling window.
    pub max_trades_per_window: u32,
    /// Rolling window length.
    pub window_secs: u64,
    /// Max SOL a single buy may spend.
    pub max_sol_per_trade: f64,
    /// Max SOL confirmed per UTC day.
    pub max_daily_sol: f64,
    /// How long a confirmation stays valid.
    pub confirm_ttl_secs: u64,
}

impl Default for ChatLimits {
    fn default() -> Self {
        Self {
            max_trades_per_window: 5,
            window_secs: 60,
            max_sol_per_trade: 10.0,
            max_daily_sol: 100.0,
            confirm_ttl_secs: 90,
        }
    }
}

impl ChatLimits {
    /// Validate the bounds themselves (config hygiene).
    pub fn validate(&self) -> BotResult<()> {
        if self.max_trades_per_window == 0 {
            return Err(BotError::invalid("limits: max_trades_per_window must be > 0"));
        }
        if self.window_secs == 0 {
            return Err(BotError::invalid("limits: window_secs must be > 0"));
        }
        if !self.max_sol_per_trade.is_finite() || self.max_sol_per_trade <= 0.0 {
            return Err(BotError::invalid("limits: max_sol_per_trade must be positive"));
        }
        if !self.max_daily_sol.is_finite() || self.max_daily_sol <= 0.0 {
            return Err(BotError::invalid("limits: max_daily_sol must be positive"));
        }
        if self.confirm_ttl_secs == 0 {
            return Err(BotError::invalid("limits: confirm_ttl_secs must be > 0"));
        }
        Ok(())
    }
}

/// A pending confirmation awaiting the inline-keyboard tap.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTrade {
    /// Random nonce — the confirmation handle (never guessable: 16 hex
    /// chars from the OS RNG).
    pub nonce: String,
    pub intent: TradeIntent,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// The executor boundary. The server implements this tenant-bound (it
/// re-runs EVERY gate — risk, exposure, kill switch — before anything is
/// signed); the crate ships [`UnconfiguredTradeExecutor`] so an unwired
/// deployment fails closed instead of silently doing nothing.
#[async_trait]
pub trait TradeExecutor: Send + Sync {
    /// Execute one confirmed intent; the returned summary becomes the
    /// chat reply.
    async fn execute(&self, intent: &TradeIntent) -> BotResult<String>;
}

/// Default executor: refuses everything with an honest, auditable error.
pub struct UnconfiguredTradeExecutor;

#[async_trait]
impl TradeExecutor for UnconfiguredTradeExecutor {
    async fn execute(&self, _intent: &TradeIntent) -> BotResult<String> {
        Err(BotError::config(
            "telegram trading: no TradeExecutor is wired for this deployment",
        ))
    }
}

/// UTC-day key for the daily SOL counter.
fn day_key(ts: DateTime<Utc>) -> i32 {
    ts.year() * 1000 + ts.ordinal() as i32
}

/// The trade session: bindings + limits + pending confirmations for every
/// chat the bot trades with. One instance per bot process, behind a Mutex
/// in the [`TradeDesk`].
#[derive(Debug)]
pub struct TradeSession {
    bindings: HashMap<i64, ChatBinding>,
    limits: ChatLimits,
    /// Confirmed-trade timestamps per chat (rolling window bookkeeping).
    window: HashMap<i64, VecDeque<DateTime<Utc>>>,
    /// SOL confirmed per (chat, UTC day).
    daily_sol: HashMap<(i64, i32), f64>,
    /// Live confirmations, keyed by nonce.
    pending: HashMap<String, PendingTrade>,
    /// Nonces already resolved (confirm OR cancel) — replay defence that
    /// survives the pending-map removal.
    resolved: VecDeque<String>,
}

/// Upper bound on the replay-defence ring.
const RESOLVED_CAP: usize = 10_000;

impl TradeSession {
    /// Build a session with the given limits (validated).
    pub fn new(limits: ChatLimits) -> BotResult<Self> {
        limits.validate()?;
        Ok(Self {
            bindings: HashMap::new(),
            limits,
            window: HashMap::new(),
            daily_sol: HashMap::new(),
            pending: HashMap::new(),
            resolved: VecDeque::new(),
        })
    }

    /// The active limits (for `/limit` reporting).
    pub fn limits(&self) -> &ChatLimits {
        &self.limits
    }

    /// Bind a chat to a tenant (server calls this from the link flow).
    pub fn bind(&mut self, chat_id: i64, binding: ChatBinding) {
        self.bindings.insert(chat_id, binding);
    }

    /// Remove a binding (unlink). Pending confirmations of that chat are
    /// dropped: an unlinked chat must not keep live money buttons.
    pub fn unbind(&mut self, chat_id: i64) -> bool {
        self.pending.retain(|_, p| p.intent.chat_id != chat_id);
        self.bindings.remove(&chat_id).is_some()
    }

    /// The binding, if any.
    pub fn binding(&self, chat_id: i64) -> Option<&ChatBinding> {
        self.bindings.get(&chat_id)
    }

    /// Trades confirmed by this chat inside the current window.
    pub fn trades_in_window(&self, chat_id: i64, now: DateTime<Utc>) -> usize {
        let cutoff = now - chrono::Duration::seconds(self.limits.window_secs as i64);
        self.window
            .get(&chat_id)
            .map(|dq| dq.iter().filter(|t| **t > cutoff).count())
            .unwrap_or(0)
    }

    /// SOL confirmed by this chat today (UTC).
    pub fn sol_today(&self, chat_id: i64, now: DateTime<Utc>) -> f64 {
        self.daily_sol
            .get(&(chat_id, day_key(now)))
            .copied()
            .unwrap_or(0.0)
    }

    /// Drop expired pending confirmations (call on the poll tick).
    pub fn prune_expired(&mut self, now: DateTime<Utc>) -> usize {
        let before = self.pending.len();
        self.pending.retain(|_, p| p.expires_at > now);
        before - self.pending.len()
    }

    /// Validate an intent against binding + limits WITHOUT consuming any
    /// capacity. Returns the would-be pending confirmation.
    pub fn prepare_confirmation(
        &mut self,
        intent: TradeIntent,
        now: DateTime<Utc>,
    ) -> Result<PendingTrade, TradeDeny> {
        let binding = self
            .bindings
            .get(&intent.chat_id)
            .ok_or(TradeDeny::UnboundChat)?;
        if !binding.trading_enabled {
            return Err(TradeDeny::TradingDisabled);
        }
        // The intent carries the TENANT identity from the binding — the
        // chat never supplies it, so a forged intent field cannot redirect
        // execution to another organization.
        let mut intent = intent;
        intent.organization_id = binding.organization_id.clone();
        if intent.validate().is_err() {
            return Err(if intent.mint.trim().is_empty()
                || intent.mint.len() < 32
                || intent.mint.len() > 44
                || !intent
                    .mint
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() && !"0OIl".contains(c))
            {
                TradeDeny::InvalidMint
            } else {
                TradeDeny::InvalidAmount
            });
        }
        self.check_limits(&intent, now)?;
        let nonce = fresh_nonce();
        let pending = PendingTrade {
            nonce: nonce.clone(),
            intent: intent.clone(),
            created_at: now,
            expires_at: now + chrono::Duration::seconds(self.limits.confirm_ttl_secs as i64),
        };
        self.pending.insert(nonce, pending.clone());
        Ok(pending)
    }

    /// Resolve a confirmation. Re-checks EVERY limit at the moment of the
    /// tap (not at keyboard draw time) and only consumes capacity on
    /// success. Returns the intent to execute.
    pub fn confirm(
        &mut self,
        nonce: &str,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> Result<TradeIntent, TradeDeny> {
        if self.resolved.iter().any(|n| n == nonce) {
            return Err(TradeDeny::AlreadyResolved);
        }
        let pending = self
            .pending
            .remove(nonce)
            .ok_or(TradeDeny::UnknownConfirmation)?;
        if pending.intent.user_id != user_id {
            // Put it back: the rightful owner must still be able to tap.
            self.pending.insert(nonce.to_string(), pending);
            return Err(TradeDeny::WrongUser);
        }
        if pending.expires_at <= now {
            self.mark_resolved(nonce);
            return Err(TradeDeny::ConfirmationExpired);
        }
        // The binding may have been revoked between draw and tap.
        let binding = self
            .bindings
            .get(&pending.intent.chat_id)
            .ok_or(TradeDeny::UnboundChat)?;
        if !binding.trading_enabled {
            self.mark_resolved(nonce);
            return Err(TradeDeny::TradingDisabled);
        }
        // Limits re-checked at the moment money moves.
        if let Err(deny) = self.check_limits(&pending.intent, now) {
            self.mark_resolved(nonce);
            return Err(deny);
        }
        self.consume_capacity(&pending.intent, now);
        self.mark_resolved(nonce);
        Ok(pending.intent)
    }

    /// Cancel a pending confirmation. Only the requester may cancel.
    pub fn cancel(&mut self, nonce: &str, user_id: i64) -> Result<(), TradeDeny> {
        if self.resolved.iter().any(|n| n == nonce) {
            return Err(TradeDeny::AlreadyResolved);
        }
        let pending = self
            .pending
            .get(nonce)
            .ok_or(TradeDeny::UnknownConfirmation)?;
        if pending.intent.user_id != user_id {
            return Err(TradeDeny::WrongUser);
        }
        self.pending.remove(nonce);
        self.mark_resolved(nonce);
        Ok(())
    }

    /// Number of live pending confirmations (diagnostics/tests).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    fn check_limits(&self, intent: &TradeIntent, now: DateTime<Utc>) -> Result<(), TradeDeny> {
        if self.trades_in_window(intent.chat_id, now) >= self.limits.max_trades_per_window as usize
        {
            return Err(TradeDeny::TooManyTrades);
        }
        if intent.side == TradeSide::Buy {
            if intent.amount_sol > self.limits.max_sol_per_trade {
                return Err(TradeDeny::SolPerTradeExceeded);
            }
            if self.sol_today(intent.chat_id, now) + intent.amount_sol > self.limits.max_daily_sol
            {
                return Err(TradeDeny::DailySolExceeded);
            }
        }
        Ok(())
    }

    fn consume_capacity(&mut self, intent: &TradeIntent, now: DateTime<Utc>) {
        let dq = self.window.entry(intent.chat_id).or_default();
        dq.push_back(now);
        let cutoff = now - chrono::Duration::seconds(self.limits.window_secs as i64);
        while let Some(front) = dq.front() {
            if *front <= cutoff {
                dq.pop_front();
            } else {
                break;
            }
        }
        if intent.side == TradeSide::Buy {
            let entry = self.daily_sol.entry((intent.chat_id, day_key(now))).or_insert(0.0);
            *entry += intent.amount_sol;
        }
    }

    fn mark_resolved(&mut self, nonce: &str) {
        if self.resolved.len() >= RESOLVED_CAP {
            self.resolved.pop_front();
        }
        self.resolved.push_back(nonce.to_string());
    }
}

/// 16-hex-char nonce from the OS RNG.
pub fn fresh_nonce() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 8] = rng.gen();
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> TradeSession {
        TradeSession::new(ChatLimits {
            max_trades_per_window: 2,
            window_secs: 60,
            max_sol_per_trade: 5.0,
            max_daily_sol: 10.0,
            confirm_ttl_secs: 90,
        })
        .unwrap()
    }

    fn bound(s: &mut TradeSession) {
        s.bind(
            100,
            ChatBinding {
                organization_id: "org-1".into(),
                trading_enabled: true,
            },
        );
    }

    fn buy(chat: i64, user: i64, sol: f64) -> TradeIntent {
        TradeIntent {
            chat_id: chat,
            user_id: user,
            organization_id: String::new(), // filled from binding
            side: TradeSide::Buy,
            mint: "So11111111111111111111111111111111111111112".into(),
            amount_sol: sol,
            sell_fraction: 0.0,
        }
    }

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn intent_validation_fails_closed() {
        let mut i = buy(100, 1, 1.0);
        assert!(i.validate().is_ok());
        i.mint = "not base58 0OIl".into();
        assert!(i.validate().is_err());
        i.mint = "short".into();
        assert!(i.validate().is_err());
        let mut s = buy(100, 1, 0.0);
        s.mint = "So11111111111111111111111111111111111111112".into();
        assert!(s.validate().is_err(), "zero SOL");
        let mut f = buy(100, 1, 1.0);
        f.side = TradeSide::Sell;
        f.sell_fraction = 1.5;
        assert!(f.validate().is_err(), "fraction > 1");
        f.sell_fraction = 1.0;
        assert!(f.validate().is_ok(), "100% sell is valid");
    }

    #[test]
    fn unbound_chats_are_refused() {
        let mut s = session();
        let err = s.prepare_confirmation(buy(100, 1, 1.0), t(0)).unwrap_err();
        assert_eq!(err, TradeDeny::UnboundChat);
    }

    #[test]
    fn alerts_only_bindings_refuse_trades() {
        let mut s = session();
        s.bind(
            100,
            ChatBinding {
                organization_id: "org-1".into(),
                trading_enabled: false,
            },
        );
        assert_eq!(
            s.prepare_confirmation(buy(100, 1, 1.0), t(0)).unwrap_err(),
            TradeDeny::TradingDisabled
        );
    }

    #[test]
    fn confirm_consumes_and_replays_are_rejected() {
        let mut s = session();
        bound(&mut s);
        let p = s.prepare_confirmation(buy(100, 1, 2.0), t(0)).unwrap();
        let intent = s.confirm(&p.nonce, 1, t(5)).unwrap();
        assert_eq!(intent.amount_sol, 2.0);
        assert_eq!(s.trades_in_window(100, t(5)), 1);
        assert!((s.sol_today(100, t(5)) - 2.0).abs() < 1e-9);
        // Replaying the same nonce never executes twice.
        assert_eq!(s.confirm(&p.nonce, 1, t(6)).unwrap_err(), TradeDeny::AlreadyResolved);
        assert_eq!(s.cancel(&p.nonce, 1).unwrap_err(), TradeDeny::AlreadyResolved);
    }

    #[test]
    fn only_the_requester_can_confirm_or_cancel() {
        let mut s = session();
        bound(&mut s);
        let p = s.prepare_confirmation(buy(100, 1, 1.0), t(0)).unwrap();
        assert_eq!(s.confirm(&p.nonce, 999, t(1)).unwrap_err(), TradeDeny::WrongUser);
        assert_eq!(s.cancel(&p.nonce, 999).unwrap_err(), TradeDeny::WrongUser);
        // The rightful owner can still act after the foreign attempts.
        assert!(s.confirm(&p.nonce, 1, t(2)).is_ok());
    }

    #[test]
    fn confirmations_expire() {
        let mut s = session();
        bound(&mut s);
        let p = s.prepare_confirmation(buy(100, 1, 1.0), t(0)).unwrap();
        assert_eq!(
            s.confirm(&p.nonce, 1, t(91)).unwrap_err(),
            TradeDeny::ConfirmationExpired
        );
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn limits_re_checked_at_tap_time() {
        let mut s = session();
        bound(&mut s);
        // Two confirmations at the per-trade cap fill the daily budget.
        let p1 = s.prepare_confirmation(buy(100, 1, 5.0), t(0)).unwrap();
        s.confirm(&p1.nonce, 1, t(1)).unwrap();
        let p2 = s.prepare_confirmation(buy(100, 1, 5.0), t(2)).unwrap();
        s.confirm(&p2.nonce, 1, t(3)).unwrap();
        // Window has room again later, but the DAILY cap must still
        // refuse at tap time even though prepare succeeded earlier.
        // Confirmations expire after 90s and the window is 60s, so the
        // timeline keeps every confirmation inside its TTL.
        let mut s2 = session();
        bound(&mut s2);
        let pa = s2.prepare_confirmation(buy(100, 1, 5.0), t(0)).unwrap();
        s2.confirm(&pa.nonce, 1, t(3)).unwrap(); // 5 SOL spent
        let pb = s2.prepare_confirmation(buy(100, 1, 5.0), t(95)).unwrap();
        // Prepared while the budget still has room, confirmed later.
        let pc = s2.prepare_confirmation(buy(100, 1, 5.0), t(96)).unwrap();
        s2.confirm(&pb.nonce, 1, t(100)).unwrap(); // 10 SOL spent; new window
        // Third tap at t=130: window has room (pa aged out), but the daily
        // 10 SOL cap is already reached, so the tap must still be refused.
        assert_eq!(s2.confirm(&pc.nonce, 1, t(130)).unwrap_err(), TradeDeny::DailySolExceeded);
    }

    #[test]
    fn per_trade_cap_and_rate_limits() {
        let mut s = session();
        bound(&mut s);
        assert_eq!(
            s.prepare_confirmation(buy(100, 1, 50.0), t(0)).unwrap_err(),
            TradeDeny::SolPerTradeExceeded
        );
        let p1 = s.prepare_confirmation(buy(100, 1, 0.1), t(0)).unwrap();
        let p2 = s.prepare_confirmation(buy(100, 1, 0.1), t(1)).unwrap();
        s.confirm(&p1.nonce, 1, t(2)).unwrap();
        s.confirm(&p2.nonce, 1, t(3)).unwrap();
        // Window (2) now full.
        assert_eq!(
            s.prepare_confirmation(buy(100, 1, 0.1), t(4)).unwrap_err(),
            TradeDeny::TooManyTrades
        );
        // After the window slides, trading is possible again.
        assert!(s.prepare_confirmation(buy(100, 1, 0.1), t(65)).is_ok());
    }

    #[test]
    fn unbind_drops_pending_confirmations() {
        let mut s = session();
        bound(&mut s);
        let p = s.prepare_confirmation(buy(100, 1, 1.0), t(0)).unwrap();
        assert!(s.unbind(100));
        assert_eq!(s.pending_count(), 0);
        assert_eq!(s.confirm(&p.nonce, 1, t(1)).unwrap_err(), TradeDeny::UnknownConfirmation);
    }

    #[test]
    fn prune_expired_keeps_live_confirmations() {
        let mut s = session();
        bound(&mut s);
        let _live = s.prepare_confirmation(buy(100, 1, 1.0), t(100)).unwrap();
        let stale = s.prepare_confirmation(buy(100, 1, 1.0), t(0)).unwrap();
        let removed = s.prune_expired(t(95));
        assert_eq!(removed, 1);
        assert_eq!(s.pending_count(), 1);
        assert_eq!(
            s.confirm(&stale.nonce, 1, t(95)).unwrap_err(),
            TradeDeny::UnknownConfirmation
        );
    }

    #[tokio::test]
    async fn unconfigured_executor_fails_closed_loudly() {
        let ex = UnconfiguredTradeExecutor;
        let err = ex.execute(&buy(1, 1, 1.0)).await.unwrap_err();
        assert!(err.to_string().contains("no TradeExecutor"));
    }

    #[test]
    fn nonces_are_unique_and_wellformed() {
        let a = fresh_nonce();
        let b = fresh_nonce();
        assert_ne!(a, b);
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
