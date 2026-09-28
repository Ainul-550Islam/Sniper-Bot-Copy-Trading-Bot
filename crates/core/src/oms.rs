//! Order management system (BUILD PLAN §7 / OMS layer).
//!
//! A normalized order lifecycle shared by every execution venue:
//!
//! ```text
//! Created → Validated → Queued → Submitted → Accepted → PartiallyFilled → Filled
//!              │           │         │           │            │
//!              └───────────┴─────────┴───────────┴────────────┴──→ Failed
//!                                            │
//!                              Cancelled / Expired / Unknown → Reconciled
//! ```
//!
//! * Every order has a **stable internal id** (`ord_…`) plus, when known, the
//!   external identifiers (chain `signature`, provider `external_id`).
//! * Every order carries a **deterministic idempotency key** derived from the
//!   intent (module + feed event + side), so a restart or a duplicate signal
//!   can never double-submit: [`OrderManager::create`] returns the existing
//!   order for a repeated key instead of creating a second one.
//! * Transitions are validated against the state machine
//!   ([`OrderStatus::can_transition_to`]); illegal moves are rejected loudly.
//! * With a database attached, every order, transition and execution attempt
//!   is durable and survives restarts; without one the manager is an
//!   in-memory, bounded ledger (behaviour identical to the pre-OMS app).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::db::Database;
use crate::error::{BotError, BotResult};
use crate::models::{BotModule, ExecutionMode};

/// Normalized order lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Created,
    Validated,
    Queued,
    Submitted,
    Accepted,
    PartiallyFilled,
    Filled,
    Failed,
    Cancelled,
    Expired,
    /// External state could not be determined (e.g. restart after an
    /// ambiguous send). Reconciliation must resolve it.
    Unknown,
    /// Resolved by the reconciler against external truth.
    Reconciled,
}

impl OrderStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OrderStatus::Created => "created",
            OrderStatus::Validated => "validated",
            OrderStatus::Queued => "queued",
            OrderStatus::Submitted => "submitted",
            OrderStatus::Accepted => "accepted",
            OrderStatus::PartiallyFilled => "partially_filled",
            OrderStatus::Filled => "filled",
            OrderStatus::Failed => "failed",
            OrderStatus::Cancelled => "cancelled",
            OrderStatus::Expired => "expired",
            OrderStatus::Unknown => "unknown",
            OrderStatus::Reconciled => "reconciled",
        }
    }

    pub fn parse(s: &str) -> Option<OrderStatus> {
        Some(match s {
            "created" => OrderStatus::Created,
            "validated" => OrderStatus::Validated,
            "queued" => OrderStatus::Queued,
            "submitted" => OrderStatus::Submitted,
            "accepted" => OrderStatus::Accepted,
            "partially_filled" => OrderStatus::PartiallyFilled,
            "filled" => OrderStatus::Filled,
            "failed" => OrderStatus::Failed,
            "cancelled" => OrderStatus::Cancelled,
            "expired" => OrderStatus::Expired,
            "unknown" => OrderStatus::Unknown,
            "reconciled" => OrderStatus::Reconciled,
            _ => return None,
        })
    }

    /// Terminal states end the lifecycle; nothing may leave them.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            OrderStatus::Filled
                | OrderStatus::Failed
                | OrderStatus::Cancelled
                | OrderStatus::Expired
                | OrderStatus::Reconciled
        )
    }

    /// The legal state machine. Deliberately permissive about *forward*
    /// progress (venues skip states — a market order goes Submitted → Filled)
    /// and strict about nonsense (nothing leaves a terminal state, Filled
    /// never regresses to Accepted, …). `Unknown` is reachable from any
    /// non-terminal state (ambiguous crash) and may only go to a terminal
    /// state or `Reconciled`.
    pub fn can_transition_to(&self, next: OrderStatus) -> bool {
        use OrderStatus::*;
        if self.is_terminal() {
            return false;
        }
        match (self, next) {
            (Unknown, t) => t.is_terminal() || t == Reconciled,
            (_, Unknown) => true,
            (Created, Validated | Queued | Submitted | Failed | Cancelled | Expired) => true,
            (Validated, Queued | Submitted | Failed | Cancelled | Expired) => true,
            (Queued, Submitted | Failed | Cancelled | Expired) => true,
            (Submitted, Accepted | PartiallyFilled | Filled | Failed | Cancelled | Expired) => true,
            (Accepted, PartiallyFilled | Filled | Failed | Cancelled | Expired) => true,
            (PartiallyFilled, PartiallyFilled | Filled | Failed | Cancelled | Expired) => true,
            (_, Reconciled) => true,
            _ => false,
        }
    }
}

