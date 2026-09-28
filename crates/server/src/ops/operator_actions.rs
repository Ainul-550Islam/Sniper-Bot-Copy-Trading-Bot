//! Typed operator-action audit model (Batch 4). Sensitive categories require reason.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorAction {
    pub actor: String,
    pub scope: String,  // "platform" or org id
    pub action: String, // e.g. "kill_switch_engage" | "suspend_org" | "revoke_custody"
    pub resource: Option<String>,
    pub outcome: String, // "success"|"failure"|"denied"
    pub reason: Option<String>,
    pub at: DateTime<Utc>,
    pub correlation_id: String,
}

fn redact(s: &str) -> String {
    let l = s.to_ascii_lowercase();
    if l.contains("secret")
        || l.contains("password")
        || l.contains("private")
        || l.contains("token")
    {
        "<redacted>".into()
    } else {
        s.to_string()
    }
}

fn requires_reason(action: &str) -> bool {
    matches!(
        action,
        "kill_switch_engage"
            | "kill_switch_release"
            | "suspend_org"
            | "close_org"
            | "revoke_custody"
            | "rotate_signer"
            | "key_revoke"
            | "journal_rotate"
            | "live_mode"
    ) || action.contains("revoke")
        || action.contains("kill")
}

pub fn validate(action: &OperatorAction) -> Result<(), String> {
    if action.actor.trim().is_empty() {
        return Err("actor required".into());
    }
    if action.action.trim().is_empty() {
        return Err("action required".into());
    }
    if requires_reason(&action.action)
        && action
            .reason
            .as_ref()
            .map(|r| r.trim().is_empty())
            .unwrap_or(true)
    {
        return Err(format!("action {} requires reason text", action.action));
    }
    if action.actor.to_ascii_lowercase().contains("secret")
        || action
            .resource
            .as_ref()
            .map(|r| r.to_ascii_lowercase().contains("secret"))
            .unwrap_or(false)
    {
        return Err("actor/resource must not contain raw secret".into());
    }
    // never include raw credentials
    if let Some(reason) = &action.reason {
        if reason.contains("BEGIN PRIVATE KEY") || reason.to_ascii_lowercase().contains("password")
        {
            return Err("reason must not contain raw credentials".into());
        }
    }
    Ok(())
}

impl OperatorAction {
    pub fn new(
        actor: impl Into<String>,
        scope: impl Into<String>,
        action: impl Into<String>,
        resource: Option<String>,
        outcome: impl Into<String>,
        reason: Option<String>,
    ) -> Self {
        Self {
            actor: redact(&actor.into()),
            scope: redact(&scope.into()),
            action: action.into(),
            resource: resource.map(|r| redact(&r)),
            outcome: outcome.into(),
            reason: reason.map(|r| redact(&r)),
            at: Utc::now(),
            correlation_id: format!("corr-{}", uuid::Uuid::new_v4()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sensitive_requires_reason() {
        let a = OperatorAction::new(
            "op",
            "platform",
            "kill_switch_engage",
            None,
            "success",
            None,
        );
        assert!(validate(&a).is_err());
        let mut b = a;
        b.reason = Some("incident #123".into());
        assert!(validate(&b).is_ok());
    }
    #[test]
    fn redacts() {
        let a = OperatorAction::new(
            "secret_token",
            "org",
            "key_revoke",
            Some("secret".into()),
            "success",
            Some("rotation".into()),
        );
        assert_eq!(a.actor, "<redacted>");
        assert_eq!(a.resource, Some("<redacted>".into()));
    }
    #[test]
    fn benign_no_reason_ok() {
        let a = OperatorAction::new("op", "org", "audit_read", None, "success", None);
        assert!(validate(&a).is_ok());
    }
    #[test]
    fn reason_must_not_contain_private() {
        // Construct directly to bypass redact in ::new, so validate sees raw secret
        let a = OperatorAction {
            actor: "op".into(),
            scope: "org".into(),
            action: "rotate_signer".into(),
            resource: None,
            outcome: "success".into(),
            reason: Some("BEGIN PRIVATE KEY".into()),
            at: Utc::now(),
            correlation_id: "corr-test".into(),
        };
        assert!(validate(&a).is_err());
    }
}
