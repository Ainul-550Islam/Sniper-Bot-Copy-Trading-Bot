//! Tenant execution-intent model (PROMPT 3/10 §E30).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// One `execution_intents` row (write-ahead pre-broadcast journal),
/// owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantIntent {
    pub organization_id: OrganizationId,
    pub intent_id: String,
    pub module: String,
    pub symbol: String,
    pub wallet: String,
    pub side: String,
    pub qty: String,
    /// `pending` | `submitted` | `abandoned`.
    pub status: String,
    pub signature: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantIntent {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

pub(super) fn intent_from_row(row: &PgRow) -> TenantIntent {
    TenantIntent {
        organization_id: row
            .try_get::<Option<uuid::Uuid>, _>("organization_id")
            .ok()
            .flatten()
            .map(OrganizationId::from)
            .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil())),
        intent_id: row.try_get("intent_id").unwrap_or_default(),
        module: row.try_get("module").unwrap_or_default(),
        symbol: row.try_get("symbol").unwrap_or_default(),
        wallet: row.try_get("wallet").unwrap_or_default(),
        side: row.try_get("side").unwrap_or_default(),
        qty: row.try_get("qty").unwrap_or_default(),
        status: row.try_get("status").unwrap_or_default(),
        signature: row.try_get("signature").ok().flatten(),
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .unwrap_or_else(|_| Utc::now()),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
    }
}
