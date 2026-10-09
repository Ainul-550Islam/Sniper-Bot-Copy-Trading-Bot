//! Tenant copy leader/event/link models (PROMPT 3/10 §F36).
//!
//! Mirrors the legacy `db::copy` records with the owning
//! [`OrganizationId`] as a first-class field. After the 0029 swap the
//! SAME external leader address MAY exist for multiple tenants — each
//! tenant's row (label, status, counters) is independent.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgRow;
use sqlx::Row;

use crate::tenant::OrganizationId;

/// One tracked leader, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantLeader {
    pub organization_id: OrganizationId,
    /// The EXTERNAL on-chain leader address (globally known identity;
    /// tenant-local CONFIGURATION row).
    pub address: String,
    pub label: String,
    /// `active` | `paused` | `removed`.
    pub status: String,
    pub source: String,
    pub followed_at: DateTime<Utc>,
    pub status_since: DateTime<Utc>,
    pub events_seen: i64,
    pub mirrored: i64,
    pub rejected: i64,
    pub last_event_at: Option<DateTime<Utc>>,
    pub last_slot: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantLeader {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One append-only leader lifecycle transition, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantLeaderEvent {
    pub organization_id: OrganizationId,
    pub id: i64,
    pub address: String,
    /// `followed` | `paused` | `resumed` | `unfollowed` | `rule_changed`.
    pub event: String,
    pub reason: Option<String>,
    pub replica_id: String,
    pub ts: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantLeaderEvent {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One leader-trade event the pipeline finished with, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantCopyEvent {
    pub organization_id: OrganizationId,
    pub event_id: String,
    pub leader: String,
    pub signature: String,
    pub slot: i64,
    pub mint: String,
    /// `buy` | `sell`.
    pub side: String,
    pub venue: String,
    pub token_amount: f64,
    pub sol_amount: f64,
    pub source: String,
    pub source_sequence: i64,
    pub event_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub stage: String,
    pub reject_reason: Option<String>,
    pub detail: Option<String>,
    pub intent_id: Option<String>,
    pub position_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantCopyEvent {
    fn row_organization_id(&self) -> OrganizationId {
        self.organization_id
    }
}

/// One follower-position ↔ leader-entry link, owned by a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantCopyLink {
    pub organization_id: OrganizationId,
    pub position_id: String,
    pub leader: String,
    pub mint: String,
    pub entry_event_id: String,
    pub entry_signature: String,
    pub intent_id: Option<String>,
    pub leader_token_amount: f64,
    pub follower_qty: f64,
    /// `open` | `closed` | `orphaned` | `mismatch`.
    pub status: String,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub exit_event_id: Option<String>,
    pub last_reconciled_at: Option<DateTime<Utc>>,
    pub note: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl crate::trading_repository::tenant_assert::OwnedRow for TenantCopyLink {
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

pub(super) fn leader_from_row(row: &PgRow) -> TenantLeader {
    TenantLeader {
        organization_id: org_of(row),
        address: row.try_get("address").unwrap_or_default(),
        label: row.try_get("label").unwrap_or_default(),
        status: row.try_get("status").unwrap_or_default(),
        source: row.try_get("source").unwrap_or_default(),
        followed_at: row
            .try_get::<DateTime<Utc>, _>("followed_at")
            .unwrap_or_else(|_| Utc::now()),
        status_since: row
            .try_get::<DateTime<Utc>, _>("status_since")
            .unwrap_or_else(|_| Utc::now()),
        events_seen: row.try_get("events_seen").unwrap_or_default(),
        mirrored: row.try_get("mirrored").unwrap_or_default(),
        rejected: row.try_get("rejected").unwrap_or_default(),
        last_event_at: row.try_get("last_event_at").ok().flatten(),
        last_slot: row.try_get("last_slot").ok().flatten(),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
    }
}

pub(super) fn leader_event_from_row(row: &PgRow) -> TenantLeaderEvent {
    TenantLeaderEvent {
        organization_id: org_of(row),
        id: row.try_get("id").unwrap_or_default(),
        address: row.try_get("address").unwrap_or_default(),
        event: row.try_get("event").unwrap_or_default(),
        reason: row.try_get("reason").ok().flatten(),
        replica_id: row.try_get("replica_id").unwrap_or_default(),
        ts: row
            .try_get::<DateTime<Utc>, _>("ts")
            .unwrap_or_else(|_| Utc::now()),
    }
}

pub(super) fn copy_event_from_row(row: &PgRow) -> TenantCopyEvent {
    TenantCopyEvent {
        organization_id: org_of(row),
        event_id: row.try_get("event_id").unwrap_or_default(),
        leader: row.try_get("leader").unwrap_or_default(),
        signature: row.try_get("signature").unwrap_or_default(),
        slot: row.try_get("slot").unwrap_or_default(),
        mint: row.try_get("mint").unwrap_or_default(),
        side: row.try_get("side").unwrap_or_default(),
        venue: row.try_get("venue").unwrap_or_default(),
        token_amount: row.try_get("token_amount").unwrap_or_default(),
        sol_amount: row.try_get("sol_amount").unwrap_or_default(),
        source: row.try_get("source").unwrap_or_default(),
        source_sequence: row.try_get("source_sequence").unwrap_or_default(),
        event_at: row.try_get("event_at").ok().flatten(),
        observed_at: row
            .try_get::<DateTime<Utc>, _>("observed_at")
            .unwrap_or_else(|_| Utc::now()),
        stage: row.try_get("stage").unwrap_or_default(),
        reject_reason: row.try_get("reject_reason").ok().flatten(),
        detail: row.try_get("detail").ok().flatten(),
        intent_id: row.try_get("intent_id").ok().flatten(),
        position_id: row.try_get("position_id").ok().flatten(),
        created_at: row
            .try_get::<DateTime<Utc>, _>("created_at")
            .unwrap_or_else(|_| Utc::now()),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
    }
}

pub(super) fn link_from_row(row: &PgRow) -> TenantCopyLink {
    TenantCopyLink {
        organization_id: org_of(row),
        position_id: row.try_get("position_id").unwrap_or_default(),
        leader: row.try_get("leader").unwrap_or_default(),
        mint: row.try_get("mint").unwrap_or_default(),
        entry_event_id: row.try_get("entry_event_id").unwrap_or_default(),
        entry_signature: row.try_get("entry_signature").unwrap_or_default(),
        intent_id: row.try_get("intent_id").ok().flatten(),
        leader_token_amount: row.try_get("leader_token_amount").unwrap_or_default(),
        follower_qty: row.try_get("follower_qty").unwrap_or_default(),
        status: row.try_get("status").unwrap_or_default(),
        opened_at: row
            .try_get::<DateTime<Utc>, _>("opened_at")
            .unwrap_or_else(|_| Utc::now()),
        closed_at: row.try_get("closed_at").ok().flatten(),
        exit_event_id: row.try_get("exit_event_id").ok().flatten(),
        last_reconciled_at: row.try_get("last_reconciled_at").ok().flatten(),
        note: row.try_get("note").ok().flatten(),
        updated_at: row
            .try_get::<DateTime<Utc>, _>("updated_at")
            .unwrap_or_else(|_| Utc::now()),
    }
}
