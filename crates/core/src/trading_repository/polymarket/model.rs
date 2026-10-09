//! Tenant polymarket signal/order/fill/recon models (PROMPT 3/10 §G42).
//!
//! Column-faithful to migration 0014 (plus the 0023/0024
//! `organization_id`): signals key the strategy pipeline, orders key
//! the mirror book by venue order id, fills key executions by fill
//! id — each row owned by exactly one tenant since the 0030
//! composite swaps.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// One `poly_signals` row, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPolySignal {
    pub organization_id: OrganizationId,
    pub signal_id: String,
    pub condition_id: String,
    pub token_id: String,
    pub outcome: String,
    /// `buy` | `sell` (table CHECK).
    pub side: String,
    pub strategy: String,
    pub limit_price: f64,
    pub size_tokens: f64,
    pub stake_usd: f64,
    /// `paper` | `simulate` | `live` (table CHECK).
    pub mode: String,
    /// Pipeline stage (free text; progresses `observed` → … → terminal).
    pub stage: String,
    pub reject_reason: Option<String>,
    pub detail: String,
    pub order_id: Option<String>,
    pub venue_order_id: Option<String>,
    pub position_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantPolySignal {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One `poly_orders` row (mirror book), owned by a tenant. A mirror
/// order is open while `closed_at IS NULL`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPolyOrder {
    pub organization_id: OrganizationId,
    pub venue_order_id: String,
    pub order_id: String,
    pub signal_id: String,
    pub condition_id: String,
    pub token_id: String,
    pub outcome: String,
    /// `buy` | `sell` (table CHECK).
    pub side: String,
    pub order_type: String,
    pub limit_price: f64,
    pub size_tokens: f64,
    pub size_matched: f64,
    /// `paper` | `simulate` | `live` (table CHECK).
    pub mode: String,
    /// Mirror-book state (free text; terminal once `closed_at` is set).
    pub state: String,
    pub venue_status: String,
    pub expiration: i64,
    pub position_id: Option<String>,
    pub replica_id: String,
    pub submitted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantPolyOrder {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One `poly_fills` row, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPolyFill {
    pub organization_id: OrganizationId,
    pub fill_id: String,
    pub venue_order_id: String,
    pub order_id: String,
    pub token_id: String,
    /// `buy` | `sell` (table CHECK).
    pub side: String,
    pub price: f64,
    pub size_tokens: f64,
    pub quote_usd: f64,
    pub source: String,
    pub position_id: Option<String>,
    pub ts: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantPolyFill {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One `poly_recon_findings` row (append-only), owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPolyReconFinding {
    pub organization_id: OrganizationId,
    pub id: i64,
    /// `drift` | `orphan_fill` | `stuck_order` | `balance_gap`.
    pub kind: String,
    pub venue_order_id: Option<String>,
    pub order_id: Option<String>,
    pub token_id: Option<String>,
    pub detail: String,
    pub action: String,
    pub replica_id: String,
    pub ts: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantPolyReconFinding {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

fn org_of(row: &PgRow) -> OrganizationId {
    row.try_get::<Option<uuid::Uuid>, _>("organization_id")
        .ok()
        .flatten()
        .map(OrganizationId::from)
        .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil()))
}

pub(super) fn signal_from_row(row: &PgRow) -> TenantPolySignal {
    TenantPolySignal {
        organization_id: org_of(row),
        signal_id: row.try_get("signal_id").unwrap_or_default(),
        condition_id: row.try_get("condition_id").unwrap_or_default(),
        token_id: row.try_get("token_id").unwrap_or_default(),
        outcome: row.try_get("outcome").unwrap_or_default(),
        side: row.try_get("side").unwrap_or_default(),
        strategy: row.try_get("strategy").unwrap_or_default(),
        limit_price: row.try_get("limit_price").unwrap_or_default(),
        size_tokens: row.try_get("size_tokens").unwrap_or_default(),
        stake_usd: row.try_get("stake_usd").unwrap_or_default(),
        mode: row.try_get("mode").unwrap_or_default(),
        stage: row.try_get("stage").unwrap_or_default(),
        reject_reason: row.try_get("reject_reason").ok().flatten(),
        detail: row.try_get("detail").unwrap_or_default(),
        order_id: row.try_get("order_id").ok().flatten(),
        venue_order_id: row.try_get("venue_order_id").ok().flatten(),
        position_id: row.try_get("position_id").ok().flatten(),
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .unwrap_or_else(|_| Utc::now()),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
    }
}

pub(super) fn order_from_row(row: &PgRow) -> TenantPolyOrder {
    TenantPolyOrder {
        organization_id: org_of(row),
        venue_order_id: row.try_get("venue_order_id").unwrap_or_default(),
        order_id: row.try_get("order_id").unwrap_or_default(),
        signal_id: row.try_get("signal_id").unwrap_or_default(),
        condition_id: row.try_get("condition_id").unwrap_or_default(),
        token_id: row.try_get("token_id").unwrap_or_default(),
        outcome: row.try_get("outcome").unwrap_or_default(),
        side: row.try_get("side").unwrap_or_default(),
        order_type: row.try_get("order_type").unwrap_or_default(),
        limit_price: row.try_get("limit_price").unwrap_or_default(),
        size_tokens: row.try_get("size_tokens").unwrap_or_default(),
        size_matched: row.try_get("size_matched").unwrap_or_default(),
        mode: row.try_get("mode").unwrap_or_default(),
        state: row.try_get("state").unwrap_or_default(),
        venue_status: row.try_get("venue_status").unwrap_or_default(),
        expiration: row.try_get("expiration").unwrap_or_default(),
        position_id: row.try_get("position_id").ok().flatten(),
        replica_id: row.try_get("replica_id").unwrap_or_default(),
        submitted_at: row
            .try_get::<DateTime<Utc>, _>("submitted_at")
            .unwrap_or_else(|_| Utc::now()),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
        closed_at: row.try_get("closed_at").ok().flatten(),
    }
}

pub(super) fn fill_from_row(row: &PgRow) -> TenantPolyFill {
    TenantPolyFill {
        organization_id: org_of(row),
        fill_id: row.try_get("fill_id").unwrap_or_default(),
        venue_order_id: row.try_get("venue_order_id").unwrap_or_default(),
        order_id: row.try_get("order_id").unwrap_or_default(),
        token_id: row.try_get("token_id").unwrap_or_default(),
        side: row.try_get("side").unwrap_or_default(),
        price: row.try_get("price").unwrap_or_default(),
        size_tokens: row.try_get("size_tokens").unwrap_or_default(),
        quote_usd: row.try_get("quote_usd").unwrap_or_default(),
        source: row.try_get("source").unwrap_or_default(),
        position_id: row.try_get("position_id").ok().flatten(),
        ts: row
            .try_get::<DateTime<Utc>, _>("ts")
            .unwrap_or_else(|_| Utc::now()),
    }
}

pub(super) fn finding_from_row(row: &PgRow) -> TenantPolyReconFinding {
    TenantPolyReconFinding {
        organization_id: org_of(row),
        id: row.try_get("id").unwrap_or_default(),
        kind: row.try_get("kind").unwrap_or_default(),
        venue_order_id: row.try_get("venue_order_id").ok().flatten(),
        order_id: row.try_get("order_id").ok().flatten(),
        token_id: row.try_get("token_id").ok().flatten(),
        detail: row.try_get("detail").unwrap_or_default(),
        action: row.try_get("action").unwrap_or_default(),
        replica_id: row.try_get("replica_id").unwrap_or_default(),
        ts: row
            .try_get::<DateTime<Utc>, _>("ts")
            .unwrap_or_else(|_| Utc::now()),
    }
}
