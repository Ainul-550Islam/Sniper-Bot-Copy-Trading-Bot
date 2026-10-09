//! Tenant-scoped reporting (PROMPT 3/10 §I53–I58).
//!
//! One orchestrating repo assembles the per-tenant summary the data
//! plane serves; the section repos stay individually callable.

pub mod executions;
pub mod model;
pub mod orders;
pub mod pnl;
pub mod positions;

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

pub use executions::TenantExecutionReport;
pub use model::{
    TenantExecutionStats, TenantOrderStatusCounts, TenantOrderSummary, TenantPositionSummary,
    TenantRealizedPnl, TenantSymbolPnl, TenantTradingSummary,
};
pub use orders::TenantOrderReport;
pub use pnl::TenantPnlReport;
pub use positions::TenantPositionReport;

/// Orchestrating tenant reporting repository — composes the section
/// reports; every underlying aggregate is tenant-scoped in SQL.
pub struct TenantReportingRepo {
    db: Arc<Database>,
    orders: TenantOrderReport,
    positions: TenantPositionReport,
    executions: TenantExecutionReport,
    pnl: TenantPnlReport,
}

impl TenantReportingRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantReportingRepo {
            orders: TenantOrderReport::new(db.clone()),
            positions: TenantPositionReport::new(db.clone()),
            executions: TenantExecutionReport::new(db.clone()),
            pnl: TenantPnlReport::new(db.clone()),
            db,
        }
    }

    /// The acting tenant's full trading summary (dashboard payload).
    pub async fn summary(
        &self,
        scope: &TradingQueryScope,
    ) -> Result<TenantTradingSummary, RepositoryError> {
        let since = Utc::now() - chrono::Duration::days(30);
        let until = Utc::now() + chrono::Duration::days(1);
        let (orders, positions, executions, realized_pnl) = tokio::join!(
            self.orders.summary(scope),
            self.positions.summary(scope),
            self.executions.stats_between(scope, since, until),
            self.pnl.realized_between(scope, since, until),
        );
        Ok(TenantTradingSummary {
            organization_id: scope.organization_id(),
            generated_at: Utc::now(),
            orders: orders?,
            positions: positions?,
            executions: executions?,
            realized_pnl: realized_pnl?,
        })
    }

    /// Section accessors for the individual route handlers.
    pub fn orders(&self) -> &TenantOrderReport {
        &self.orders
    }

    pub fn positions(&self) -> &TenantPositionReport {
        &self.positions
    }

    pub fn executions(&self) -> &TenantExecutionReport {
        &self.executions
    }

    pub fn pnl(&self) -> &TenantPnlReport {
        &self.pnl
    }

    /// Expose the pool-backed database for callers composing ad-hoc
    /// tenant-scoped reports (the scope discipline is theirs to keep).
    pub fn database(&self) -> &Arc<Database> {
        &self.db
    }
}

/// Window helper shared by route handlers: clamp a caller-supplied
/// window to a sane maximum.
pub fn clamp_window(since: DateTime<Utc>, until: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    if until <= since {
        let s = since;
        return (s, s + chrono::Duration::days(1));
    }
    let max = chrono::Duration::days(366);
    if until - since > max {
        (until - max, until)
    } else {
        (since, until)
    }
}
