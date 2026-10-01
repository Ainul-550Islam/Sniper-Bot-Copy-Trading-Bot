//! Core tenant identity/context isolation tests (tenant-isolation
//! file 64).
//!
//! Positive AND negative proofs at the core type system level:
//!
//! * tenant identity: valid slugs parse, invalid ones are refused;
//! * execution scope: nil runtimes refused, organization bound in;
//! * execution context: the ONLY constructor verifies
//!   authority↔scope, wallet ownership and signer ownership — a
//!   wallet or signer belonging to ANOTHER tenant can never be
//!   issued into a context;
//! * lifecycle snapshot: trading/refusal verdicts per status.
//!
//! Pure unit-level (no database): the types fail closed by
//! construction, which is the property every later guard inherits.

use chrono::Utc;

use bot_core::execution::{
    AuthorityChecklist, ExecutionScope, ExecutionTrace, TenantExecutionContext,
    AUTHORITY_CHECK_ORDER,
};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{
    is_valid_tenant_id, parse_tenant_id, Organization, OrganizationId, OrganizationStatus,
    RuntimeGeneration, RuntimeId, SignerProvider, TenantIdentity, TenantSignerRef,
    TenantStateSnapshot, TenantWalletRef,
};

fn organization_id() -> OrganizationId {
    OrganizationId::new()
}

fn runtime_id() -> RuntimeId {
    RuntimeId::new()
}

/// Record every mandated authority check in order and finish for the
/// given scope (the same construction the gateway performs).
fn authority_for(scope: &ExecutionScope) -> bot_core::execution::ExecutionAuthority {
    let now = Utc::now();
    let mut checklist = AuthorityChecklist::new();
    for name in AUTHORITY_CHECK_ORDER {
        checklist
            .record(name, now)
            .expect("mandated checks record in order");
    }
    checklist.finish(scope, now).expect("checklist finishes")
}

fn context_parts(
    organization_id: OrganizationId,
) -> (
    OrganizationId,
    RuntimeId,
    RuntimeGeneration,
    BotModule,
    ExecutionMode,
) {
    (
        organization_id,
        runtime_id(),
        RuntimeGeneration::first(),
        BotModule::Copy,
        ExecutionMode::Paper,
    )
}

#[test]
fn tenant_identity_accepts_valid_slugs_and_refuses_invalid_ones() {
    let id = organization_id();
    assert!(is_valid_tenant_id(id));

    let identity = TenantIdentity::new(id, "acme-trading").unwrap();
    assert_eq!(identity.id(), id);
    assert_eq!(identity.slug(), "acme-trading");
    assert!(identity.is(id));
    assert!(!identity.is(organization_id()));

    for invalid in ["", "   ", "UPPER", "has space", "symbol!"] {
        assert!(
            TenantIdentity::new(id, invalid).is_err(),
            "slug {invalid:?} must be refused"
        );
    }
}

#[test]
fn tenant_id_parsing_round_trips_and_rejects_garbage() {
    let id = organization_id();
    let parsed = parse_tenant_id(&id.to_string()).unwrap();
    assert_eq!(parsed, id);
    assert!(parse_tenant_id("not-a-uuid").is_err());
    assert!(parse_tenant_id("").is_err());
}

#[test]
fn an_execution_scope_binds_the_organization_and_refuses_nil_runtimes() {
    let org = organization_id();
    let (organization_id, runtime, generation, module, mode) = context_parts(org);
    let scope = ExecutionScope::new(organization_id, runtime, generation, module, mode).unwrap();
    assert_eq!(scope.organization_id(), org);
    assert_eq!(scope.module(), module);
    assert_eq!(scope.mode(), mode);

    // A nil runtime id can never scope an execution.
    let nil_runtime = RuntimeId::from(uuid::Uuid::nil());
    assert!(
        ExecutionScope::new(org, nil_runtime, generation, module, mode).is_err(),
        "nil runtime must be refused"
    );
}

#[test]
fn a_context_issues_for_owned_wallet_and_signer() {
    let org = organization_id();
    let (organization_id, runtime, generation, module, mode) = context_parts(org);
    let scope = ExecutionScope::new(organization_id, runtime, generation, module, mode).unwrap();
    let authority = authority_for(&scope);
    let wallet = TenantWalletRef::new(org, "WalletMain1111111111111111111111111111111111111")
        .unwrap()
        .with_label("main");
    let signer = TenantSignerRef::new(org, SignerProvider::Custody, "key-7").unwrap();
    let trace = ExecutionTrace::new("http");

    let context = TenantExecutionContext::issue(
        organization_id,
        runtime,
        generation,
        module,
        mode,
        authority,
        wallet,
        signer,
        trace,
    )
    .unwrap();
    assert_eq!(context.organization_id(), org);
    assert_eq!(context.wallet().label(), Some("main"));
}

