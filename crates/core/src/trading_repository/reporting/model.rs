//! Tenant reporting view models (PROMPT 3/10 §I53).
//!
//! Every aggregate here is computed tenant-scoped IN SQL (`WHERE
//! organization_id = $1` inside the aggregate query) — never a
//! post-load filter over global rows.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// Order-count breakdown by status for one tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantOrderStatusCounts {
    pub organization_id: OrganizationId,
    pub status: String,
    pub count: i64,
}

/// The acting tenant's order summary (counts + last activity).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantOrderSummary {
    pub organization_id: OrganizationId,
    pub total: i64,
    pub open: i64,
    pub filled: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub other: i64,
    pub first_created_at: Option<DateTime<Utc>>,
    pub last_created_at: Option<DateTime<Utc>>,
}

/// The acting tenant's position book summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPositionSummary {
    pub organization_id: OrganizationId,
    pub open_count: i64,
    pub closing_count: i64,
    pub closed_count: i64,
    pub open_cost_basis: f64,
    pub realized_quote_total: f64,
}

/// Per-symbol PnL rollup for one tenant (SQL aggregate, not Rust).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantSymbolPnl {
    pub organization_id: OrganizationId,
    pub symbol: String,
    pub closed_trades: i64,
    pub realized_quote: f64,
    pub fees: f64,
}

/// The acting tenant's realized PnL over a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantRealizedPnl {
    pub organization_id: OrganizationId,
    /// `SUM(realized_quote - cost_basis)` over positions closed in
    /// the window (matches the risk oracle's realized_today shape,
    /// tenant-scoped).
    pub realized_net: f64,
    pub positions_closed: i64,
    pub fees_paid: f64,
}

/// The acting tenant's execution throughput in a window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantExecutionStats {
    pub organization_id: OrganizationId,
    pub attempts: i64,
    pub succeeded: i64,
    pub failed: i64,
    pub submitted_transactions: i64,
    pub in_flight_transactions: i64,
}

/// Top-level report row the data plane serves at `/reports/summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantTradingSummary {
    pub organization_id: OrganizationId,
    pub generated_at: DateTime<Utc>,
    pub orders: TenantOrderSummary,
    pub positions: TenantPositionSummary,
    pub executions: TenantExecutionStats,
    pub realized_pnl: TenantRealizedPnl,
}

fn org_of(row: &PgRow) -> OrganizationId {
    row.try_get::<Option<uuid::Uuid>, _>("organization_id")
        .ok()
        .flatten()
        .map(OrganizationId::from)
        .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil()))
}

pub(super) fn order_status_count_from_row(row: &PgRow) -> TenantOrderStatusCounts {
    TenantOrderStatusCounts {
        organization_id: org_of(row),
        status: row.try_get("status").unwrap_or_default(),
        count: row.try_get("count").unwrap_or_default(),
    }
}

pub(super) fn symbol_pnl_from_row(row: &PgRow) -> TenantSymbolPnl {
    TenantSymbolPnl {
        organization_id: org_of(row),
        symbol: row.try_get("symbol").unwrap_or_default(),
        closed_trades: row.try_get("closed_trades").unwrap_or_default(),
        realized_quote: row.try_get("realized_quote").unwrap_or_default(),
        fees: row.try_get("fees").unwrap_or_default(),
    }
}
