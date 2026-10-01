//! Tenant order model (PROMPT 3/10 §B9).
//!
//! [`TenantOrder`] mirrors the `orders` table (0002 + 0023/0024 tenant
//! columns) with the owning [`OrganizationId`] as a first-class field.
//! It implements [`OwnedRow`](crate::trading_repository::tenant_assert::OwnedRow)
//! so every returned row passes the runtime ownership assertion.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// One order row, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantOrder {
    pub organization_id: OrganizationId,
    pub id: String,
    pub idempotency_key: Option<String>,
    /// `sniper` | `copy` | `polymarket` | `contract` | `telegram` | `system`.
    pub module: String,
    /// `buy` | `sell` | `stake` | `unstake` | `claim` | `other`.
    pub side: String,
    pub symbol: String,
    pub venue: String,
    /// `paper` | `simulate` | `live`.
    pub mode: String,
    pub status: String,
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

impl crate::trading_repository::tenant_assert::OwnedRow for TenantOrder {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One `order_status_history` row, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantOrderStatusEntry {
    pub organization_id: OrganizationId,
    pub id: i64,
    pub order_id: String,
    pub from_status: Option<String>,
    pub to_status: String,
    pub reason: Option<String>,
    pub ts: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantOrderStatusEntry {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// Map a full-row SELECT of `orders` (which always includes
/// `organization_id` after 0023) into [`TenantOrder`].
pub(super) fn order_from_row(row: &PgRow) -> TenantOrder {
    TenantOrder {
        organization_id: row
            .try_get::<Option<uuid::Uuid>, _>("organization_id")
            .ok()
            .flatten()
            .map(OrganizationId::from)
            // NOT NULL after 0024; the fallback only guards a malformed
            // SELECT and fails the ownership assertion downstream.
            .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil())),
        id: row.try_get("id").unwrap_or_default(),
        idempotency_key: row.try_get("idempotency_key").ok().flatten(),
        module: row.try_get("module").unwrap_or_default(),
        side: row.try_get("side").unwrap_or_default(),
        symbol: row.try_get("symbol").unwrap_or_default(),
        venue: row.try_get("venue").unwrap_or_default(),
        mode: row.try_get("mode").unwrap_or_default(),
        status: row.try_get("status").unwrap_or_default(),
        qty: row.try_get("qty").unwrap_or_default(),
        price: row.try_get("price").ok().flatten(),
        external_id: row.try_get("external_id").ok().flatten(),
        signature: row.try_get("signature").ok().flatten(),
        error: row.try_get("error").ok().flatten(),
        meta: row.try_get("meta").unwrap_or(serde_json::json!({})),
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .unwrap_or_else(|_| Utc::now()),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
        submitted_at: row
            .try_get::<Option<DateTime<Utc>>, _>("submitted_at")
            .ok()
            .flatten(),
        finished_at: row
            .try_get::<Option<DateTime<Utc>>, _>("finished_at")
            .ok()
            .flatten(),
    }
}

/// Map a full-row SELECT of `order_status_history` joined with its
/// order's organization (the history table has no organization column;
/// the repository query carries `orders.organization_id` alongside).
pub(super) fn status_entry_from_row(row: &PgRow) -> TenantOrderStatusEntry {
    TenantOrderStatusEntry {
        organization_id: row
            .try_get::<Option<uuid::Uuid>, _>("organization_id")
            .ok()
            .flatten()
            .map(OrganizationId::from)
            // NOT NULL after 0024; the fallback only guards a malformed
            // SELECT and fails the ownership assertion downstream.
            .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil())),
        id: row.try_get("id").unwrap_or_default(),
        order_id: row.try_get("order_id").unwrap_or_default(),
        from_status: row.try_get("from_status").ok().flatten(),
        to_status: row.try_get("to_status").unwrap_or_default(),
        reason: row.try_get("reason").ok().flatten(),
        ts: row
            .try_get::<DateTime<Utc>, _>("ts")
            .unwrap_or_else(|_| Utc::now()),
    }
}
