//! Tenant context → audit correlation (tenant-isolation file 62).
//!
//! [`AuditContext`] bridges the execution layer's typed contexts
//! ([`TenantContext`], [`JobContext`]) into the append-only decision
//! log: one correlation snapshot (tenant, principal, origin, time)
//! from which [`DecisionLogEntry`] values are minted through the
//! gateway's existing vocabulary. The audit trail therefore always
//! shows WHO acted, for WHICH tenant, through WHICH entry path —
//! and never a secret (the principal is a label; values are swept
//! by [`Redaction`]).

use chrono::{DateTime, Utc};

use bot_core::tenant::OrganizationId;

use crate::tenant::context::TenantContext;

use crate::tenant_background::job_context::JobContext;

use super::decision_log::DecisionLogEntry;
use super::redaction::Redaction;

/// One correlated audit snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditContext {
    /// The tenant the action was for.
    pub organization_id: OrganizationId,
    /// The acting principal (non-secret label: `user:<id>`,
    /// `apikey:<prefix>`, `job:<module>:<name>`, `recovery`).
    pub principal: String,
    /// The entry path (http/stream/job/recovery).
    pub origin: String,
    /// When the action happened.
    pub at: DateTime<Utc>,
}

impl AuditContext {
    /// Correlate an HTTP/stream tenant context.
    pub fn from_tenant(context: &TenantContext) -> Self {
        AuditContext {
            organization_id: context.organization_id(),
            principal: Redaction::redact_value("principal", context.principal()).into_owned(),
            origin: context.origin().as_str().to_string(),
            at: context.resolved_at(),
        }
    }

    /// Correlate a background job context.
    pub fn from_job(context: &JobContext) -> Self {
        AuditContext {
            organization_id: context.organization_id(),
            principal: Redaction::redact_value("principal", context.tenant().principal())
                .into_owned(),
            origin: context.tenant().origin().as_str().to_string(),
            at: context.tenant().resolved_at(),
        }
    }

    /// An operator/platform-level audit context (explicit tenant; the
    /// principal must still be a real label — no anonymous audit).
    pub fn platform(principal: &str, at: DateTime<Utc>) -> Self {
        AuditContext {
            organization_id: OrganizationId::new(), //n/a: platform-scope
            principal: principal.to_string(),
            origin: "platform".to_string(),
            at,
        }
    }

    /// Mint a decision-log entry from this context (the gateway's
    /// vocabulary; detail is swept for secret-shaped content).
    pub fn decision_entry(
        &self,
        decision: &'static str,
        detail: impl Into<String>,
        module: &'static str,
        mode: &'static str,
    ) -> DecisionLogEntry {
        let detail = Redaction::redact_text(&detail.into());
        // Built as a struct literal (not `from_static`) because the
        // correlation's origin/principal are owned strings, not the
        // gateway's static vocabulary.
        DecisionLogEntry {
            organization_id: self.organization_id,
            decision: decision.to_string(),
            detail,
            module: module.to_string(),
            mode: mode.to_string(),
            origin: self.origin.clone(),
            principal: self.principal.clone(),
            at: self.at,
        }
    }

    /// Does this audit context belong to this tenant?
    pub fn belongs_to(&self, organization_id: OrganizationId) -> bool {
        self.organization_id == organization_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::context::ContextOrigin;
    use bot_core::tenant::{
        ModuleKind, Organization, OrganizationId, RuntimeGeneration, RuntimeId,
    };
    use chrono::Utc;

    use crate::runtime_registry::FenceToken;

    fn tenant_context(principal: &str) -> TenantContext {
        let organization =
            Organization::new(OrganizationId::new(), "acme", "Acme", None, Utc::now());
        TenantContext::new(organization, principal, ContextOrigin::Http, Utc::now()).unwrap()
    }

    fn job_context(organization_id: OrganizationId) -> JobContext {
        let organization = Organization::new(organization_id, "acme", "Acme", None, Utc::now());
        JobContext::new(
            &organization,
            ModuleKind::Copy,
            "recon",
            FenceToken {
                organization_id,
                runtime_id: RuntimeId::new(),
                generation: RuntimeGeneration::first(),
            },
            Utc::now(),
        )
        .unwrap()
    }

    #[test]
    fn an_http_context_correlates_tenant_principal_origin() {
        let context = tenant_context("user:1");
        let audit = AuditContext::from_tenant(&context);
        assert_eq!(audit.organization_id, context.organization_id());
        assert_eq!(audit.principal, "user:1");
        assert_eq!(audit.origin, "http");
        assert!(audit.belongs_to(context.organization_id()));
    }

    #[test]
    fn a_job_context_correlates_its_job_principal() {
        let org = OrganizationId::new();
        let audit = AuditContext::from_job(&job_context(org));
        assert_eq!(audit.organization_id, org);
        assert_eq!(audit.principal, "job:copy:recon");
        assert_eq!(audit.origin, "job");
    }

    #[test]
    fn decision_entries_carry_the_context_and_never_secrets() {
        let context = tenant_context("apikey:sk_ab12");
        let audit = AuditContext::from_tenant(&context);
        let entry = audit.decision_entry("deny", "token=abc leaked", "copy", "paper");
        assert_eq!(entry.organization_id, context.organization_id());
        assert_eq!(entry.decision, "deny");
        assert_eq!(entry.module, "copy");
        assert_eq!(entry.mode, "paper");
        assert_eq!(entry.origin, "http");
        assert_eq!(entry.principal, "apikey:sk_ab12");
        // The secret-shaped assignment inside the detail is masked.
        assert!(!entry.detail.contains("abc"), "{}", entry.detail);
        assert!(entry.detail.contains("[REDACTED]"), "{}", entry.detail);
    }

    #[test]
    fn platform_audit_contexts_are_explicit() {
        let audit = AuditContext::platform("operator:ada", Utc::now());
        assert_eq!(audit.principal, "operator:ada");
        assert_eq!(audit.origin, "platform");
    }
}
