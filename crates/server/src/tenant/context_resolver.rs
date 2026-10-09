//! Principal → tenant resolution (tenant-isolation file 17).
//!
//! [`TenantContextResolver`] turns an authenticated credential into a
//! [`TenantContext`]. It is the ONLY sanctioned path from the SaaS
//! request layer (which authenticated the caller and resolved the
//! organization) to the execution layer's typed context — and it
//! rejects the two failure classes the isolation program calls out:
//!
//! * **missing** tenant mapping — a credential with no resolved
//!   organization never yields a context (there is no default
//!   tenant);
//! * **ambiguous** tenant mapping — a credential whose stated
//!   organization disagrees with its resolved organization (or a
//!   request header naming a tenant the credential does not act
//!   for) is a hard error, not a "best guess".
//!
//! The resolver also mints contexts for the non-HTTP entry paths:
//! background jobs and recovery passes, always with an explicit
//! principal so no anonymous tenant work exists.

use chrono::{DateTime, Utc};

use bot_core::tenant::{ModuleKind, Organization, OrganizationId};

use super::context::{ContextError, ContextOrigin, TenantContext};

/// What an authenticated request hands the resolver: the RESOLVED
/// organization record and the authenticated principal label, exactly
/// as the SaaS middleware produces them (`SaasContext::organization`
/// and `SaasContext::actor_label`). Keeping this as a plain input
/// type decouples the execution layer from the request layer's
/// plumbing — the bin side builds it in one expression.
#[derive(Debug, Clone)]
pub struct AuthenticatedTenant {
    /// The tenant the credential authenticated for (resolved, never
    /// caller-supplied).
    pub organization: Organization,
    /// The non-secret principal label (`user:<id>`, `apikey:<prefix>`).
    pub principal: String,
}

/// Every way principal → tenant resolution can fail. Fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextResolutionError {
    /// The credential carried no principal at all.
    MissingPrincipal,
    /// No organization was resolved for the credential.
    MissingTenant,
    /// The credential acts for more than one tenant and no valid
    /// selection was made, or the requested tenant disagrees with the
    /// authenticated one.
    AmbiguousTenant { detail: String },
    /// The context itself could not be constructed.
    InvalidContext(ContextError),
}

impl ContextResolutionError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ContextResolutionError::MissingPrincipal => "missing_principal",
            ContextResolutionError::MissingTenant => "missing_tenant",
            ContextResolutionError::AmbiguousTenant { .. } => "ambiguous_tenant",
            ContextResolutionError::InvalidContext(_) => "invalid_context",
        }
    }
}

impl std::fmt::Display for ContextResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextResolutionError::AmbiguousTenant { detail } => {
                write!(f, "ambiguous tenant: {detail}")
            }
            other => write!(f, "tenant resolution error: {}", other.as_str()),
        }
    }
}

impl std::error::Error for ContextResolutionError {}

impl From<ContextError> for ContextResolutionError {
    fn from(error: ContextError) -> Self {
        ContextResolutionError::InvalidContext(error)
    }
}

/// Resolves authenticated credentials into tenant contexts.
#[derive(Debug, Clone, Copy, Default)]
pub struct TenantContextResolver;

impl TenantContextResolver {
    /// The shared resolver (stateless — construction is trivial).
    pub fn new() -> Self {
        TenantContextResolver
    }

    /// Resolve an authenticated request into a context.
    ///
    /// `requested_organization` is what the caller asked to act for
    /// (the `x-organization` header). It may only ever CONFIRM the
    /// authenticated organization — any disagreement is an
    /// [`ContextResolutionError::AmbiguousTenant`], never a widening.
    pub fn resolve_http(
        &self,
        authenticated: &AuthenticatedTenant,
        requested_organization: Option<OrganizationId>,
        at: DateTime<Utc>,
    ) -> Result<TenantContext, ContextResolutionError> {
        // Missing principal: the middleware never produces these, but
        // the resolver fails closed regardless.
        if authenticated.principal.trim().is_empty() {
            return Err(ContextResolutionError::MissingPrincipal);
        }

        // Ambiguity: the header must match the authenticated tenant.
        if let Some(requested) = requested_organization {
            if requested != authenticated.organization.id {
                return Err(ContextResolutionError::AmbiguousTenant {
                    detail: "requested organization is not the authenticated organization"
                        .to_string(),
                });
            }
        }

        // Missing tenant: an unresolved organization is a hard error.
        if authenticated.organization.id.as_uuid().is_nil() {
            return Err(ContextResolutionError::MissingTenant);
        }

        Ok(TenantContext::new(
            authenticated.organization.clone(),
            authenticated.principal.clone(),
            ContextOrigin::Http,
            at,
        )?)
    }

