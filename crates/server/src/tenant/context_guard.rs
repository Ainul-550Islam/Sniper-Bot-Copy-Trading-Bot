//! The tenant context guard (tenant-isolation file 18).
//!
//! The first guard of the chain when a [`TenantContext`] is available:
//! it guarantees the context is well-formed AND that the request it
//! carries actually belongs to the context's tenant, before any
//! tenant-sensitive operation runs. It is the typed replacement for
//! the gateway's inline "authenticated principal / tenant context"
//! steps — the gateway still performs them (defense in depth), but
//! entry paths that hold a context check here FIRST and never build
//! a mismatched request at all.
//!
//! Checks, in order (first deny wins):
//!
//! 1. the principal is present (contexts are constructed non-anonymous,
//!    but a guard never trusts construction alone);
//! 2. the request's organization IS the context's organization —
//!    a request for another tenant is a hard deny, not a redirect;
//! 3. the tenant's lifecycle permits new trading activity
//!    (delegated to [`super::tenant_guard`], the lifecycle gate).

use bot_core::tenant::OrganizationId;

use super::context::TenantContext;
use super::decision::{DenyReason, GuardOutcome};
use super::request::TenantExecutionRequest;
use super::tenant_guard;

/// Verify the context is valid for this request.
pub fn check(context: &TenantContext, request: &TenantExecutionRequest) -> GuardOutcome {
    // 1. authenticated principal (fail closed on blank).
    if context.principal().trim().is_empty() {
        return GuardOutcome::Deny(DenyReason::ContextIssue {
            reason: "missing principal",
        });
    }

    // 2. the request belongs to the context's tenant — exactly.
    if !context.is(request.organization_id) {
        return GuardOutcome::Deny(DenyReason::ContextIssue {
            reason: "request tenant does not match the resolved organization",
        });
    }

    // 3. the tenant's lifecycle state (delegate to the lifecycle gate).
    tenant_guard::check(context.organization())
}

/// Verify a context is usable for tenant-scoped work that is NOT an
/// execution request (stream subscriptions, background ticks): the
/// principal is present and the tenant was resolved at all.
pub fn check_present(context: &TenantContext) -> GuardOutcome {
    if context.principal().trim().is_empty() {
        return GuardOutcome::Deny(DenyReason::ContextIssue {
            reason: "missing principal",
        });
    }
    GuardOutcome::Allow("context_present")
}

/// The organization a context is bound to (helper for call sites that
/// then scope repository reads).
pub fn bound_organization(context: &TenantContext) -> OrganizationId {
    context.organization_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::request::{SignerBinding, TenantExecutionRequest};
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::tenant::{Organization, OrganizationStatus, SignerProvider};
    use chrono::Utc;

    use super::super::context::ContextOrigin;

    fn organization(status: OrganizationStatus) -> Organization {
        let mut organization =
            Organization::new(OrganizationId::new(), "acme", "Acme", None, Utc::now());
        organization.status = status;
        organization
    }

    fn request(organization_id: OrganizationId) -> TenantExecutionRequest {
        TenantExecutionRequest::new(
            organization_id,
            BotModule::Copy,
            ExecutionMode::Paper,
            "main",
            SignerBinding {
                provider: SignerProvider::Custody,
                key_ref: "key-1".into(),
            },
        )
    }

    #[test]
    fn a_matching_active_context_allows() {
        let organization = organization(OrganizationStatus::Active);
        let id = organization.id;
        let context =
            TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap();
        let outcome = check(&context, &request(id));
        assert!(outcome.is_allow(), "{:?}", outcome.deny_reason());
    }

    #[test]
    fn a_request_for_another_tenant_is_denied() {
        let organization = organization(OrganizationStatus::Active);
        let context =
            TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap();
        let outcome = check(&context, &request(OrganizationId::new()));
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "context_issue");
    }

    #[test]
    fn a_suspended_tenant_is_denied_at_the_lifecycle_step() {
        let organization = organization(OrganizationStatus::Suspended);
        let id = organization.id;
        let context =
            TenantContext::new(organization, "user:1", ContextOrigin::Http, Utc::now()).unwrap();
        let outcome = check(&context, &request(id));
        assert_eq!(outcome.deny_reason().unwrap().as_str(), "tenant_state");
    }

    #[test]
    fn a_context_can_never_be_constructed_without_a_principal() {
        // The guard's first rule (blank principal -> deny) is backed by
        // an even stronger invariant: the constructor refuses to build
        // an anonymous context in the first place. Both layers hold.
        let organization = organization(OrganizationStatus::Active);
        for blank in ["", "   "] {
            assert_eq!(
                TenantContext::new(organization.clone(), blank, ContextOrigin::Http, Utc::now())
                    .unwrap_err()
                    .as_str(),
                "missing_principal"
            );
        }
    }

    #[test]
    fn check_present_rejects_nothing_valid_and_stays_typed() {
        let organization = organization(OrganizationStatus::Active);
        let context = TenantContext::new(
            organization,
            "job:copy:recon",
            ContextOrigin::Job,
            Utc::now(),
        )
        .unwrap();
        assert!(check_present(&context).is_allow());
        assert_eq!(bound_organization(&context), context.organization_id());
    }
}
