//! Authenticated principal → tenant resolution tests (tenant-isolation
//! file 66).
//!
//! Positive: an authenticated credential resolves into a typed
//! [`TenantContext`]; a confirming organization header passes; stream,
//! job and recovery entry paths get explicit principals.
//!
//! Negative: a disagreeing organization header is AMBIGUOUS (deny), a
//! missing/unresolved tenant is MISSING (deny), a blank principal is
//! MISSING (deny), and the context guard refuses a request that names
//! another tenant or a suspended tenant.

use chrono::Utc;

use sniper_suite::tenant::context::{ContextError, ContextOrigin, TenantContext};
use sniper_suite::tenant::context_guard;
use sniper_suite::tenant::context_resolver::{
    AuthenticatedTenant, ContextResolutionError, TenantContextResolver,
};
use sniper_suite::tenant::request::{SignerBinding, TenantExecutionRequest};
use sniper_suite::tenant::DenyReason;

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{Organization, OrganizationId, OrganizationStatus, SignerProvider};

fn organization(status: OrganizationStatus) -> Organization {
    let mut organization =
        Organization::new(OrganizationId::new(), "acme", "Acme", None, Utc::now());
    organization.status = status;
    organization
}

fn authenticated(organization: &Organization) -> AuthenticatedTenant {
    AuthenticatedTenant {
        organization: organization.clone(),
        principal: "user_session:s-1".to_string(),
    }
}

fn request(organization_id: OrganizationId) -> TenantExecutionRequest {
    TenantExecutionRequest::new(
        organization_id,
        BotModule::Copy,
        ExecutionMode::Paper,
        "main",
        SignerBinding {
            provider: SignerProvider::Custody,
            key_ref: "key-7".into(),
        },
    )
}

#[test]
fn an_authenticated_principal_resolves_into_a_typed_context() {
    let organization = organization(OrganizationStatus::Active);
    let id = organization.id;
    let context = TenantContextResolver::new()
        .resolve_http(&authenticated(&organization), None, Utc::now())
        .unwrap();
    assert_eq!(context.organization_id(), id);
    assert_eq!(context.origin(), ContextOrigin::Http);
    assert_eq!(context.principal(), "user_session:s-1");
    assert!(context.state().may_trade());
}

#[test]
fn a_confirming_organization_header_passes() {
    let organization = organization(OrganizationStatus::Active);
    let context = TenantContextResolver::new()
        .resolve_http(
            &authenticated(&organization),
            Some(organization.id),
            Utc::now(),
        )
        .unwrap();
    assert!(context.is(organization.id));
}

#[test]
fn a_disagreeing_organization_header_is_ambiguous_deny() {
    let organization = organization(OrganizationStatus::Active);
    let error = TenantContextResolver::new()
        .resolve_http(
            &authenticated(&organization),
            Some(OrganizationId::new()),
            Utc::now(),
        )
        .unwrap_err();
    assert_eq!(error.as_str(), "ambiguous_tenant");
    assert!(matches!(
        error,
        ContextResolutionError::AmbiguousTenant { .. }
    ));
}

#[test]
fn an_unresolved_tenant_is_a_missing_deny() {
    let mut organization = organization(OrganizationStatus::Active);
    organization.id = OrganizationId(uuid::Uuid::nil());
    let error = TenantContextResolver::new()
        .resolve_http(&authenticated(&organization), None, Utc::now())
        .unwrap_err();
    assert_eq!(error.as_str(), "missing_tenant");
}

#[test]
fn a_blank_principal_is_a_missing_deny_never_anonymous() {
    let organization = organization(OrganizationStatus::Active);
    let mut credential = authenticated(&organization);
    credential.principal = "   ".into();
    let error = TenantContextResolver::new()
        .resolve_http(&credential, None, Utc::now())
        .unwrap_err();
    assert_eq!(error.as_str(), "missing_principal");
}

#[test]
fn stream_job_and_recovery_paths_carry_explicit_principals() {
    let resolver = TenantContextResolver::new();
    let organization = organization(OrganizationStatus::Active);

    let stream = resolver
        .resolve_stream(&authenticated(&organization), None, Utc::now())
        .unwrap();
    assert_eq!(stream.origin(), ContextOrigin::Stream);

    let job = resolver
        .resolve_job(&organization, BotModule::Copy, "recon", Utc::now())
        .unwrap();
    assert_eq!(job.principal(), "job:copy:recon");

    let recovery = resolver
        .resolve_recovery(&organization, Utc::now())
        .unwrap();
    assert_eq!(recovery.principal(), "recovery");
}

#[test]
fn the_context_guard_allows_a_matching_request_for_an_active_tenant() {
    let organization = organization(OrganizationStatus::Active);
    let id = organization.id;
    let context = TenantContextResolver::new()
        .resolve_http(&authenticated(&organization), None, Utc::now())
        .unwrap();
    let outcome = context_guard::check(&context, &request(id));
    assert!(outcome.is_allow(), "{:?}", outcome.deny_reason());
}

#[test]
fn the_context_guard_denies_a_request_naming_another_tenant() {
    let organization = organization(OrganizationStatus::Active);
    let context = TenantContextResolver::new()
        .resolve_http(&authenticated(&organization), None, Utc::now())
        .unwrap();
    let outcome = context_guard::check(&context, &request(OrganizationId::new()));
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "context_issue");
    assert!(matches!(
        outcome.deny_reason().unwrap(),
        DenyReason::ContextIssue { .. }
    ));
}

#[test]
fn the_context_guard_denies_a_suspended_tenant() {
    let organization = organization(OrganizationStatus::Suspended);
    let id = organization.id;
    let context = TenantContextResolver::new()
        .resolve_http(&authenticated(&organization), None, Utc::now())
        .unwrap();
    let outcome = context_guard::check(&context, &request(id));
    assert_eq!(outcome.deny_reason().unwrap().as_str(), "tenant_state");
}

#[test]
fn a_context_is_never_constructible_without_a_principal() {
    let organization = organization(OrganizationStatus::Active);
    let error = TenantContext::new(organization, "", ContextOrigin::Http, Utc::now()).unwrap_err();
    assert_eq!(error, ContextError::MissingPrincipal);
    assert_eq!(error.as_str(), "missing_principal");
}
