//! The trade-command seam (BATCH 04).
//!
//! `module-telegram` never touches a venue, a wallet or the risk engine
//! directly. It speaks to exactly one port, [`TradeCommandPort`], which the
//! server crate implements in the final integration batch (Batch 07) and
//! which this crate ships as a complete in-memory double
//! ([`FakeTradePort`]) so the whole consent → nonce → confirm → execute flow
//! is testable end to end today.
//!
//! The port is deliberately small and fail-closed:
//!
//! * [`TradeCommandPort::is_halted`] — the operator kill switch / emergency
//!   halt, as seen by the executing side.
//! * [`TradeCommandPort::current_mode`] — paper / simulate / live **and**
//!   whether the two live gates (`execution.mode == live` *and*
//!   `execution.allow_live_trading`) are actually open.
//! * [`TradeCommandPort::org_risk_limits`] — the organization's own per-order
//!   and 24h ceilings. The Telegram consent can only ever TIGHTEN them.
//! * [`TradeCommandPort::place_buy`] / [`TradeCommandPort::place_sell`] — the
//!   only two methods that move money. Both take an idempotency key that the
//!   Telegram side derives from the command id, so a replayed confirmation can
//!   never execute twice even if the reply was lost on the wire.
//!
//! Money is expressed in **integer lamports** everywhere on this boundary: no
//! `f64` ever crosses it.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use bot_core::models::ExecutionMode;
use uuid::Uuid;

/// Which side of the book a command targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TradeAction {
    /// Spend SOL to acquire a token.
    Buy,
    /// Sell a fraction of an existing token position.
    Sell,
}

impl TradeAction {
    /// Stable lowercase label — used in replies, audit rows and SQL.
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeAction::Buy => "buy",
            TradeAction::Sell => "sell",
        }
    }

    /// Parse the label back (fail-closed: unknown labels are rejected).
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "buy" => Some(TradeAction::Buy),
            "sell" => Some(TradeAction::Sell),
            _ => None,
        }
    }
}

/// One fully validated order, ready for the executing side.
///
/// The Telegram module guarantees that every field here already passed
/// parsing, consent, cap, mode and rate-limit checks; the port must still
/// re-run its own gates (defence in depth) and is expected to reject whatever
/// it does not like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRequest {
    /// Telegram command id — the primary key of `telegram_trade_commands`.
    pub command_id: Uuid,
    /// Idempotency key handed to the venue path; always equals `command_id`
    /// so a retried confirmation is a no-op on the execution side.
    pub idempotency_key: Uuid,
    /// Tenant that owns the chat binding.
    pub organization_id: String,
    /// Telegram chat the command came from.
    pub chat_id: i64,
    /// Telegram user who requested (and confirmed) the command.
    pub user_id: i64,
    /// Buy or sell.
    pub action: TradeAction,
    /// Base58 token mint (canonically encoded, 32 bytes when decoded).
    pub mint: String,
    /// Buy only: SOL to spend, in lamports.
    pub amount_lamports: Option<u64>,
    /// Sell only: fraction of the open position, in basis points (`10000` =
    /// `all`).
    pub percent_bps: Option<u16>,
    /// Slippage tolerance in basis points, already range-checked.
    pub slippage_bps: u16,
}

impl OrderRequest {
    /// The SOL amount in lamports, or `0` for a sell (which carries a
    /// percentage instead). Never used for money arithmetic — only for audit
    /// detail — but kept total so callers cannot unwrap.
    pub fn amount_lamports_or_zero(&self) -> u64 {
        self.amount_lamports.unwrap_or(0)
    }
}

/// What the executing side reports back for a filled order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReceipt {
    /// Venue-side reference (signature / order id). Opaque to Telegram.
    pub reference: String,
    /// One-line human summary shown in the chat.
    pub detail: String,
}

impl ExecutionReceipt {
    /// Convenience constructor for implementations and tests.
    pub fn new(reference: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            detail: detail.into(),
        }
    }
}

/// Why the port refused or failed a command. The vocabulary is closed so the
/// audit trail stays queryable; the human text lives in
/// [`crate::commands::reply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortError {
    /// Kill switch / emergency halt is engaged.
    Halted,
    /// `execution.mode == live` but `allow_live_trading` is false (or the mode
    /// is unknown): the two live gates are not both open.
    LiveGatesClosed,
    /// The risk engine rejected the order (exposure, reserve, loss limit…).
    RiskRejected(String),
    /// The port itself is unavailable (DB down, RPC down, …).
    Unavailable(String),
}

