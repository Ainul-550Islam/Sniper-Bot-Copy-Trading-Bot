//! Tenant position/trade/balance models (PROMPT 3/10 §D23).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// One `positions` row, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPosition {
    pub organization_id: OrganizationId,
    pub id: String,
    pub source: String,
    pub venue: String,
    pub mode: String,
    /// `open` | `closing` | `closed` | `stopped_out` | `failed`.
    pub status: String,
    pub symbol: String,
    pub symbol_display: String,
    pub quote_symbol: String,
    pub qty: f64,
    pub avg_entry: f64,
    pub cost_basis: f64,
    pub realized_quote: f64,
    pub last_mark: f64,
    pub stop_loss: Option<f64>,
    pub take_profit: Option<f64>,
    pub trailing_stop: Option<f64>,
    pub trailing_high_water: Option<f64>,
    pub max_hold_secs: Option<i64>,
    pub entry_signature: Option<String>,
    pub exit_signature: Option<String>,
    pub entry_latency_ms: Option<i64>,
    pub copied_wallet: Option<String>,
    pub market_id: Option<String>,
    pub outcome: Option<String>,
    pub reason_closed: Option<String>,
    pub opened_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantPosition {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One `trades` row, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantTrade {
    pub organization_id: OrganizationId,
    pub id: String,
    pub ts: DateTime<Utc>,
    pub source: String,
    pub venue: String,
    pub mode: String,
    /// `long` | `short`.
    pub side: String,
    pub symbol: String,
    pub symbol_display: String,
    pub amount_in: f64,
    pub amount_out: f64,
    pub quote_symbol: String,
    pub price: f64,
    pub fee: f64,
    pub slippage_bps: i64,
    pub signature: Option<String>,
    pub position_id: Option<String>,
    pub note: Option<String>,
    pub latency_ms: Option<i64>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantTrade {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One `balance_snapshots` row, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantBalanceSnapshot {
    pub organization_id: OrganizationId,
    pub id: i64,
    pub ts: DateTime<Utc>,
    pub chain: String,
    pub address: String,
    pub asset: String,
    pub amount: f64,
    pub usd_value: Option<f64>,
    pub source: String,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantBalanceSnapshot {
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

pub(super) fn position_from_row(row: &PgRow) -> TenantPosition {
    TenantPosition {
        organization_id: org_of(row),
        id: row.try_get("id").unwrap_or_default(),
        source: row.try_get("source").unwrap_or_default(),
        venue: row.try_get("venue").unwrap_or_default(),
        mode: row.try_get("mode").unwrap_or_default(),
        status: row.try_get("status").unwrap_or_default(),
        symbol: row.try_get("symbol").unwrap_or_default(),
        symbol_display: row.try_get("symbol_display").unwrap_or_default(),
        quote_symbol: row.try_get("quote_symbol").unwrap_or_default(),
        qty: row.try_get("qty").unwrap_or_default(),
        avg_entry: row.try_get("avg_entry").unwrap_or_default(),
        cost_basis: row.try_get("cost_basis").unwrap_or_default(),
        realized_quote: row.try_get("realized_quote").unwrap_or_default(),
        last_mark: row.try_get("last_mark").unwrap_or_default(),
        stop_loss: row.try_get("stop_loss").ok().flatten(),
        take_profit: row.try_get("take_profit").ok().flatten(),
        trailing_stop: row.try_get("trailing_stop").ok().flatten(),
        trailing_high_water: row.try_get("trailing_high_water").ok().flatten(),
        max_hold_secs: row
            .try_get::<Option<i64>, _>("max_hold_secs")
            .ok()
            .flatten(),
        entry_signature: row.try_get("entry_signature").ok().flatten(),
        exit_signature: row.try_get("exit_signature").ok().flatten(),
        entry_latency_ms: row
            .try_get::<Option<i64>, _>("entry_latency_ms")
            .ok()
            .flatten(),
        copied_wallet: row.try_get("copied_wallet").ok().flatten(),
        market_id: row.try_get("market_id").ok().flatten(),
        outcome: row.try_get("outcome").ok().flatten(),
        reason_closed: row.try_get("reason_closed").ok().flatten(),
        opened_at: row
            .try_get::<DateTime<Utc>, _>("opened_at")
            .unwrap_or_else(|_| Utc::now()),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
        closed_at: row
            .try_get::<Option<DateTime<Utc>>, _>("closed_at")
            .ok()
            .flatten(),
    }
}

pub(super) fn trade_from_row(row: &PgRow) -> TenantTrade {
    TenantTrade {
        organization_id: org_of(row),
        id: row.try_get("id").unwrap_or_default(),
        ts: row
            .try_get::<DateTime<Utc>, _>("ts")
            .unwrap_or_else(|_| Utc::now()),
        source: row.try_get("source").unwrap_or_default(),
        venue: row.try_get("venue").unwrap_or_default(),
        mode: row.try_get("mode").unwrap_or_default(),
        side: row.try_get("side").unwrap_or_default(),
        symbol: row.try_get("symbol").unwrap_or_default(),
        symbol_display: row.try_get("symbol_display").unwrap_or_default(),
        amount_in: row.try_get("amount_in").unwrap_or_default(),
        amount_out: row.try_get("amount_out").unwrap_or_default(),
        quote_symbol: row.try_get("quote_symbol").unwrap_or_default(),
        price: row.try_get("price").unwrap_or_default(),
        fee: row.try_get("fee").unwrap_or_default(),
        slippage_bps: row
            .try_get::<Option<i64>, _>("slippage_bps")
            .ok()
            .flatten()
            .unwrap_or(0),
        signature: row.try_get("signature").ok().flatten(),
        position_id: row.try_get("position_id").ok().flatten(),
        note: row.try_get("note").ok().flatten(),
        latency_ms: row.try_get::<Option<i64>, _>("latency_ms").ok().flatten(),
    }
}

pub(super) fn balance_from_row(row: &PgRow) -> TenantBalanceSnapshot {
    TenantBalanceSnapshot {
        organization_id: org_of(row),
        id: row.try_get("id").unwrap_or_default(),
        ts: row
            .try_get::<DateTime<Utc>, _>("ts")
            .unwrap_or_else(|_| Utc::now()),
        chain: row.try_get("chain").unwrap_or_default(),
        address: row.try_get("address").unwrap_or_default(),
        asset: row.try_get("asset").unwrap_or_default(),
        amount: row.try_get("amount").unwrap_or_default(),
        usd_value: row.try_get("usd_value").ok().flatten(),
        source: row.try_get("source").unwrap_or_default(),
    }
}
