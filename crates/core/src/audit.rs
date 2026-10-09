//! Immutable audit trail (BUILD PLAN §4-xii).
//!
//! Every security- or money-relevant control action (kill switch, mode
//! change, key rotation, config reload, recovery verdicts, manual orders)
//! gets one [`AuditTrail::record`] call. Each record fans out to:
//!
//! 1. **PostgreSQL** `audit_events` with a SHA-256 hash chain (when the DB
//!    is attached) — append-only; the application exposes NO update/delete
//!    path, and [`AuditTrail::verify`] can prove tampering by re-walking
//!    the chain.
//! 2. The **event bus** as `AppEvent::Audit` — live dashboard/Telegram
//!    visibility AND automatic JSONL journaling via the storage subscriber
//!    (so even DB-less deployments keep an audit file).
//! 3. A bounded **in-memory ring** — the read API's fallback when there is
//!    no database.
//!
//! Failure policy: a DB outage downgrades durability (ring + journal only)
//! and raises `bot_audit_persist_failed_total`; auditing NEVER blocks or
//! fails the operation being audited — but the degraded state is loud.

use std::collections::VecDeque;
use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::db::repo::{AuditEntry, AuditRepo};
use crate::db::Database;
use crate::events::{AppEvent, EventBus};
use crate::obs::metrics;

/// Ring buffer capacity for the no-DB read path.
const RING_CAP: usize = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditOutcome {
    Success,
    Failure,
    Denied,
}

impl AuditOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditOutcome::Success => "success",
            AuditOutcome::Failure => "failure",
            AuditOutcome::Denied => "denied",
        }
    }
}

/// A trail entry as surfaced by the API (DB rows and ring entries share
/// this shape; ring entries have `id = 0` and synthetic hashes).
#[derive(Clone, Debug, Serialize)]
pub struct AuditRecord {
    pub id: i64,
    pub ts: chrono::DateTime<Utc>,
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub outcome: String,
    pub detail: serde_json::Value,
    pub hash: String,
    /// True when this row is part of the durable hash-chained log.
    pub chained: bool,
}

impl From<AuditEntry> for AuditRecord {
    fn from(e: AuditEntry) -> Self {
        AuditRecord {
            id: e.id,
            ts: e.ts,
            actor: e.actor,
            action: e.action,
            target: e.target,
            outcome: e.outcome,
            detail: e.detail,
            hash: e.hash,
            chained: true,
        }
    }
}

/// Result of a durable-chain verification.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VerifyResult {
    /// No durable log attached — nothing to verify.
    NotChained,
    /// Whole chain recomputed and intact.
    Intact { entries: i64 },
    /// Chain broken at this entry id (tamper evidence).
    Broken { at_id: i64 },
    /// Verification could not run (DB error).
    Unknown { error: String },
}

pub struct AuditTrail {
    db: Option<Arc<Database>>,
    bus: EventBus,
    ring: RwLock<VecDeque<AuditRecord>>,
}

impl AuditTrail {
    pub fn new(db: Option<Arc<Database>>, bus: EventBus) -> Arc<Self> {
        Arc::new(AuditTrail {
            db,
            bus,
            ring: RwLock::new(VecDeque::with_capacity(RING_CAP.min(64))),
        })
    }

    /// Record one audited action. Never returns an error: persistence
    /// problems degrade loudly (log + metric) but the caller's operation is
    /// never blocked by the audit subsystem itself.
    pub async fn record(
        &self,
        actor: &str,
        action: &str,
        target: Option<&str>,
        outcome: AuditOutcome,
        detail: serde_json::Value,
    ) {
        // 1) Durable hash-chained row.
        let mut chained: Option<AuditEntry> = None;
        if let Some(db) = &self.db {
            match AuditRepo::new(db.clone())
                .append(actor, action, target, outcome.as_str(), &detail)
                .await
            {
                Ok(entry) => chained = Some(entry),
                Err(e) => {
                    metrics::global()
                        .counter(
                            "bot_audit_persist_failed_total",
                            "Audit records that could not reach the durable log.",
                            &[],
                        )
                        .inc();
                    warn!(error = %e, action, actor, "AUDIT PERSISTENCE FAILED — record kept in ring + journal only");
                }
            }
        }

        // 2) Ring (API fallback read path).
        let record = match &chained {
            Some(e) => AuditRecord::from(e.clone()),
            None => AuditRecord {
                id: 0,
                ts: Utc::now(),
                actor: actor.to_string(),
                action: action.to_string(),
                target: target.map(str::to_string),
                outcome: outcome.as_str().to_string(),
                detail: detail.clone(),
                hash: String::new(),
                chained: false,
            },
        };
        {
            let mut ring = self.ring.write().await;
            ring.push_back(record.clone());
            while ring.len() > RING_CAP {
                ring.pop_front();
            }
        }

        // 3) Event bus → live feeds + JSONL journal.
        self.bus.publish(AppEvent::Audit {
            ts: record.ts,
            actor: actor.to_string(),
            action: action.to_string(),
            target: record.target.clone(),
            outcome: record.outcome.clone(),
        });

        debug!(actor, action, outcome = outcome.as_str(), "audit recorded");
    }

