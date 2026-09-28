//! Disaster-recovery plan model and validation (Batch 4). Separate documented target from demonstrated result.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPlan {
    pub rpo_seconds: u64,
    pub rto_seconds: u64,
    pub backup_source: String,        // redacted, not credential
    pub restore_order: Vec<String>, // e.g. ["postgres","redis","migrations","secrets","signer","smoke"]
    pub secrets_reattachment: String, // e.g. "vault reattach required" (redacted)
    pub migration_verification: String,
    pub redis_rebuild: String, // e.g. "rebuild from postgres, dedup TTL etc"
    pub signer_checks: Vec<String>,
    pub smoke_checks: Vec<String>, // e.g. ["/health","/ready","/api/status"]
    pub documented: bool,          // true = documented
    pub demonstrated: bool,        // true = actually demonstrated via real restore
    pub last_drilled_at: Option<String>,
}

impl RecoveryPlan {
    pub fn default_documented() -> Self {
        Self {
            rpo_seconds: 3600,
            rto_seconds: 4*3600,
            backup_source: "<redacted>".into(),
            restore_order: vec!["postgres".into(),"migrations".into(),"redis".into(),"secrets".into(),"signer".into(),"smoke".into()],
            secrets_reattachment: "re-attach from vault/kms (redacted)".into(),
            migration_verification: "verify high_water 0021 and no gaps".into(),
            redis_rebuild: "redis ephemeral: rebuild dedup/leases/rate-limit from postgres; expect NotConfigured→Pass".into(),
            signer_checks: vec!["signer reachable".into(),"no private key in logs".into()],
            smoke_checks: vec!["/health 200".into(),"/ready 200".into(),"/api/status paper live_allowed=false".into()],
            documented: true,
            demonstrated: false,
            last_drilled_at: None,
        }
    }
    pub fn is_documented_only(&self) -> bool {
        self.documented && !self.demonstrated
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanValidation {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn validate(plan: &RecoveryPlan) -> PlanValidation {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if plan.rpo_seconds == 0 {
        errors.push("RPO must be >0".into());
    }
    if plan.rto_seconds == 0 {
        errors.push("RTO must be >0".into());
    }
    if plan.rpo_seconds > plan.rto_seconds {
        warnings.push("RPO > RTO unusual".into());
    }
    if plan.restore_order.is_empty() {
        errors.push("restore_order empty".into());
    }
    if !plan.restore_order.contains(&"postgres".to_string()) {
        errors.push("postgres missing in restore_order".into());
    }
    if plan.backup_source.contains("postgres://") || plan.backup_source.contains("redis://") {
        errors.push("backup_source must be redacted".into());
    }
    if plan
        .secrets_reattachment
        .to_ascii_lowercase()
        .contains("BEGIN PRIVATE")
    {
        errors.push("secrets_reattachment leaks private key".into());
    }
    if plan.documented && !plan.demonstrated {
        warnings.push("plan documented but not demonstrated (no real restore)".into());
    }
    let ok = errors.is_empty();
    PlanValidation {
        ok,
        errors,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn documented_not_demonstrated_is_warning() {
        let p = RecoveryPlan::default_documented();
        assert!(p.is_documented_only());
        let v = validate(&p);
        assert!(v.ok);
        assert!(v.warnings.iter().any(|w| w.contains("not demonstrated")));
    }
    #[test]
    fn rpo_zero_fails() {
        let mut p = RecoveryPlan::default_documented();
        p.rpo_seconds = 0;
        let v = validate(&p);
        assert!(v.errors.iter().any(|e| e.contains("RPO")));
    }
    #[test]
    fn must_include_postgres() {
        let mut p = RecoveryPlan::default_documented();
        p.restore_order = vec!["redis".into()];
        let v = validate(&p);
        assert!(v.errors.iter().any(|e| e.contains("postgres")));
    }
    #[test]
    fn redaction_required() {
        let mut p = RecoveryPlan::default_documented();
        p.backup_source = "postgres://user:pass@host/db".into();
        let v = validate(&p);
        assert!(v.errors.iter().any(|e| e.contains("redacted")));
    }
    #[test]
    fn demonstrated_true_is_ok() {
        let mut p = RecoveryPlan::default_documented();
        p.demonstrated = true;
        let v = validate(&p);
        assert!(
            v.warnings.is_empty() || !v.warnings.iter().any(|w| w.contains("not demonstrated"))
        );
    }
}
