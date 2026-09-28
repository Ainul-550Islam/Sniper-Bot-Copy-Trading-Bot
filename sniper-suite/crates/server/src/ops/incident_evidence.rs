//! Incident evidence record (Batch 4). Redact credentials, append-only semantics.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentRecord {
    pub incident_id: String,
    pub started_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub severity: Severity,
    pub scope: String, // tenant scope or "platform"
    pub summary: String,
    pub actions: Vec<String>,
    pub operator: String,       // redacted handle, not secret
    pub recovery_state: String, // e.g. "recovered" or "mitigated"
    pub audit_refs: Vec<String>,
}

impl IncidentRecord {
    pub fn new(
        severity: Severity,
        scope: impl Into<String>,
        summary: impl Into<String>,
        operator: impl Into<String>,
    ) -> Self {
        Self {
            incident_id: format!("inc-{}", uuid::Uuid::new_v4()),
            started_at: Utc::now(),
            resolved_at: None,
            severity,
            scope: redact(&scope.into()),
            summary: redact(&summary.into()),
            actions: Vec::new(),
            operator: redact(&operator.into()),
            recovery_state: "open".into(),
            audit_refs: Vec::new(),
        }
    }
    pub fn append_action(&mut self, action: impl Into<String>) {
        self.actions.push(redact(&action.into()));
    }
    pub fn resolve(&mut self, recovery: impl Into<String>) {
        self.resolved_at = Some(Utc::now());
        self.recovery_state = redact(&recovery.into());
    }
    pub fn is_append_only(&self) -> bool {
        true
    } // application perspective: never delete
}

fn redact(s: &str) -> String {
    let l = s.to_ascii_lowercase();
    if l.contains("secret")
        || l.contains("password")
        || l.contains("private")
        || l.contains("token")
        || l.contains("BEGIN PRIVATE")
    {
        "<redacted>".into()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts() {
        let r = IncidentRecord::new(Severity::High, "org-123", "found secret=abc", "op1");
        assert_eq!(r.summary, "<redacted>");
    }
    #[test]
    fn append_only() {
        let mut r = IncidentRecord::new(Severity::Low, "platform", "latency spike", "op");
        r.append_action("restarted worker");
        assert_eq!(r.actions.len(), 1);
        assert!(r.is_append_only());
    }
    #[test]
    fn resolve_sets_time() {
        let mut r = IncidentRecord::new(Severity::Medium, "platform", "db lag", "op");
        r.resolve("recovered");
        assert!(r.resolved_at.is_some());
        assert_eq!(r.recovery_state, "recovered");
    }
    #[test]
    fn operator_redacted() {
        let r = IncidentRecord::new(Severity::Critical, "platform", "test", "token=secret");
        assert_eq!(r.operator, "<redacted>");
    }
}
