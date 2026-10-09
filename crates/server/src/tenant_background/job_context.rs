//! Tenant context for background jobs (tenant-isolation file 52).
//!
//! [`JobContext`] is what a background job carries INSTEAD of ambient
//! process state: the job's tenant (a full [`TenantContext`], never a
//! bare organization id), the module it serves, its stable name, and
//! the fence token that authorizes the current tick. There is no
//! anonymous tenant job — a tick without a context is refused by the
//! scheduler, and a context without a principal cannot be built.

use chrono::{DateTime, Utc};

use bot_core::tenant::{ModuleKind, Organization, OrganizationId};

use crate::runtime_registry::FenceToken;

use crate::tenant::context::{ContextOrigin, TenantContext};

use super::job_identity::JobIdentity;

/// Every way job context construction can fail. Fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobContextError {
    /// The tenant context itself was invalid (see
    /// [`crate::tenant::context::ContextError`]).
    InvalidTenantContext(&'static str),
    /// The job name was empty.
    MissingJobName,
}

impl JobContextError {
    /// Stable machine-readable label.
    pub const fn as_str(&self) -> &'static str {
        match self {
            JobContextError::InvalidTenantContext(_) => "invalid_tenant_context",
            JobContextError::MissingJobName => "missing_job_name",
        }
    }
}

impl std::fmt::Display for JobContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobContextError::InvalidTenantContext(detail) => {
                write!(f, "invalid tenant context: {detail}")
            }
            JobContextError::MissingJobName => write!(f, "job context error: missing_job_name"),
        }
    }
}

impl std::error::Error for JobContextError {}

/// The explicit context one background job tick runs under.
#[derive(Debug, Clone)]
pub struct JobContext {
    tenant: TenantContext,
    module: ModuleKind,
    name: &'static str,
    fence: FenceToken,
}

impl JobContext {
    /// Build a job context from the resolved organization record and
    /// the job's identity, bound to the fence token that authorizes
    /// this tick. The tenant principal becomes
    /// `job:<module>:<name>` — explicit, non-anonymous, auditable.
    pub fn new(
        organization: &Organization,
        module: ModuleKind,
        name: &'static str,
        fence: FenceToken,
        at: DateTime<Utc>,
    ) -> Result<Self, JobContextError> {
        if name.trim().is_empty() {
            return Err(JobContextError::MissingJobName);
        }
        let tenant = TenantContext::new(
            organization.clone(),
            TenantContext::job_principal(module.as_str(), name),
            ContextOrigin::Job,
            at,
        )
        .map_err(|_| JobContextError::InvalidTenantContext("principal or tenant rejected"))?;
        // The fence token must belong to the same tenant as the
        // context — a job never borrows another tenant's fence.
        if fence.organization_id != tenant.organization_id() {
            return Err(JobContextError::InvalidTenantContext(
                "fence token belongs to a different tenant",
            ));
        }
        Ok(JobContext {
            tenant,
            module,
            name,
            fence,
        })
    }

    /// The full tenant context (principal, origin, lifecycle snapshot).
    pub fn tenant(&self) -> &TenantContext {
        &self.tenant
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.tenant.organization_id()
    }

    /// The module this job serves.
    pub fn module(&self) -> ModuleKind {
        self.module
    }

    /// The job's stable name.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The fence token authorizing the current tick (re-verified by
    /// the job guard on every tick — this value is a claim).
    pub fn fence_token(&self) -> &FenceToken {
        &self.fence
    }

    /// The stable identity (tenant + module + name).
    pub fn identity(&self) -> JobIdentity {
        JobIdentity::new(self.organization_id(), self.module, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::tenant::{Organization, RuntimeGeneration, RuntimeId};
    use chrono::Utc;

    fn organization() -> Organization {
        Organization::new(OrganizationId::new(), "acme", "Acme", None, Utc::now())
    }

    fn fence_for(organization_id: OrganizationId) -> FenceToken {
        FenceToken {
            organization_id,
            runtime_id: RuntimeId::new(),
            generation: RuntimeGeneration::first(),
        }
    }

    #[test]
    fn a_job_context_carries_an_explicit_tenant_principal() {
        let organization = organization();
        let id = organization.id;
        let context = JobContext::new(
            &organization,
            ModuleKind::Copy,
            "recon",
            fence_for(id),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(context.organization_id(), id);
        assert_eq!(context.tenant().principal(), "job:copy:recon");
        assert_eq!(context.tenant().origin().as_str(), "job");
        assert_eq!(context.name(), "recon");
        assert_eq!(context.module(), ModuleKind::Copy);
        assert_eq!(context.identity().name(), "recon");
    }

    #[test]
    fn an_empty_job_name_is_rejected() {
        let organization = organization();
        let err = JobContext::new(
            &organization,
            ModuleKind::Copy,
            "",
            fence_for(organization.id),
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(err.as_str(), "missing_job_name");
    }

    #[test]
    fn a_foreign_fence_token_is_rejected() {
        let organization = organization();
        let err = JobContext::new(
            &organization,
            ModuleKind::Copy,
            "recon",
            fence_for(OrganizationId::new()),
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(err.as_str(), "invalid_tenant_context");
    }
}