#[test]
fn a_wallet_belonging_to_another_tenant_is_never_issued() {
    let org = organization_id();
    let other = organization_id();
    let (organization_id, runtime, generation, module, mode) = context_parts(org);
    let scope = ExecutionScope::new(organization_id, runtime, generation, module, mode).unwrap();
    let authority = authority_for(&scope);
    // The wallet belongs to ANOTHER tenant.
    let foreign_wallet =
        TenantWalletRef::new(other, "WalletOther111111111111111111111111111111111111111")
            .unwrap()
            .with_label("main");
    let signer = TenantSignerRef::new(org, SignerProvider::Custody, "key-7").unwrap();
    let trace = ExecutionTrace::new("http");

    let result = TenantExecutionContext::issue(
        organization_id,
        runtime,
        generation,
        module,
        mode,
        authority,
        foreign_wallet,
        signer,
        trace,
    );
    assert!(result.is_err(), "cross-tenant wallet must be refused");
}

#[test]
fn a_signer_belonging_to_another_tenant_is_never_issued() {
    let org = organization_id();
    let other = organization_id();
    let (organization_id, runtime, generation, module, mode) = context_parts(org);
    let scope = ExecutionScope::new(organization_id, runtime, generation, module, mode).unwrap();
    let authority = authority_for(&scope);
    let wallet = TenantWalletRef::new(org, "WalletMain1111111111111111111111111111111111111")
        .unwrap()
        .with_label("main");
    // The signer belongs to ANOTHER tenant.
    let foreign_signer = TenantSignerRef::new(other, SignerProvider::Custody, "key-7").unwrap();
    let trace = ExecutionTrace::new("http");

    let result = TenantExecutionContext::issue(
        organization_id,
        runtime,
        generation,
        module,
        mode,
        authority,
        wallet,
        foreign_signer,
        trace,
    );
    assert!(result.is_err(), "cross-tenant signer must be refused");
}

#[test]
fn an_authority_for_a_different_scope_never_issues() {
    let org = organization_id();
    let (organization_id, runtime, generation, module, mode) = context_parts(org);
    // The authority was finished for a DIFFERENT runtime's scope.
    let other_scope =
        ExecutionScope::new(organization_id, runtime_id(), generation, module, mode).unwrap();
    let mismatched_authority = authority_for(&other_scope);
    let wallet = TenantWalletRef::new(org, "WalletMain1111111111111111111111111111111111111")
        .unwrap()
        .with_label("main");
    let signer = TenantSignerRef::new(org, SignerProvider::Custody, "key-7").unwrap();
    let trace = ExecutionTrace::new("http");

    let result = TenantExecutionContext::issue(
        organization_id,
        runtime,
        generation,
        module,
        mode,
        mismatched_authority,
        wallet,
        signer,
        trace,
    );
    assert!(result.is_err(), "authority/scope mismatch must be refused");
}

#[test]
fn wallet_references_are_tenant_scoped() {
    let org = organization_id();
    let other = organization_id();
    let address = "WalletMain1111111111111111111111111111111111111";
    let wallet = TenantWalletRef::new(org, address).unwrap();
    assert!(wallet.belongs_to(org));
    assert!(!wallet.belongs_to(other));
    // The same address bound by another tenant is a DIFFERENT
    // reference — address equality never implies cross-tenant access.
    let other_wallet = TenantWalletRef::new(other, address).unwrap();
    assert!(other_wallet.belongs_to(other));
    assert!(!other_wallet.belongs_to(org));
}

#[test]
fn lifecycle_snapshots_decide_trading_read_and_risk_reduction() {
    let active = TenantStateSnapshot::of(OrganizationStatus::Active);
    let trialing = TenantStateSnapshot::of(OrganizationStatus::Trialing);
    let past_due = TenantStateSnapshot::of(OrganizationStatus::PastDue);
    let suspended = TenantStateSnapshot::of(OrganizationStatus::Suspended);
    let closed = TenantStateSnapshot::of(OrganizationStatus::Closed);

    // Active/trialing: full powers.
    assert!(active.may_trade() && active.may_read() && active.may_manage());
    assert!(trialing.may_trade() && trialing.may_read());

    // Past due: reads and reduce-risk continue, new trading refused.
    assert!(!past_due.may_trade());
    assert!(past_due.may_read());

    // Suspended/closed: no new trading.
    assert!(!suspended.may_trade());
    assert!(!closed.may_trade());
    assert!(closed.require_trade().is_err());

    // Snapshots derive from organization rows identically.
    let mut organization = Organization::new(organization_id(), "acme", "Acme", None, Utc::now());
    organization.status = OrganizationStatus::Suspended;
    assert!(!TenantStateSnapshot::of_organization(&organization).may_trade());
}
