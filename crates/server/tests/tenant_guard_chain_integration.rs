//! STEP 3 integration: the guard chain's mandated order and the core
//! execution types it issues.

use bot_core::execution::{
    AuthorityChecklist, ExecutionScope, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
};
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{
    OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
};
use chrono::Utc;

#[test]
fn the_authority_checklist_enforces_the_mandated_order() {
    let mut checklist = AuthorityChecklist::new();
    // Out of order is refused…
    assert!(checklist.record("tenant_config", Utc::now()).is_err());
    // …and the first check lands.
    assert!(checklist
        .record("authenticated_principal", Utc::now())
        .is_ok());
    // Duplicates are idempotent re-records.
    assert!(checklist
        .record("authenticated_principal", Utc::now())
        .is_ok());
}

#[test]
fn a_scope_cannot_be_forged_across_tenants() {
    let org = OrganizationId::new();
    let scope = ExecutionScope::new(
        org,
        RuntimeId::new(),
        RuntimeGeneration::first(),
        BotModule::Copy,
        ExecutionMode::Paper,
    )
    .unwrap();

    // A wallet bound to ANOTHER tenant cannot enter a context for this
    // one: issue() fails closed.
    let mut checklist = AuthorityChecklist::new();
    for check in AUTHORITY_CHECK_ORDER {
        checklist.record(check, Utc::now()).unwrap();
    }
    let authority = checklist.finish(&scope, Utc::now()).unwrap();

    let foreign_wallet = TenantWalletRef::new(
        OrganizationId::new(),
        "ForeignWallet1111111111111111111111111111111111",
    )
    .unwrap();
    let signer =
        TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Custody, "key").unwrap();

    let result = TenantExecutionContext::issue(
        org,
        scope.runtime_id(),
        scope.generation(),
        BotModule::Copy,
        ExecutionMode::Paper,
        authority,
        foreign_wallet,
        signer,
        bot_core::execution::ExecutionTrace::for_request(),
    );
    assert!(
        result.is_err(),
        "a cross-tenant wallet must never issue a context"
    );
}

#[test]
fn an_authority_only_authorizes_its_own_scope() {
    let org = OrganizationId::new();
    let scope = ExecutionScope::new(
        org,
        RuntimeId::new(),
        RuntimeGeneration::first(),
        BotModule::Copy,
        ExecutionMode::Paper,
    )
    .unwrap();
    let mut checklist = AuthorityChecklist::new();
    for check in AUTHORITY_CHECK_ORDER {
        checklist.record(check, Utc::now()).unwrap();
    }
    let authority = checklist.finish(&scope, Utc::now()).unwrap();

    // A different module under the same tenant is NOT covered.
    let other = ExecutionScope::new(
        org,
        scope.runtime_id(),
        scope.generation(),
        BotModule::Sniper,
        ExecutionMode::Paper,
    )
    .unwrap();
    assert!(!authority.authorizes(&other));
    assert!(authority.authorizes(&scope));
}
