//! Tenant worker-claim lane models (PROMPT 3/10 §H48).
//!
//! Backed by the 0032 `worker_claims` table — PK
//! `(organization_id, purpose)`. The GLOBAL `ha_leases` plane (0016)
//! stays global (deployment infra); these lanes are the TENANT data
//! plane's leadership: "which replica leads tenant X's pipeline?".

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// A lease the acting tenant's worker holds on a named work lane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantWorkClaim {
    /// The tenant that owns the lane.
    pub organization_id: OrganizationId,
    /// The lane: `copy_pipeline`, `poly_recon`, `gc`, `reporting`, …
    pub purpose: String,
    /// The replica instance id that holds the lease.
    pub leader_name: String,
    /// FENCING TOKEN: strictly increasing per lane, never reused.
    pub generation: i64,
    pub acquired_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    pub lease_until: DateTime<Utc>,
    pub takeover_count: i64,
    pub previous_leader: Option<String>,
    pub released: bool,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantWorkClaim {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

impl TenantWorkClaim {
    /// Has the lease expired as of `now`?
    pub fn is_lapsed(&self, now: DateTime<Utc>) -> bool {
        self.lease_until <= now
    }

    /// Does the given replica currently hold a live (unexpired,
    /// unreleased) lease?
    pub fn held_by(&self, replica: &str, now: DateTime<Utc>) -> bool {
        self.leader_name == replica && !self.released && !self.is_lapsed(now)
    }
}

/// Outcome of a lane acquisition attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimDecision {
    /// This replica won (fresh acquisition, takeover, or re-acquire).
    Acquired(TenantWorkClaim),
    /// Another live leader holds the lane for THIS tenant.
    Rejected {
        holder: String,
        generation: i64,
        lease_until: DateTime<Utc>,
    },
}

pub(super) fn claim_from_row(row: &PgRow) -> TenantWorkClaim {
    TenantWorkClaim {
        organization_id: row
            .try_get::<Option<uuid::Uuid>, _>("organization_id")
            .ok()
            .flatten()
            .map(OrganizationId::from)
            .unwrap_or_else(|| OrganizationId(uuid::Uuid::nil())),
        purpose: row.try_get("purpose").unwrap_or_default(),
        leader_name: row.try_get("leader_name").unwrap_or_default(),
        generation: row.try_get("generation").unwrap_or_default(),
        acquired_at: row
            .try_get::<DateTime<Utc>, _>("acquired_at")
            .unwrap_or_else(|_| Utc::now()),
        heartbeat_at: row
            .try_get::<DateTime<Utc>, _>("heartbeat_at")
            .unwrap_or_else(|_| Utc::now()),
        lease_until: row
            .try_get::<DateTime<Utc>, _>("lease_until")
            .unwrap_or_else(|_| Utc::now()),
        takeover_count: row.try_get("takeover_count").unwrap_or_default(),
        previous_leader: row.try_get("previous_leader").ok().flatten(),
        released: row.try_get("released").unwrap_or(false),
    }
}
