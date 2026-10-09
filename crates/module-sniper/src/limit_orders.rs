//! Persistent limit / trigger orders (GAP-MAP v2, P2).
//!
//! Lets a tenant say "buy mint X if the mark falls to P" (or the sell
//! mirror) and have the background scheduler evaluate the trigger on every
//! sweep. The module owns the ORDER MODEL and the TRIGGER MATH; execution of
//! a triggered order re-enters the normal entry/exit pipeline (never a
//! bespoke money path), and the scheduler hook lives with the server's
//! `tenant_background` job loop.
//!
//! Durability: orders persist in Postgres (`limit_orders`, migration 0052)
//! through the [`LimitOrderStore`] trait. The in-memory store here serves
//! tests and operator-mode local runs; the server supplies the sqlx-backed
//! implementation when it wires the scheduler.
//!
//! Price convention: trigger prices are SOL per WHOLE token (the same units
//! the mark pipeline produces), stored as f64 exactly like `Position::last_mark`.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use bot_core::error::{BotError, BotResult};

/// Which side of the market the order sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        }
    }
}

/// Trigger semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerKind {
    /// Fires when `mark <= price` (buy-the-dip style).
    AtOrBelow,
    /// Fires when `mark >= price` (take-profit / chase style).
    AtOrAbove,
}

/// Order lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Waiting for its trigger.
    Active,
    /// Trigger observed; execution handed to the pipeline (terminal for the
    /// order record — the fill has its own lifecycle there).
    Triggered,
    /// Cancelled by the tenant before triggering.
    Cancelled,
    /// Expired without triggering (`expires_at` passed).
    Expired,
}

impl OrderStatus {
    /// True while the scheduler must keep evaluating the order.
    pub fn open(self) -> bool {
        matches!(self, OrderStatus::Active)
    }
}

/// One limit / trigger order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitOrder {
    /// Stable id (assigned by the store).
    pub id: String,
    /// Tenant scope — every query MUST be org-scoped (tenant_query gate).
    pub organization_id: String,
    /// The mint to trade.
    pub mint: String,
    pub side: OrderSide,
    pub trigger: TriggerKind,
    /// Trigger price, SOL per whole token.
    pub price_sol: f64,
    /// Size: SOL to spend (Buy) or fraction of the open position to sell
    /// (Sell, clamped to [0, 1] by the evaluator).
    pub amount: f64,
    pub status: OrderStatus,
    pub created_at: DateTime<Utc>,
    /// Optional expiry; `None` = good-til-cancelled.
    pub expires_at: Option<DateTime<Utc>>,
}

impl LimitOrder {
    /// Validate an order before it is stored. Fails closed on anything the
    /// trigger math cannot represent.
    pub fn validate(&self) -> BotResult<()> {
        if self.mint.trim().is_empty() {
            return Err(BotError::invalid("limit order: mint is required"));
        }
        if !self.price_sol.is_finite() || self.price_sol <= 0.0 {
            return Err(BotError::invalid(
                "limit order: price must be a positive finite number",
            ));
        }
        if !self.amount.is_finite() || self.amount <= 0.0 {
            return Err(BotError::invalid(
                "limit order: amount must be a positive finite number",
            ));
        }
        if matches!(self.side, OrderSide::Sell) && self.amount > 1.0 {
            return Err(BotError::invalid(
                "limit order: sell amount is a fraction of the position (<= 1.0)",
            ));
        }
        Ok(())
    }
}

/// What the evaluator wants the pipeline to do for one triggered order.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggeredOrder {
    pub order: LimitOrder,
    /// The mark that satisfied the trigger (for the audit trail).
    pub fired_at_price_sol: f64,
}

/// Persistence boundary. Implementations MUST scope every method by
/// `organization_id` (defense in depth: the server also runs the
/// tenant_query gate over the SQL).
#[async_trait]
pub trait LimitOrderStore: Send + Sync {
    /// Persist a new order, returning its assigned id.
    async fn insert(&self, order: &LimitOrder) -> BotResult<String>;
    /// All OPEN orders for one tenant.
    async fn list_active(&self, organization_id: &str) -> BotResult<Vec<LimitOrder>>;
    /// Move an order to a terminal status.
    async fn set_status(&self, organization_id: &str, order_id: &str, status: OrderStatus) -> BotResult<()>;
}

