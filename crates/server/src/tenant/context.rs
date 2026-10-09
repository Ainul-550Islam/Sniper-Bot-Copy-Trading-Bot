//! The typed tenant execution context (tenant-isolation file 16).
//!
//! [`TenantContext`] is the wrapper every tenant-sensitive entry path
//! (HTTP handler, stream consumer, background job, recovery pass) builds
//! from AUTHENTICATED request information before any guard runs. It
//! pairs the resolved tenant record with the authenticated principal
//! label and the entry origin, and is the single input the
//! [`super::context_guard`], [`super::runtime_context`] and the
//! gateway's `authorize_in_context` consume.
//!
//! Rules (mirroring the SaaS middleware):
//!
//! * a context is built from a RESOLVED organization record — a
//!   caller-supplied organization id never constructs one directly;
//! * a missing or blank principal is a construction error (fail
//!   closed, no anonymous tenant work);
//! * the lifecycle snapshot is DERIVED on demand
//!   ([`TenantContext::state`]), never stored, so it can never go
//!   stale inside a long-lived context;
//! * the context carries no secrets — only identities and labels.

use chrono::{DateTime, Utc};

use bot_core::tenant::{Organization, OrganizationId, TenantStateSnapshot};

/// Where a tenant context entered the system. Recorded on the decision
/// log and the execution trace so an audit reader can tell an HTTP
/// request from a background tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextOrigin {
    /// An authenticated HTTP/API request.
    Http,
    /// A stream/WS consumer acting for the tenant.
    Stream,
    /// A scheduled background job.
    Job,
    /// A recovery/repair pass.
    Recovery,
}

impl ContextOrigin {
    /// Stable label (logs, decision entries).
    pub const fn as_str(self) -> &'static str {
        match self {
            ContextOrigin::Http => "http",
            ContextOrigin::Stream => "stream",
            ContextOrigin::Job => "job",
            ContextOrigin::Recovery => "recovery",
        }
    }
}

/// Every way context construction can fail. All of them are fail-closed:
/// the caller does NOT get a best-effort context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextError {
    /// The principal label was empty or blank.
    MissingPrincipal,
    /// The tenant record was not supplied.
    MissingTenant,
}

impl ContextError {
    /// Stable machine-readable label.
    pub const fn as_str(&self) -> &'static str {
        match self {
            ContextError::MissingPrincipal => "missing_principal",
            ContextError::MissingTenant => "missing_tenant",
        }
    }
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tenant context error: {}", self.as_str())
    }
}

impl std::error::Error for ContextError {}

/// The typed tenant execution context.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// The resolved tenant record (authenticated, not caller-supplied).
    organization: Organization,
    /// The authenticated actor label (`user:<id>`, `apikey:<prefix>`,
    /// `job:<module>:<name>`, `recovery`) — never a secret.
    principal: String,
    /// How the request entered.
    origin: ContextOrigin,
    /// When the context was resolved.
    at: DateTime<Utc>,
}

impl TenantContext {
    /// Build a context from a resolved organization and principal.
    ///
    /// Fails closed on a blank principal — there is no anonymous tenant
    /// context, for HTTP, jobs or recovery alike.
    pub fn new(
        organization: Organization,
        principal: impl Into<String>,
        origin: ContextOrigin,
        at: DateTime<Utc>,
    ) -> Result<Self, ContextError> {
        let principal = principal.into();
        if principal.trim().is_empty() {
            return Err(ContextError::MissingPrincipal);
        }
        Ok(TenantContext {
            organization,
            principal,
            origin,
            at,
        })
    }

    /// The resolved tenant record.
    pub fn organization(&self) -> &Organization {
        &self.organization
    }

    /// The acting tenant's identity.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization.id
    }

    /// The authenticated principal label (non-secret).
    pub fn principal(&self) -> &str {
        &self.principal
    }

    /// The entry origin.
    pub fn origin(&self) -> ContextOrigin {
        self.origin
    }

    /// When the context was resolved.
    pub fn resolved_at(&self) -> DateTime<Utc> {
        self.at
    }

    /// The DERIVED lifecycle snapshot (never cached in the context).
    pub fn state(&self) -> TenantStateSnapshot {
        TenantStateSnapshot::of_organization(&self.organization)
    }

    /// Does this context act for exactly this organization?
    pub fn is(&self, organization_id: OrganizationId) -> bool {
        self.organization.id == organization_id
    }

    /// The standard principal label for a background job tick.
    pub fn job_principal(module: &str, job_name: &str) -> String {
        format!("job:{module}:{job_name}")
    }

    /// The standard principal label for a recovery pass.
    pub const RECOVERY_PRINCIPAL: &'static str = "recovery";
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::OrganizationStatus;
    use chrono::Utc;

    fn org(status: OrganizationStatus) -> Organization {
        let mut organization =
            Organization::new(OrganizationId::new(), "acme", "Acme", None, Utc::now());
        organization.status = status;
        organization
    }

    #[test]
    fn a_resolved_organization_and_principal_form_a_context() {
        let organization = org(OrganizationStatus::Active);
        let id = organization.id;
        let context =
            TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap();
        assert_eq!(context.organization_id(), id);
        assert_eq!(context.principal(), "user:1");
        assert_eq!(context.origin().as_str(), "http");
        assert!(context.is(id));
        assert!(!context.is(OrganizationId::new()));
    }

    #[test]
    fn a_blank_principal_is_rejected_fail_closed() {
        let organization = org(OrganizationStatus::Active);
        for blank in ["", "   ", "\t"] {
            let err =
                TenantContext::new(organization.clone(), blank, ContextOrigin::Http, Utc::now())
                    .unwrap_err();
            assert_eq!(err, ContextError::MissingPrincipal);
            assert_eq!(err.as_str(), "missing_principal");
        }
    }

    #[test]
    fn the_state_snapshot_is_derived_per_call() {
        let active = org(OrganizationStatus::Active);
        let context = TenantContext::new(active, "user:1", ContextOrigin::Job, Utc::now()).unwrap();
        assert!(context.state().may_trade());
        assert!(context.state().may_read());

        let mut suspended = org(OrganizationStatus::Active);
        suspended.status = OrganizationStatus::Suspended;
        let context =
            TenantContext::new(suspended, "user:1", ContextOrigin::Job, Utc::now()).unwrap();
        assert!(!context.state().may_trade());
        assert!(context.state().may_read());
    }

    #[test]
    fn job_and_recovery_principal_labels_are_stable() {
        assert_eq!(
            TenantContext::job_principal("copy", "recon"),
            "job:copy:recon"
        );
        assert_eq!(TenantContext::RECOVERY_PRINCIPAL, "recovery");
        assert_eq!(ContextOrigin::Stream.as_str(), "stream");
        assert_eq!(ContextOrigin::Recovery.as_str(), "recovery");
    }
}