/// A normalized order. `meta` carries venue-specific payload (bounded by the
/// producers; never secrets).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    /// Deterministic intent key; duplicates collapse onto one order.
    pub idempotency_key: String,
    pub module: BotModule,
    pub side: String,
    pub symbol: String,
    pub venue: String,
    pub mode: ExecutionMode,
    pub status: OrderStatus,
    pub qty: f64,
    pub price: Option<f64>,
    pub external_id: Option<String>,
    pub signature: Option<String>,
    pub error: Option<String>,
    pub meta: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl Order {
    /// Human-readable one-liner for logs/events (no secrets by construction).
    pub fn summary(&self) -> String {
        format!(
            "{} {} {} {} {} qty={} status={}{}",
            self.id,
            self.module.as_str(),
            self.side,
            self.symbol,
            self.venue,
            self.qty,
            self.status.as_str(),
            self.signature
                .as_deref()
                .map(|s| format!(" sig={s}"))
                .unwrap_or_default()
        )
    }
}

/// One lifecycle attempt/observation against an order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub order_id: String,
    pub ts: DateTime<Utc>,
    pub kind: String,
    pub endpoint: Option<String>,
    pub latency_ms: Option<u64>,
    pub ok: bool,
    pub detail: Option<String>,
}

/// Draft used to create an order. `id` and timestamps are assigned by the
/// manager.
#[derive(Debug, Clone)]
pub struct OrderDraft {
    pub idempotency_key: String,
    pub module: BotModule,
    pub side: String,
    pub symbol: String,
    pub venue: String,
    pub mode: ExecutionMode,
    pub qty: f64,
    pub price: Option<f64>,
    pub meta: serde_json::Value,
}

/// The order ledger: in-memory mirror + optional Postgres durability.
pub struct OrderManager {
    db: Option<Arc<Database>>,
    orders: RwLock<HashMap<String, Order>>,
    by_idem: RwLock<HashMap<String, String>>,
    /// FIFO cap so a long run without DB cannot grow unbounded.
    order: RwLock<VecDeque<String>>,
    cap: usize,
    id_counter: std::sync::atomic::AtomicU64,
}

impl OrderManager {
    /// `cap` bounds the in-memory mirror (eviction drops the OLDEST
    /// *terminal* orders first; live orders are never evicted).
    pub fn new(db: Option<Arc<Database>>, cap: usize) -> Arc<Self> {
        Arc::new(OrderManager {
            db,
            orders: RwLock::new(HashMap::new()),
            by_idem: RwLock::new(HashMap::new()),
            order: RwLock::new(VecDeque::new()),
            cap: cap.max(64),
            id_counter: std::sync::atomic::AtomicU64::new(0),
        })
    }

    fn next_id(&self) -> String {
        let n = self
            .id_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("ord_{}_{}", Utc::now().timestamp_millis(), n)
    }