    /// Resolve a stream/WS consumer acting for an authenticated
    /// tenant. Same rules as HTTP: the requested organization must
    /// agree with the authenticated one.
    pub fn resolve_stream(
        &self,
        authenticated: &AuthenticatedTenant,
        requested_organization: Option<OrganizationId>,
        at: DateTime<Utc>,
    ) -> Result<TenantContext, ContextResolutionError> {
        let context = self.resolve_http(authenticated, requested_organization, at)?;
        // Re-tag the origin as stream (same construction rules).
        Ok(TenantContext::new(
            context.organization().clone(),
            context.principal(),
            ContextOrigin::Stream,
            at,
        )?)
    }

    /// Resolve a background job's context. The organization record
    /// must come from the registry/lookup the caller performed — a
    /// context is never built from a bare id the caller "knows".
    pub fn resolve_job(
        &self,
        organization: &Organization,
        module: ModuleKind,
        job_name: &'static str,
        at: DateTime<Utc>,
    ) -> Result<TenantContext, ContextResolutionError> {
        if organization.id.as_uuid().is_nil() {
            return Err(ContextResolutionError::MissingTenant);
        }
        Ok(TenantContext::new(
            organization.clone(),
            TenantContext::job_principal(module.as_str(), job_name),
            ContextOrigin::Job,
            at,
        )?)
    }

    /// Resolve a recovery pass context (operator-triggered repair
    /// work on one tenant's data).
    pub fn resolve_recovery(
        &self,
        organization: &Organization,
        at: DateTime<Utc>,
    ) -> Result<TenantContext, ContextResolutionError> {
        if organization.id.as_uuid().is_nil() {
            return Err(ContextResolutionError::MissingTenant);
        }
        Ok(TenantContext::new(
            organization.clone(),
            TenantContext::RECOVERY_PRINCIPAL,
            ContextOrigin::Recovery,
            at,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn authenticated(organization: &Organization) -> AuthenticatedTenant {
        AuthenticatedTenant {
            organization: organization.clone(),
            principal: "user_session:s-1".to_string(),
        }
    }

    fn organization() -> Organization {
        Organization::new(OrganizationId::new(), "acme", "Acme", None, Utc::now())
    }

    #[test]
    fn an_authenticated_request_resolves_into_a_context() {
        let organization = organization();
        let tenant = authenticated(&organization);
        let context = TenantContextResolver::new()
            .resolve_http(&tenant, None, Utc::now())
            .unwrap();
        assert_eq!(context.organization_id(), organization.id);
        assert_eq!(context.origin().as_str(), "http");
        assert_eq!(context.principal(), "user_session:s-1");
    }

    #[test]
    fn a_matching_organization_header_confirms() {
        let organization = organization();
        let tenant = authenticated(&organization);
        let context = TenantContextResolver::new()
            .resolve_http(&tenant, Some(organization.id), Utc::now())
            .unwrap();
        assert!(context.is(organization.id));
    }

    #[test]
    fn a_disagreeing_organization_header_is_ambiguous() {
        let organization = organization();
        let tenant = authenticated(&organization);
        let err = TenantContextResolver::new()
            .resolve_http(&tenant, Some(OrganizationId::new()), Utc::now())
            .unwrap_err();
        assert_eq!(err.as_str(), "ambiguous_tenant");
    }

    #[test]
    fn a_nil_organization_is_a_missing_tenant() {
        let mut organization = organization();
        organization.id = OrganizationId(uuid::Uuid::nil());
        let tenant = authenticated(&organization);
        let err = TenantContextResolver::new()
            .resolve_http(&tenant, None, Utc::now())
            .unwrap_err();
        assert_eq!(err.as_str(), "missing_tenant");
    }

    #[test]
    fn a_blank_principal_is_missing_not_anonymous() {
        let organization = organization();
        let mut tenant = authenticated(&organization);
        tenant.principal = "   ".into();
        let err = TenantContextResolver::new()
            .resolve_http(&tenant, None, Utc::now())
            .unwrap_err();
        assert_eq!(err.as_str(), "missing_principal");
    }

    #[test]
    fn stream_resolution_retags_the_origin() {
        let organization = organization();
        let tenant = authenticated(&organization);
        let context = TenantContextResolver::new()
            .resolve_stream(&tenant, None, Utc::now())
            .unwrap();
        assert_eq!(context.origin().as_str(), "stream");
    }

    #[test]
    fn job_and_recovery_contexts_carry_explicit_principals() {
        let resolver = TenantContextResolver::new();
        let organization = organization();
        let module = ModuleKind::Copy;
        let job = resolver
            .resolve_job(&organization, module, "recon", Utc::now())
            .unwrap();
        assert_eq!(job.principal(), "job:copy:recon");
        assert_eq!(job.origin().as_str(), "job");

        let recovery = resolver
            .resolve_recovery(&organization, Utc::now())
            .unwrap();
        assert_eq!(recovery.principal(), "recovery");
        assert_eq!(recovery.origin().as_str(), "recovery");
    }
}