impl PortError {
    /// Stable machine code used as the audit `failure_code`.
    pub fn code(&self) -> &'static str {
        match self {
            PortError::Halted => "halted",
            PortError::LiveGatesClosed => "live_gates_closed",
            PortError::RiskRejected(_) => "risk_rejected",
            PortError::Unavailable(_) => "port_unavailable",
        }
    }

    /// Free-text detail. Always passed through
    /// [`crate::commands::audit::scrub`] before it reaches an audit row or a
    /// chat reply, so a port that echoes a token or a nonce cannot leak it.
    pub fn detail(&self) -> &str {
        match self {
            PortError::Halted => "kill switch engaged",
            PortError::LiveGatesClosed => "live trading gates are not both open",
            PortError::RiskRejected(reason) => reason,
            PortError::Unavailable(reason) => reason,
        }
    }
}

/// The organization's own risk ceilings, as reported by the executing side.
///
/// Both are integer lamports and both are hard: the Telegram layer takes
/// `min(consent cap, org limit)` and never the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrgRiskLimits {
    /// Largest single order the organization allows, in lamports.
    pub max_order_lamports: u64,
    /// Largest rolling 24h buy volume the organization allows, in lamports.
    pub daily_cap_lamports: u64,
}

impl OrgRiskLimits {
    /// Convenience constructor.
    pub fn new(max_order_lamports: u64, daily_cap_lamports: u64) -> Self {
        Self {
            max_order_lamports,
            daily_cap_lamports,
        }
    }
}

/// Execution mode plus the state of the live gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortMode {
    /// Configured execution mode.
    pub mode: ExecutionMode,
    /// `execution.allow_live_trading` — the second of the two live gates.
    pub live_allowed: bool,
}

impl PortMode {
    /// Convenience constructor.
    pub fn new(mode: ExecutionMode, live_allowed: bool) -> Self {
        Self { mode, live_allowed }
    }

    /// Paper is the safe default an unwired deployment starts in.
    pub fn paper() -> Self {
        Self::new(ExecutionMode::Paper, false)
    }

    /// `true` only when BOTH live gates are open: the mode is `live` **and**
    /// the hard `allow_live_trading` flag is set.
    pub fn live_gates_open(&self) -> bool {
        self.mode.is_live() && self.live_allowed
    }

    /// `true` when the mode is `live` but the gates are not both open — an
    /// inconsistent configuration that must fail closed.
    pub fn live_but_gated(&self) -> bool {
        self.mode.is_live() && !self.live_allowed
    }
}