/// Evaluate one order against a mark. Pure and total: a malformed mark
/// never fires a trigger (fail-closed).
pub fn evaluate(order: &LimitOrder, mark_sol: f64) -> bool {
    if !mark_sol.is_finite() || mark_sol <= 0.0 {
        return false;
    }
    match order.trigger {
        TriggerKind::AtOrBelow => mark_sol <= order.price_sol,
        TriggerKind::AtOrAbove => mark_sol >= order.price_sol,
    }
}

/// One scheduler pass for one tenant.
///
/// `mark_for` resolves the current mark for a mint (the scheduler passes the
/// module's mark pipeline). Orders whose mark cannot be resolved are LEFT
/// ALONE (a missing price is not a trigger and not an error). Expired
/// orders are moved to `Expired` as housekeeping.
pub async fn run_once<S: LimitOrderStore + ?Sized, F>(
    store: &S,
    organization_id: &str,
    now: DateTime<Utc>,
    mut mark_for: F,
) -> BotResult<Vec<TriggeredOrder>>
where
    F: FnMut(&str) -> Option<f64>,
{
    let orders = store.list_active(organization_id).await?;
    let mut triggered = Vec::new();
    for mut order in orders {
        // Expiry housekeeping first — an expired order can never fire, even
        // if its price condition holds.
        if let Some(expires_at) = order.expires_at {
            if now >= expires_at {
                store
                    .set_status(organization_id, &order.id, OrderStatus::Expired)
                    .await?;
                continue;
            }
        }
        let Some(mark) = mark_for(&order.mint) else {
            continue; // unpriceable: keep waiting
        };
        if evaluate(&order, mark) {
            store
                .set_status(organization_id, &order.id, OrderStatus::Triggered)
                .await?;
            order.status = OrderStatus::Triggered;
            triggered.push(TriggeredOrder {
                order,
                fired_at_price_sol: mark,
            });
        }
    }
    Ok(triggered)
}

// ---------------------------------------------------------------------------
// In-memory store (tests + operator-mode local runs)
// ---------------------------------------------------------------------------

/// Thread-safe in-memory [`LimitOrderStore`].
#[derive(Default)]
pub struct InMemoryLimitOrderStore {
    inner: Mutex<HashMap<String, Vec<LimitOrder>>>,
}

#[async_trait]
impl LimitOrderStore for InMemoryLimitOrderStore {
    async fn insert(&self, order: &LimitOrder) -> BotResult<String> {
        order.validate()?;
        let mut inner = self.inner.lock().await;
        let orders = inner.entry(order.organization_id.clone()).or_default();
        let id = if order.id.is_empty() {
            let id = format!("lo-{}-{}", orders.len() + 1, order.mint);
            id
        } else {
            if orders.iter().any(|o| o.id == order.id) {
                return Err(BotError::invalid(format!(
                    "limit order id {} already exists",
                    order.id
                )));
            }
            order.id.clone()
        };
        let mut stored = order.clone();
        stored.id = id.clone();
        orders.push(stored);
        Ok(id)
    }

    async fn list_active(&self, organization_id: &str) -> BotResult<Vec<LimitOrder>> {
        let inner = self.inner.lock().await;
        Ok(inner
            .get(organization_id)
            .map(|orders| orders.iter().filter(|o| o.status.open()).cloned().collect())
            .unwrap_or_default())
    }