    /// Create an order, or return the existing one for the same idempotency
    /// key (duplicate intent). Persistence failures are logged + metered but
    /// do not fail the create — the in-memory ledger stays authoritative for
    /// the running process (graceful degradation).
    pub async fn create(&self, draft: OrderDraft) -> BotResult<Order> {
        if draft.idempotency_key.trim().is_empty() {
            return Err(BotError::InvalidArgument(
                "order idempotency_key must not be empty".into(),
            ));
        }
        // Fast duplicate check.
        if let Some(existing) = self.get_by_key(&draft.idempotency_key).await {
            debug!(key = %draft.idempotency_key, "duplicate order intent — returning existing");
            crate::obs::metrics::global()
                .counter(
                    "bot_duplicate_execution_prevented_total",
                    "Duplicate logical executions prevented by idempotency layers.",
                    &[("where", "oms_memory")],
                )
                .inc();
            return Ok(existing);
        }
        let now = Utc::now();
        let order = Order {
            id: self.next_id(),
            idempotency_key: draft.idempotency_key.clone(),
            module: draft.module,
            side: draft.side,
            symbol: draft.symbol,
            venue: draft.venue,
            mode: draft.mode,
            status: OrderStatus::Created,
            qty: draft.qty,
            price: draft.price,
            external_id: None,
            signature: None,
            error: None,
            meta: draft.meta,
            created_at: now,
            updated_at: now,
            submitted_at: None,
            finished_at: None,
        };

        if let Some(db) = &self.db {
            // Race-safe: the DB unique key is the final arbiter. A conflicting
            // row means another task/process created the same intent first.
            match crate::db::repo::OrderRepo::new(db.clone())
                .insert_if_absent(&order)
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    if let Some(existing) = crate::db::repo::OrderRepo::new(db.clone())
                        .get_by_key(&order.idempotency_key)
                        .await
                        .ok()
                        .flatten()
                    {
                        crate::obs::metrics::global()
                            .counter(
                                "bot_duplicate_execution_prevented_total",
                                "Duplicate logical executions prevented by idempotency layers.",
                                &[("where", "oms_db")],
                            )
                            .inc();
                        self.insert_mirror(existing.clone()).await;
                        return Ok(existing);
                    }
                }
                Err(e) => warn!(error = %e, "order persistence failed — continuing in memory"),
            }
        }

        self.insert_mirror(order.clone()).await;
        info!(order = %order.summary(), "order created");
        Ok(order)
    }

    /// Validate + apply a state transition. Illegal transitions are errors
    /// (programming bugs must be loud). `reason` lands in the status history.
    pub async fn transition(
        &self,
        id: &str,
        to: OrderStatus,
        reason: Option<&str>,
    ) -> BotResult<Order> {
        let mut orders = self.orders.write().await;
        let order = orders
            .get_mut(id)
            .ok_or_else(|| BotError::NotFound(format!("order {id}")))?;
        if !order.status.can_transition_to(to) {
            return Err(BotError::InvalidArgument(format!(
                "illegal order transition {} -> {} for {id}",
                order.status.as_str(),
                to.as_str()
            )));
        }
        let from = order.status;
        let now = Utc::now();
        order.status = to;
        order.updated_at = now;
        if to == OrderStatus::Submitted && order.submitted_at.is_none() {
            order.submitted_at = Some(now);
        }
        if to.is_terminal() {
            order.finished_at = Some(now);
        }
        if let Some(r) = reason {
            order.error = if to == OrderStatus::Failed {
                Some(r.to_string())
            } else {
                order.error.clone()
            };
        }
        let snapshot = order.clone();
        drop(orders);

        if let Some(db) = &self.db {
            let repo = crate::db::repo::OrderRepo::new(db.clone());
            if let Err(e) = repo.set_status(&snapshot, from, reason).await {
                warn!(error = %e, order = id, "order transition persistence failed");
            }
        }
        debug!(order = %snapshot.summary(), from = from.as_str(), "order transition");
        Ok(snapshot)
    }

    /// Record the external identifiers once a venue acknowledges a send.
    pub async fn attach_external(
        &self,
        id: &str,
        external_id: Option<String>,
        signature: Option<String>,
    ) -> BotResult<Order> {
        let mut orders = self.orders.write().await;
        let order = orders
            .get_mut(id)
            .ok_or_else(|| BotError::NotFound(format!("order {id}")))?;
        if let Some(x) = external_id {
            order.external_id = Some(x);
        }
        if let Some(s) = signature {
            order.signature = Some(s);
        }
        order.updated_at = Utc::now();
        let snapshot = order.clone();
        drop(orders);
        if let Some(db) = &self.db {
            let repo = crate::db::repo::OrderRepo::new(db.clone());
            if let Err(e) = repo.update_external(&snapshot).await {
                warn!(error = %e, order = id, "order external-id persistence failed");
            }
        }
        Ok(snapshot)
    }

    /// Append one execution attempt/observation.
    pub async fn record_execution(&self, rec: ExecutionRecord) {
        if let Some(db) = &self.db {
            let repo = crate::db::repo::OrderRepo::new(db.clone());
            if let Err(e) = repo.append_execution(&rec).await {
                debug!(error = %e, "execution record persistence failed");
            }
        }
    }

    pub async fn get(&self, id: &str) -> Option<Order> {
        self.orders.read().await.get(id).cloned()
    }

    pub async fn get_by_key(&self, key: &str) -> Option<Order> {
        let id = self.by_idem.read().await.get(key).cloned()?;
        self.get(&id).await
    }

    pub async fn list(&self, limit: usize) -> Vec<Order> {
        let orders = self.orders.read().await;
        let mut all: Vec<Order> = orders.values().cloned().collect();
        all.sort_by_key(|o| std::cmp::Reverse(o.created_at));
        all.truncate(limit);
        all
    }

    /// Non-terminal orders — the recovery/reconciliation input after a
    /// restart. Prefers the database (durable truth) when attached.
    pub async fn incomplete(&self) -> Vec<Order> {
        if let Some(db) = &self.db {
            let repo = crate::db::repo::OrderRepo::new(db.clone());
            if let Ok(rows) = repo.list_incomplete().await {
                return rows;
            }
            warn!("incomplete-order query failed — falling back to the memory mirror");
        }
        self.orders
            .read()
            .await
            .values()
            .filter(|o| !o.status.is_terminal())
            .cloned()
            .collect()
    }

    /// Restart recovery: load every non-terminal order from the database into
    /// the mirror and mark it `Unknown` (its external state is unverified
    /// until reconciliation says otherwise). Idempotent.
    pub async fn recover_from_db(&self) -> BotResult<usize> {
        let Some(db) = &self.db else {
            return Ok(0);
        };
        let repo = crate::db::repo::OrderRepo::new(db.clone());
        let rows = repo.list_incomplete().await?;
        let mut n = 0;
        for mut o in rows {
            // Every non-terminal order in durable storage is suspect after a
            // restart — even one a duplicate-create just re-inserted into the
            // mirror from its stale DB row. Terminal orders are immutable and
            // never listed here; live in-process orders do not exist across a
            // restart (this runs before modules spawn).
            o.status = OrderStatus::Unknown;
            o.updated_at = Utc::now();
            repo.set_status(&o, o.status, Some("restart recovery"))
                .await
                .ok();
            self.insert_mirror(o).await;
            n += 1;
        }
        if n > 0 {
            info!(
                recovered = n,
                "orders recovered from the database as Unknown"
            );
        }
        Ok(n)
    }

    async fn insert_mirror(&self, order: Order) {
        let mut orders = self.orders.write().await;
        let mut by_idem = self.by_idem.write().await;
        let mut fifo = self.order.write().await;
        let is_new = !orders.contains_key(&order.id);
        let id = order.id.clone();
        by_idem.insert(order.idempotency_key.clone(), id.clone());
        orders.insert(id.clone(), order);
        if is_new {
            fifo.push_back(id);
        }
        // Bound the mirror: evict the oldest TERMINAL orders until under the
        // cap. Live orders are never evicted (if all are live the mirror may
        // temporarily exceed the cap — correctness beats the bound).
        while fifo.len() > self.cap {
            let Some(pos) = fifo.iter().position(|id| {
                orders
                    .get(id.as_str())
                    .map(|o| o.status.is_terminal())
                    .unwrap_or(true)
            }) else {
                break;
            };
            let evicted = fifo.remove(pos).expect("pos came from the same vec");
            if let Some(removed) = orders.remove(&evicted) {
                by_idem.remove(&removed.idempotency_key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(key: &str) -> OrderDraft {
        OrderDraft {
            idempotency_key: key.into(),
            module: BotModule::Sniper,
            side: "buy".into(),
            symbol: "TEST".into(),
            venue: "pump".into(),
            mode: ExecutionMode::Paper,
            qty: 1.0,
            price: None,
            meta: serde_json::json!({}),
        }
    }

    #[test]
    fn status_machine_allows_forward_progress_and_blocks_nonsense() {
        use OrderStatus::*;
        assert!(Created.can_transition_to(Validated));
        assert!(Created.can_transition_to(Submitted)); // venues skip states
        assert!(Submitted.can_transition_to(Filled));
        assert!(Submitted.can_transition_to(Accepted));
        assert!(Accepted.can_transition_to(PartiallyFilled));
        assert!(PartiallyFilled.can_transition_to(Filled));
        assert!(Submitted.can_transition_to(Unknown)); // ambiguous crash
        assert!(Unknown.can_transition_to(Filled));
        assert!(Unknown.can_transition_to(Reconciled));
        // Illegal moves.
        assert!(!Filled.can_transition_to(Accepted), "terminal never moves");
        assert!(!Filled.can_transition_to(Failed));
        assert!(!Failed.can_transition_to(Filled));
        assert!(
            !Created.can_transition_to(PartiallyFilled),
            "no fills before submission"
        );
        assert!(!Validated.can_transition_to(Accepted));
    }

    #[test]
    fn status_round_trips_through_wire_names() {
        for s in [
            OrderStatus::Created,
            OrderStatus::PartiallyFilled,
            OrderStatus::Unknown,
            OrderStatus::Reconciled,
        ] {
            assert_eq!(OrderStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(OrderStatus::parse("nonsense"), None);
    }

    #[tokio::test]
    async fn create_is_idempotent_per_key() {
        let mgr = OrderManager::new(None, 64);
        let a = mgr.create(draft("intent-1")).await.unwrap();
        let b = mgr.create(draft("intent-1")).await.unwrap();
        assert_eq!(a.id, b.id, "duplicate intent collapses onto one order");
        let c = mgr.create(draft("intent-2")).await.unwrap();
        assert_ne!(a.id, c.id);
        assert_eq!(mgr.list(10).await.len(), 2);
    }

    #[tokio::test]
    async fn duplicate_intent_increments_prevention_metric() {
        // §G/§T: duplicate suppression must be observable (low-cardinality
        // counter, no keys/ids as labels).
        let counter = crate::obs::metrics::global().counter(
            "bot_duplicate_execution_prevented_total",
            "Duplicate logical executions prevented by idempotency layers.",
            &[("where", "oms_memory")],
        );
        let before = counter.get();
        let mgr = OrderManager::new(None, 64);
        let a = mgr.create(draft("dup-metric-intent")).await.unwrap();
        let b = mgr.create(draft("dup-metric-intent")).await.unwrap();
        assert_eq!(a.id, b.id);
        assert!(
            counter.get() > before,
            "duplicate create must meter the prevention"
        );
    }

    #[tokio::test]
    async fn empty_idempotency_key_is_rejected() {
        let mgr = OrderManager::new(None, 64);
        let err = mgr.create(draft("  ")).await.unwrap_err();
        assert!(matches!(err, BotError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn transitions_update_timestamps_and_error() {
        let mgr = OrderManager::new(None, 64);
        let o = mgr.create(draft("t")).await.unwrap();
        let o = mgr
            .transition(&o.id, OrderStatus::Submitted, None)
            .await
            .unwrap();
        assert!(o.submitted_at.is_some());
        assert!(o.finished_at.is_none());
        let err = mgr
            .transition(&o.id, OrderStatus::Failed, Some("boom"))
            .await
            .unwrap();
        assert_eq!(err.error.as_deref(), Some("boom"));
        assert!(
            err.finished_at.is_some(),
            "terminal states stamp finish time"
        );
        // Terminal: no further moves.
        assert!(mgr
            .transition(&o.id, OrderStatus::Filled, None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn illegal_transition_is_a_loud_error() {
        let mgr = OrderManager::new(None, 64);
        let o = mgr.create(draft("x")).await.unwrap();
        let err = mgr
            .transition(&o.id, OrderStatus::PartiallyFilled, None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("illegal order transition"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn attach_external_records_ids() {
        let mgr = OrderManager::new(None, 64);
        let o = mgr.create(draft("e")).await.unwrap();
        let o = mgr
            .attach_external(&o.id, Some("ext-9".into()), Some("sig-9".into()))
            .await
            .unwrap();
        assert_eq!(o.external_id.as_deref(), Some("ext-9"));
        assert_eq!(o.signature.as_deref(), Some("sig-9"));
    }

    #[tokio::test]
    async fn mirror_eviction_keeps_live_orders() {
        let mgr = OrderManager::new(None, 64);
        // 80 terminal orders + 5 live ones; cap is 64.
        for i in 0..80 {
            let o = mgr.create(draft(&format!("k{i}"))).await.unwrap();
            mgr.transition(&o.id, OrderStatus::Failed, None)
                .await
                .unwrap();
        }
        let mut live = Vec::new();
        for i in 0..5 {
            live.push(mgr.create(draft(&format!("live{i}"))).await.unwrap());
        }
        let all = mgr.list(1000).await;
        assert!(all.len() <= 69, "mirror stays bounded: {}", all.len());
        for l in &live {
            assert!(
                mgr.get(&l.id).await.is_some(),
                "live orders are never evicted"
            );
        }
    }

    #[tokio::test]
    async fn incomplete_lists_only_non_terminal() {
        let mgr = OrderManager::new(None, 64);
        let a = mgr.create(draft("a")).await.unwrap();
        let b = mgr.create(draft("b")).await.unwrap();
        mgr.transition(&b.id, OrderStatus::Failed, None)
            .await
            .unwrap();
        let inc = mgr.incomplete().await;
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].id, a.id);
    }
}