    /// Convenience wrappers for the most common entries.
    pub async fn success(&self, actor: &str, action: &str, target: Option<&str>) {
        self.record(
            actor,
            action,
            target,
            AuditOutcome::Success,
            serde_json::json!({}),
        )
        .await;
    }

    pub async fn denied(&self, actor: &str, action: &str, target: Option<&str>, reason: &str) {
        self.record(
            actor,
            action,
            target,
            AuditOutcome::Denied,
            serde_json::json!({ "reason": reason }),
        )
        .await;
    }

    /// Recent records, newest first. Prefers the durable log; falls back to
    /// the ring when there is no DB (or the query fails).
    pub async fn recent(&self, limit: usize) -> Vec<AuditRecord> {
        let limit = limit.clamp(1, RING_CAP);
        if let Some(db) = &self.db {
            match AuditRepo::new(db.clone()).list_recent(limit as i64).await {
                Ok(rows) => return rows.into_iter().map(AuditRecord::from).collect(),
                Err(e) => warn!(error = %e, "audit read failed — serving ring buffer"),
            }
        }
        let ring = self.ring.read().await;
        ring.iter().rev().take(limit).cloned().collect()
    }

    pub async fn verify(&self) -> VerifyResult {
        let Some(db) = &self.db else {
            return VerifyResult::NotChained;
        };
        let repo = AuditRepo::new(db.clone());
        match repo.verify_chain().await {
            Ok(None) => {
                let count = repo.count().await.unwrap_or(0);
                VerifyResult::Intact { entries: count }
            }
            Ok(Some(id)) => VerifyResult::Broken { at_id: id },
            Err(e) => VerifyResult::Unknown {
                error: e.to_string(),
            },
        }
    }

    pub fn durable(&self) -> bool {
        self.db.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_lands_in_ring_and_bus_without_db() {
        let bus = EventBus::new(16);
        let mut sub = bus.subscribe();
        let trail = AuditTrail::new(None, bus);
        trail
            .record(
                "api:key-abc",
                "kill_switch",
                Some("system"),
                AuditOutcome::Success,
                serde_json::json!({"via": "test"}),
            )
            .await;
        let recent = trail.recent(10).await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].action, "kill_switch");
        assert!(!recent[0].chained, "ring entries are not hash-chained");
        // The bus saw the audit event.
        let ev = sub.recv().await.unwrap();
        match ev.as_ref() {
            AppEvent::Audit { actor, action, .. } => {
                assert_eq!(actor, "api:key-abc");
                assert_eq!(action, "kill_switch");
            }
            other => panic!("expected Audit event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ring_is_bounded_and_newest_first() {
        let trail = AuditTrail::new(None, EventBus::new(8));
        for i in 0..(RING_CAP + 50) {
            trail.success("t", &format!("action-{i}"), None).await;
        }
        let recent = trail.recent(RING_CAP).await;
        assert_eq!(recent.len(), RING_CAP);
        assert_eq!(recent[0].action, format!("action-{}", RING_CAP + 49));
    }

    #[tokio::test]
    async fn verify_without_db_reports_not_chained() {
        let trail = AuditTrail::new(None, EventBus::new(8));
        assert_eq!(trail.verify().await, VerifyResult::NotChained);
        assert!(!trail.durable());
    }

    #[test]
    fn outcome_strings_match_db_check_constraint() {
        // The DB CHECK allows exactly these three.
        assert_eq!(AuditOutcome::Success.as_str(), "success");
        assert_eq!(AuditOutcome::Failure.as_str(), "failure");
        assert_eq!(AuditOutcome::Denied.as_str(), "denied");
    }
}
