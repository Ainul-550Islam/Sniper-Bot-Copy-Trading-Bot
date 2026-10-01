//! Tenant execution/transaction row models (PROMPT 3/10 §C15).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// One `executions` row (per-attempt observation against an order),
/// owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantExecution {
    pub organization_id: OrganizationId,
    pub id: i64,
    pub order_id: String,
    pub ts: DateTime<Utc>,
    /// `validate` | `simulate` | `send` | `confirm` | `cancel` |
    /// `reconcile` | `recover` | `note`.
    pub kind: String,
    pub endpoint: Option<String>,
    pub latency_ms: Option<i64>,
    pub ok: bool,
    pub detail: Option<String>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantExecution {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One `transactions` row (on-chain bookkeeping), owned by a tenant.
/// The SIGNATURE stays globally unique (chain identity, 0023 rule);
/// ownership is the org attribution + the org-first reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantTransaction {
    pub organization_id: OrganizationId,
    pub signature: String,
    pub chain: String,
    pub order_id: Option<String>,
    pub slot: Option<i64>,
    /// `submitted` | `confirmed` | `finalized` | `failed` | `not_found`.
    pub status: String,
    pub landed: bool,
    pub error: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub confirmed_at: Option<DateTime<Utc>>,
    pub signer: Option<String>,
    pub venue: Option<String>,
    pub attempts: i32,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantTransaction {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

pub(super) fn execution_from_row(row: &PgRow) -> TenantExecution {
    TenantExecution {
        organization_id: row
            .try_get::<Option<uuid::Uuid>, _>("organization_id")
            .ok()
            .flatten()
            .map(OrganizationId::from)
            .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil())),
        id: row.try_get("id").unwrap_or_default(),
        order_id: row.try_get("order_id").unwrap_or_default(),
        ts: row
            .try_get::<DateTime<Utc>, _>("ts")
            .unwrap_or_else(|_| Utc::now()),
        kind: row.try_get("kind").unwrap_or_default(),
        endpoint: row.try_get("endpoint").ok().flatten(),
        latency_ms: row.try_get::<Option<i64>, _>("latency_ms").ok().flatten(),
        ok: row.try_get("ok").unwrap_or(false),
        detail: row.try_get("detail").ok().flatten(),
    }
}

pub(super) fn transaction_from_row(row: &PgRow) -> TenantTransaction {
    TenantTransaction {
        organization_id: row
            .try_get::<Option<uuid::Uuid>, _>("organization_id")
            .ok()
            .flatten()
            .map(OrganizationId::from)
            .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil())),
        signature: row.try_get("signature").unwrap_or_default(),
        chain: row.try_get("chain").unwrap_or_default(),
        order_id: row.try_get("order_id").ok().flatten(),
        slot: row.try_get("slot").ok().flatten(),
        status: row.try_get("status").unwrap_or_default(),
        landed: row.try_get("landed").unwrap_or(false),
        error: row.try_get("error").ok().flatten(),
        submitted_at: row
            .try_get::<DateTime<Utc>, _>("submitted_at")
            .unwrap_or_else(|_| Utc::now()),
        confirmed_at: row
            .try_get::<Option<DateTime<Utc>>, _>("confirmed_at")
            .ok()
            .flatten(),
        signer: row.try_get("signer").ok().flatten(),
        venue: row.try_get("venue").ok().flatten(),
        attempts: row.try_get("attempts").unwrap_or_default(),
    }
}