    async fn set_status(&self, organization_id: &str, order_id: &str, status: OrderStatus) -> BotResult<()> {
        let mut inner = self.inner.lock().await;
        let orders = inner
            .get_mut(organization_id)
            .ok_or_else(|| BotError::invalid("limit order: unknown organization"))?;
        let order = orders
            .iter_mut()
            .find(|o| o.id == order_id)
            .ok_or_else(|| BotError::invalid(format!("limit order {order_id} not found")))?;
        order.status = status;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(side: OrderSide, trigger: TriggerKind, price: f64, amount: f64) -> LimitOrder {
        LimitOrder {
            id: String::new(),
            organization_id: "org-1".into(),
            mint: "MintX".into(),
            side,
            trigger,
            price_sol: price,
            amount,
            status: OrderStatus::Active,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    #[test]
    fn validation_rejects_degenerate_orders() {
        assert!(order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.5, 1.0).validate().is_ok());
        assert!(order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.0, 1.0).validate().is_err());
        assert!(order(OrderSide::Buy, TriggerKind::AtOrBelow, f64::NAN, 1.0).validate().is_err());
        assert!(order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.5, -1.0).validate().is_err());
        assert!(
            order(OrderSide::Sell, TriggerKind::AtOrAbove, 0.5, 1.5).validate().is_err(),
            "sell fraction > 1 rejected"
        );
    }

    #[test]
    fn trigger_math_is_directional_and_fail_closed() {
        let dip = order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.5, 1.0);
        assert!(evaluate(&dip, 0.5), "at the price fires");
        assert!(evaluate(&dip, 0.4));
        assert!(!evaluate(&dip, 0.6));
        let chase = order(OrderSide::Sell, TriggerKind::AtOrAbove, 2.0, 1.0);
        assert!(evaluate(&chase, 2.0));
        assert!(evaluate(&chase, 3.0));
        assert!(!evaluate(&chase, 1.9));
        assert!(!evaluate(&dip, f64::NAN), "bad mark never fires");
        assert!(!evaluate(&dip, 0.0), "non-positive mark never fires");
    }

    #[tokio::test]
    async fn run_once_triggers_satisfied_orders_and_marks_them() {
        let store = InMemoryLimitOrderStore::default();
        let id1 = store.insert(&order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.5, 1.0)).await.unwrap();
        let _id2 = store.insert(&order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.1, 1.0)).await.unwrap();
        let fired = run_once(&store, "org-1", Utc::now(), |mint| {
            assert_eq!(mint, "MintX");
            Some(0.4)
        })
        .await
        .unwrap();
        assert_eq!(fired.len(), 1, "only the order at/above the mark fires");
        assert_eq!(fired[0].order.id, id1);
        assert!((fired[0].fired_at_price_sol - 0.4).abs() < 1e-12);
        // Triggered order is no longer listed as active.
        assert_eq!(store.list_active("org-1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unpriceable_mints_are_skipped_not_errored() {
        let store = InMemoryLimitOrderStore::default();
        store.insert(&order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.5, 1.0)).await.unwrap();
        let fired = run_once(&store, "org-1", Utc::now(), |_| None).await.unwrap();
        assert!(fired.is_empty());
        assert_eq!(store.list_active("org-1").await.unwrap().len(), 1, "still waiting");
    }

    #[tokio::test]
    async fn expired_orders_are_housekept_and_never_fire() {
        let store = InMemoryLimitOrderStore::default();
        let mut o = order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.5, 1.0);
        o.expires_at = Some(Utc::now() - chrono::Duration::seconds(10));
        store.insert(&o).await.unwrap();
        let fired = run_once(&store, "org-1", Utc::now(), |_| Some(0.1))
            .await
            .unwrap();
        assert!(fired.is_empty(), "expired order cannot fire even in range");
        assert!(store.list_active("org-1").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stores_are_org_scoped() {
        let store = InMemoryLimitOrderStore::default();
        let mut o = order(OrderSide::Buy, TriggerKind::AtOrBelow, 0.5, 1.0);
        o.organization_id = "org-A".into();
        store.insert(&o).await.unwrap();
        assert!(store.list_active("org-B").await.unwrap().is_empty());
        assert_eq!(store.list_active("org-A").await.unwrap().len(), 1);
        assert!(
            store.set_status("org-B", "whatever", OrderStatus::Cancelled).await.is_err(),
            "unknown org is an error, not a silent no-op"
        );
    }
}
