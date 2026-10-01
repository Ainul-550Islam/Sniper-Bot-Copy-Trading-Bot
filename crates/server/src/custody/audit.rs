//! Custody audit trail (§F, Batch 8).
//!
//! Every custody boundary decision — signed or refused — produces one
//! audit record. Records are secret-free by construction: they carry
//! identifiers, the digest (already a hash), provider names, outcome
//! codes and timestamps. They can never carry key material because the
//! boundary never touches key material in plaintext form.
//!
//! The in-process sink (`CustodyAuditLog`) is a bounded ring buffer that
//! the ops API reads for the live view. It is explicitly NOT the durable
//! ledger: durable, queryable custody decisions belong to the tenant
//! decision log / ops journal when the boundary is wired to a live
//! provider. Both facts are stated in `persistence_note()` so no reader
//! mistakes the ring for a compliance store.

use std::collections::VecDeque;
use std::sync::Mutex;

use bot_core::tenant::OrganizationId;
use chrono::{DateTime, Utc};
use serde::Serialize;

/// Cap on retained records per sink (bounded memory; the durable trail
/// is the decision log, not this ring).
const RING_CAPACITY: usize = 10_000;

/// Outcome of one custody boundary attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CustodyAuditOutcome {
    /// The provider produced a signature.
    Signed,
    /// The boundary refused; `code` carries the machine reason.
    Refused,
}

impl CustodyAuditOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            CustodyAuditOutcome::Signed => "signed",
            CustodyAuditOutcome::Refused => "refused",
        }
    }
}

/// One immutable custody audit record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CustodyAuditRecord {
    /// When the boundary decided (UTC).
    pub at: DateTime<Utc>,
    /// Organization that requested the signature.
    pub organization_id: OrganizationId,
    /// Provider the request targeted.
    pub provider: String,
    /// Module the caller exercised (e.g. "module-sniper").
    pub module: String,
    /// Bounded audit purpose (e.g. "order-signing").
    pub purpose: String,
    /// The 32-byte digest that was (or would have been) signed.
    pub digest_hex: String,
    /// Signed or refused.
    pub outcome: CustodyAuditOutcome,
    /// Machine-readable refusal code (`policy.tenant_suspended`,
    /// `provider_unsupported.vault`, …). Empty for signed outcomes.
    pub code: String,
}

impl CustodyAuditRecord {
    pub fn signed(
        at: DateTime<Utc>,
        organization_id: OrganizationId,
        provider: &str,
        module: &str,
        purpose: &str,
        digest_hex: &str,
    ) -> Self {
        Self {
            at,
            organization_id,
            provider: provider.to_string(),
            module: module.to_string(),
            purpose: purpose.to_string(),
            digest_hex: digest_hex.to_string(),
            outcome: CustodyAuditOutcome::Signed,
            code: String::new(),
        }
    }

    pub fn refused(
        at: DateTime<Utc>,
        organization_id: OrganizationId,
        provider: &str,
        module: &str,
        purpose: &str,
        digest_hex: &str,
        code: &str,
    ) -> Self {
        Self {
            at,
            organization_id,
            provider: provider.to_string(),
            module: module.to_string(),
            purpose: purpose.to_string(),
            digest_hex: digest_hex.to_string(),
            outcome: CustodyAuditOutcome::Refused,
            code: code.to_string(),
        }
    }

    /// Secret-free one-liner for logs.
    pub fn summary(&self) -> String {
        format!(
            "custody_audit at={} org={} provider={} module={} purpose={} outcome={} code={}",
            self.at.to_rfc3339(),
            self.organization_id.as_uuid(),
            self.provider,
            self.module,
            self.purpose,
            self.outcome.as_str(),
            if self.code.is_empty() {
                "-"
            } else {
                &self.code
            }
        )
    }
}

/// Bounded in-process audit ring for the live ops view.
///
/// Thread-safe. Not durable — see [`CustodyAuditLog::persistence_note`].
#[derive(Debug, Default)]
pub struct CustodyAuditLog {
    ring: Mutex<VecDeque<CustodyAuditRecord>>,
}

impl CustodyAuditLog {
    pub fn new() -> Self {
        Self {
            ring: Mutex::new(VecDeque::with_capacity(64)),
        }
    }

    /// Record one outcome. Oldest records are dropped past the cap.
    pub fn record(&self, record: CustodyAuditRecord) {
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if ring.len() == RING_CAPACITY {
            ring.pop_front();
        }
        ring.push_back(record);
    }