/// The seam the server implements in Batch 07.
///
/// Every method is `async` and fallible; implementations MUST NOT panic and
/// MUST NOT block. A `PortError` is treated as a denial by the caller.
#[async_trait]
pub trait TradeCommandPort: Send + Sync {
    /// Place a buy order. `req.action` is always [`TradeAction::Buy`].
    async fn place_buy(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError>;

    /// Place a sell order. `req.action` is always [`TradeAction::Sell`].
    async fn place_sell(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError>;

    /// `true` while the kill switch or the emergency halt is engaged.
    async fn is_halted(&self) -> bool;

    /// The organization's hard risk ceilings.
    async fn org_risk_limits(&self, organization_id: &str) -> Result<OrgRiskLimits, PortError>;

    /// The current execution mode and live-gate state.
    async fn current_mode(&self) -> Result<PortMode, PortError>;

    /// Dispatch on `req.action`. Implementations normally leave this default
    /// in place; it exists so callers never have to match on the action
    /// themselves (single dispatch point, DRY).
    async fn place(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError> {
        match req.action {
            TradeAction::Buy => self.place_buy(req).await,
            TradeAction::Sell => self.place_sell(req).await,
        }
    }
}

/// A complete, deterministic in-memory [`TradeCommandPort`].
///
/// It is a real test double, not a stub: it records every accepted order,
/// honours idempotency keys (a repeated key returns the first receipt and is
/// not recorded twice), and can be told to halt, to close the live gates, or
/// to fail with a specific [`PortError`].
pub struct FakeTradePort {
    halted: Mutex<bool>,
    mode: Mutex<PortMode>,
    limits: Mutex<HashMap<String, OrgRiskLimits>>,
    forced_error: Mutex<Option<PortError>>,
    /// Accepted orders in call order.
    orders: Mutex<Vec<OrderRequest>>,
    /// Idempotency key → receipt of the first execution.
    receipts: Mutex<HashMap<Uuid, ExecutionReceipt>>,
}

impl Default for FakeTradePort {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FakeTradePort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeTradePort")
            .field("accepted_orders", &self.accepted_order_count())
            .finish_non_exhaustive()
    }
}

impl FakeTradePort {
    /// A port in paper mode, not halted, with generous limits for `org-1`.
    pub fn new() -> Self {
        let mut limits = HashMap::new();
        limits.insert(
            "org-1".to_string(),
            OrgRiskLimits::new(1_000_000_000, 10_000_000_000),
        );
        Self {
            halted: Mutex::new(false),
            mode: Mutex::new(PortMode::paper()),
            limits: Mutex::new(limits),
            forced_error: Mutex::new(None),
            orders: Mutex::new(Vec::new()),
            receipts: Mutex::new(HashMap::new()),
        }
    }

    /// Set the kill-switch state seen by the port.
    pub fn set_halted(&self, halted: bool) {
        *self.halted.lock().expect("fake port halted lock") = halted;
    }

    /// Set the execution mode / live-gate state.
    pub fn set_mode(&self, mode: PortMode) {
        *self.mode.lock().expect("fake port mode lock") = mode;
    }

    /// Set the risk ceilings reported for `organization_id`.
    pub fn set_org_limits(&self, organization_id: &str, limits: OrgRiskLimits) {
        self.limits
            .lock()
            .expect("fake port limits lock")
            .insert(organization_id.to_string(), limits);
    }

    /// Forget the ceilings for `organization_id` (the port then reports
    /// "unknown", which the Telegram layer treats as a denial).
    pub fn clear_org_limits(&self, organization_id: &str) {
        self.limits
            .lock()
            .expect("fake port limits lock")
            .remove(organization_id);
    }

    /// Make every subsequent `place_*` call fail with `error`.
    pub fn fail_with(&self, error: PortError) {
        *self.forced_error.lock().expect("fake port error lock") = Some(error);
    }

    /// Stop failing.
    pub fn clear_failure(&self) {
        *self.forced_error.lock().expect("fake port error lock") = None;
    }

    /// Number of distinct orders actually accepted (idempotent replays are
    /// not counted twice).
    pub fn accepted_order_count(&self) -> usize {
        self.receipts.lock().expect("fake port receipts lock").len()
    }

    /// Every `place_*` invocation, in call order (including idempotent
    /// replays). Tests use this to prove "never double executes".
    pub fn call_count(&self) -> usize {
        self.orders.lock().expect("fake port orders lock").len()
    }

    /// The accepted orders, in call order.
    pub fn accepted_orders(&self) -> Vec<OrderRequest> {
        self.orders.lock().expect("fake port orders lock").clone()
    }

    /// The receipt originally produced for `idempotency_key`, if any.
    pub fn receipt_for(&self, idempotency_key: Uuid) -> Option<ExecutionReceipt> {
        self.receipts
            .lock()
            .expect("fake port receipts lock")
            .get(&idempotency_key)
            .cloned()
    }

    fn record(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError> {
        if *self.halted.lock().expect("fake port halted lock") {
            return Err(PortError::Halted);
        }
        if let Some(err) = self
            .forced_error
            .lock()
            .expect("fake port error lock")
            .clone()
        {
            return Err(err);
        }
        let mode = *self.mode.lock().expect("fake port mode lock");
        if mode.live_but_gated() {
            return Err(PortError::LiveGatesClosed);
        }
        self.orders
            .lock()
            .expect("fake port orders lock")
            .push(req.clone());
        let mut receipts = self.receipts.lock().expect("fake port receipts lock");
        if let Some(existing) = receipts.get(&req.idempotency_key) {
            // Idempotent replay: the venue path must not run twice.
            return Ok(existing.clone());
        }
        let receipt = ExecutionReceipt::new(
            format!("fake-{}", req.idempotency_key),
            format!(
                "{} {} simulated in {} mode",
                req.action.as_str(),
                req.mint,
                mode.mode.as_str()
            ),
        );
        receipts.insert(req.idempotency_key, receipt.clone());
        Ok(receipt)
    }
}

#[async_trait]
impl TradeCommandPort for FakeTradePort {
    async fn place_buy(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError> {
        self.record(req)
    }

    async fn place_sell(&self, req: &OrderRequest) -> Result<ExecutionReceipt, PortError> {
        self.record(req)
    }

    async fn is_halted(&self) -> bool {
        *self.halted.lock().expect("fake port halted lock")
    }

    async fn org_risk_limits(&self, organization_id: &str) -> Result<OrgRiskLimits, PortError> {
        self.limits
            .lock()
            .expect("fake port limits lock")
            .get(organization_id)
            .copied()
            .ok_or_else(|| {
                PortError::Unavailable(format!("no risk limits for organization {organization_id}"))
            })
    }

    async fn current_mode(&self) -> Result<PortMode, PortError> {
        Ok(*self.mode.lock().expect("fake port mode lock"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(action: TradeAction) -> OrderRequest {
        OrderRequest {
            command_id: Uuid::from_u128(1),
            idempotency_key: Uuid::from_u128(1),
            organization_id: "org-1".into(),
            chat_id: 100,
            user_id: 7,
            action,
            mint: "So11111111111111111111111111111111111111112".into(),
            amount_lamports: match action {
                TradeAction::Buy => Some(10_000_000),
                TradeAction::Sell => None,
            },
            percent_bps: match action {
                TradeAction::Buy => None,
                TradeAction::Sell => Some(5_000),
            },
            slippage_bps: 100,
        }
    }

    #[test]
    fn action_labels_round_trip() {
        for a in [TradeAction::Buy, TradeAction::Sell] {
            assert_eq!(TradeAction::from_label(a.as_str()), Some(a));
        }
        assert_eq!(TradeAction::from_label("BUY"), Some(TradeAction::Buy));
        assert_eq!(TradeAction::from_label("short"), None);
        assert_eq!(TradeAction::from_label(""), None);
    }

    #[test]
    fn live_gates_need_both_flags() {
        assert!(!PortMode::paper().live_gates_open());
        assert!(!PortMode::new(ExecutionMode::Live, false).live_gates_open());
        assert!(PortMode::new(ExecutionMode::Live, true).live_gates_open());
        assert!(PortMode::new(ExecutionMode::Live, false).live_but_gated());
        assert!(!PortMode::paper().live_but_gated());
    }

    #[tokio::test]
    async fn dispatch_routes_on_action() {
        let port = FakeTradePort::new();
        let buy = req(TradeAction::Buy);
        let sell = OrderRequest {
            command_id: Uuid::from_u128(2),
            idempotency_key: Uuid::from_u128(2),
            ..req(TradeAction::Sell)
        };
        assert!(port.place(&buy).await.is_ok());
        assert!(port.place(&sell).await.is_ok());
        assert_eq!(port.accepted_order_count(), 2);
        assert_eq!(port.accepted_orders()[0].action, TradeAction::Buy);
        assert_eq!(port.accepted_orders()[1].action, TradeAction::Sell);
    }

    #[tokio::test]
    async fn idempotent_replay_does_not_duplicate() {
        let port = FakeTradePort::new();
        let r = req(TradeAction::Buy);
        let first = port.place_buy(&r).await.expect("first");
        let second = port.place_buy(&r).await.expect("replay");
        assert_eq!(first, second);
        assert_eq!(port.accepted_order_count(), 1);
        assert_eq!(port.receipt_for(r.idempotency_key), Some(first));
    }

    #[tokio::test]
    async fn halt_and_gates_and_forced_errors_are_reported() {
        let port = FakeTradePort::new();
        port.set_halted(true);
        assert!(port.is_halted().await);
        assert_eq!(
            port.place_buy(&req(TradeAction::Buy)).await,
            Err(PortError::Halted)
        );
        port.set_halted(false);
        port.set_mode(PortMode::new(ExecutionMode::Live, false));
        assert_eq!(
            port.place_buy(&req(TradeAction::Buy)).await,
            Err(PortError::LiveGatesClosed)
        );
        port.set_mode(PortMode::new(ExecutionMode::Live, true));
        assert!(port.place_buy(&req(TradeAction::Buy)).await.is_ok());
        port.fail_with(PortError::RiskRejected("max exposure".into()));
        assert_eq!(
            port.place_sell(&req(TradeAction::Sell)).await,
            Err(PortError::RiskRejected("max exposure".into()))
        );
        port.clear_failure();
        assert!(port.place_sell(&req(TradeAction::Sell)).await.is_ok());
    }

    #[tokio::test]
    async fn missing_org_limits_fail_closed() {
        let port = FakeTradePort::new();
        assert!(port.org_risk_limits("org-1").await.is_ok());
        port.clear_org_limits("org-1");
        let err = port.org_risk_limits("org-1").await.expect_err("denied");
        assert_eq!(err.code(), "port_unavailable");
    }

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(PortError::Halted.code(), "halted");
        assert_eq!(PortError::LiveGatesClosed.code(), "live_gates_closed");
        assert_eq!(PortError::RiskRejected("x".into()).code(), "risk_rejected");
        assert_eq!(
            PortError::Unavailable("x".into()).code(),
            "port_unavailable"
        );
        assert_eq!(req(TradeAction::Sell).amount_lamports_or_zero(), 0);
        assert_eq!(req(TradeAction::Buy).amount_lamports_or_zero(), 10_000_000);
    }
}