    /// Most recent `n` records (oldest first), regardless of org.
    pub fn recent(&self, n: usize) -> Vec<CustodyAuditRecord> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        let skip = ring.len().saturating_sub(n);
        ring.iter().skip(skip).cloned().collect()
    }

    /// Most recent `n` records for ONE organization — the tenant-scoped
    /// view. Never returns another organization's records.
    pub fn for_organization(
        &self,
        organization_id: &OrganizationId,
        n: usize,
    ) -> Vec<CustodyAuditRecord> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        // Newest-first take, then restore oldest-first order.
        let mut out: Vec<CustodyAuditRecord> = ring
            .iter()
            .filter(|r| r.organization_id == *organization_id)
            .rev()
            .take(n)
            .cloned()
            .collect();
        out.reverse();
        out
    }

    pub fn len(&self) -> usize {
        self.ring.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Honest statement about where the durable audit trail lives.
    pub fn persistence_note(&self) -> &'static str {
        "in-process bounded ring for the live ops view; durable custody decisions are journalled via the tenant decision log when the boundary is wired to a live provider"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org() -> OrganizationId {
        OrganizationId::new()
    }

    fn record_for(org: &OrganizationId, code: &str) -> CustodyAuditRecord {
        if code.is_empty() {
            CustodyAuditRecord::signed(
                Utc::now(),
                *org,
                "vault",
                "module-sniper",
                "order-signing",
                &"ab".repeat(32),
            )
        } else {
            CustodyAuditRecord::refused(
                Utc::now(),
                *org,
                "vault",
                "module-sniper",
                "order-signing",
                &"ab".repeat(32),
                code,
            )
        }
    }

    #[test]
    fn records_signed_and_refused_outcomes() {
        let log = CustodyAuditLog::new();
        let o = org();
        log.record(record_for(&o, ""));
        log.record(record_for(&o, "policy.tenant_suspended"));
        assert_eq!(log.len(), 2);
        let recent = log.recent(10);
        assert_eq!(recent[0].outcome, CustodyAuditOutcome::Signed);
        assert_eq!(recent[1].outcome, CustodyAuditOutcome::Refused);
        assert_eq!(recent[1].code, "policy.tenant_suspended");
        assert!(recent[1].summary().contains("outcome=refused"));
        assert!(recent[1].summary().contains("policy.tenant_suspended"));
    }

    #[test]
    fn organization_view_is_isolated() {
        let log = CustodyAuditLog::new();
        let a = org();
        let b = org();
        for i in 0..3 {
            log.record(record_for(&a, &format!("policy.deny{i}")));
        }
        log.record(record_for(&b, "provider_unsupported.kms"));
        let a_view = log.for_organization(&a, 10);
        let b_view = log.for_organization(&b, 10);
        assert_eq!(a_view.len(), 3);
        assert!(a_view
            .iter()
            .all(|r| r.organization_id == a && r.code.starts_with("policy.deny")));
        assert_eq!(b_view.len(), 1);
        assert_eq!(b_view[0].code, "provider_unsupported.kms");
    }

    #[test]
    fn recent_returns_tail_in_order() {
        let log = CustodyAuditLog::new();
        let o = org();
        for i in 0..5 {
            log.record(record_for(&o, &format!("code{i}")));
        }
        let tail = log.recent(2);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].code, "code3");
        assert_eq!(tail[1].code, "code4");
    }

    #[test]
    fn ring_is_bounded() {
        let log = CustodyAuditLog::new();
        let o = org();
        for i in 0..(RING_CAPACITY + 25) {
            log.record(record_for(&o, &format!("code{i}")));
        }
        assert_eq!(log.len(), RING_CAPACITY);
        // The first 25 records were dropped; the oldest retained is
        // code25 and the newest is code1024.
        let all = log.recent(RING_CAPACITY);
        assert_eq!(all.len(), RING_CAPACITY);
        assert_eq!(all[0].code, "code25");
        assert_eq!(
            all[RING_CAPACITY - 1].code,
            format!("code{}", RING_CAPACITY + 24)
        );
    }

    #[test]
    fn persistence_note_is_explicit_about_durability() {
        let log = CustodyAuditLog::new();
        assert!(log.persistence_note().contains("tenant decision log"));
        assert!(
            log.persistence_note().contains("not durable")
                || log.persistence_note().contains("in-process")
        );
    }
}
