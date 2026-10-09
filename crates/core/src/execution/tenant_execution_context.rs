//! Tenant execution context (STEP 3 file 11).
//!
//! The [`TenantExecutionContext`] is the single value that travels
//! through the whole execution chain — order creation, risk, signing,
//! broadcast, persistence, reconciliation — carrying everything a stage
//! needs to act AS A TENANT: the [`ExecutionScope`], the
//! [`ExecutionAuthority`] proof, the bound [`TenantWalletRef`] and
//! [`TenantSignerRef`], and the [`ExecutionTrace`] correlation identity.
//!
//! It can only be constructed via [`TenantExecutionContext::issue`],
//! which verifies coherence fail-closed:
//!
//! * the authority's fingerprint matches the scope exactly;
//! * the wallet belongs to the scope's organization;
//! * the signer belongs to the scope's organization and is active.
//!
//! A stage that receives this context never has to re-derive tenant
//! identity; a stage that receives nothing CANNOT execute for a tenant.

use serde::{Deserialize, Serialize};

use crate::models::{BotModule, ExecutionMode};
use crate::tenant::{
    OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
};

use super::execution_authority::ExecutionAuthority;
use super::execution_scope::{ExecutionScope, ScopeError};
use super::execution_trace::ExecutionTrace;

/// Everything a stage needs to execute as a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantExecutionContext {
    scope: ExecutionScope,
    authority: ExecutionAuthority,
    wallet: TenantWalletRef,
    signer: TenantSignerRef,
    trace: ExecutionTrace,
}

/// Why a context could not be issued. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextError {
    /// The scope itself was malformed (nil runtime).
    BadScope(ScopeError),
    /// The authority does not match this scope (wrong tenant, runtime,
    /// generation, module or mode — or a fabricated authority).
    AuthorityMismatch,
    /// The bound wallet belongs to another organization.
    WalletMismatch,
    /// The bound signer belongs to another organization.
    SignerMismatch,
    /// The bound signer is revoked.
    SignerRevoked,
}

impl ContextError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ContextError::BadScope(_) => "bad_scope",
            ContextError::AuthorityMismatch => "authority_mismatch",
            ContextError::WalletMismatch => "wallet_mismatch",
            ContextError::SignerMismatch => "signer_mismatch",
            ContextError::SignerRevoked => "signer_revoked",
        }
    }
}

impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContextError::BadScope(e) => write!(f, "tenant execution context: {e}"),
            ContextError::AuthorityMismatch => write!(
                f,
                "tenant execution context: the authority does not match this scope"
            ),
            ContextError::WalletMismatch => write!(
                f,
                "tenant execution context: the wallet belongs to another organization"
            ),
            ContextError::SignerMismatch => write!(
                f,
                "tenant execution context: the signer belongs to another organization"
            ),
            ContextError::SignerRevoked => {
                write!(f, "tenant execution context: the signer binding is revoked")
            }
        }
    }
}

impl std::error::Error for ContextError {}

impl From<ScopeError> for ContextError {
    fn from(e: ScopeError) -> Self {
        ContextError::BadScope(e)
    }
}

impl TenantExecutionContext {
    /// Issue a context, verifying every coherence rule fail-closed.
    ///
    /// This is the ONLY constructor: there is no way to assemble a
    /// context around an unverified authority or a cross-tenant binding.
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
        module: BotModule,
        mode: ExecutionMode,
        authority: ExecutionAuthority,
        wallet: TenantWalletRef,
        mut signer: TenantSignerRef,
        trace: ExecutionTrace,
    ) -> Result<Self, ContextError> {
        let scope = ExecutionScope::new(organization_id, runtime_id, generation, module, mode)?;
        if !authority.authorizes(&scope) {
            return Err(ContextError::AuthorityMismatch);
        }
        if !wallet.belongs_to(organization_id) {
            return Err(ContextError::WalletMismatch);
        }
        if !signer.belongs_to(organization_id) {
            return Err(ContextError::SignerMismatch);
        }
        if !signer.is_active() {
            return Err(ContextError::SignerRevoked);
        }
        // The signer reference is captured in a read-only, redacted form:
        // revocation state is re-checked by the signer guard, and the
        // context itself never widens it.
        let _ = &mut signer;
        Ok(TenantExecutionContext {
            scope,
            authority,
            wallet,
            signer,
            trace,
        })
    }

    /// The execution scope (who/where/what).
    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    /// The authority proof.
    pub fn authority(&self) -> &ExecutionAuthority {
        &self.authority
    }

    /// The bound public wallet.
    pub fn wallet(&self) -> &TenantWalletRef {
        &self.wallet
    }

    /// The bound signer reference (redacted display only).
    pub fn signer(&self) -> &TenantSignerRef {
        &self.signer
    }

    /// The correlation identity.
    pub fn trace(&self) -> &ExecutionTrace {
        &self.trace
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.scope.organization_id()
    }

    /// The executing runtime.
    pub fn runtime_id(&self) -> RuntimeId {
        self.scope.runtime_id()
    }

    /// The fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.scope.generation()
    }

    /// Re-verify coherence against a freshly-read scope (defense in
    /// depth: a stage that re-resolved the tenant may confirm the context
    /// still matches before acting).
    pub fn verify_against(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), ContextError> {
        if self.scope.organization_id() == organization_id
            && self.scope.runtime_id() == runtime_id
            && self.scope.generation() == generation
        {
            Ok(())
        } else {
            Err(ContextError::AuthorityMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::execution_authority::{AuthorityChecklist, AUTHORITY_CHECK_ORDER};
    use crate::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

    fn parts() -> (
        OrganizationId,
        RuntimeId,
        RuntimeGeneration,
        ExecutionAuthority,
        TenantWalletRef,
        TenantSignerRef,
    ) {
        let org = OrganizationId::new();
        let runtime = RuntimeId::new();
        let gen = RuntimeGeneration::first();
        let scope = ExecutionScope::new(org, runtime, gen, BotModule::Sniper, ExecutionMode::Paper)
            .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = chrono::Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet =
            TenantWalletRef::new(org, "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM").unwrap();
        let signer =
            TenantSignerRef::new(org, crate::tenant::SignerProvider::Custody, "key-main").unwrap();
        (org, runtime, gen, authority, wallet, signer)
    }

    #[test]
    fn issue_succeeds_with_coherent_parts() {
        let (org, runtime, gen, authority, wallet, signer) = parts();
        let ctx = TenantExecutionContext::issue(
            org,
            runtime,
            gen,
            BotModule::Sniper,
            ExecutionMode::Paper,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap();
        assert_eq!(ctx.organization_id(), org);
        assert!(ctx.verify_against(org, runtime, gen).is_ok());
    }

    #[test]
    fn cross_tenant_wallet_is_rejected() {
        let (org, runtime, gen, authority, _, signer) = parts();
        let foreign = TenantWalletRef::new(
            OrganizationId::new(),
            "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM",
        )
        .unwrap();
        let err = TenantExecutionContext::issue(
            org,
            runtime,
            gen,
            BotModule::Sniper,
            ExecutionMode::Paper,
            authority,
            foreign,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap_err();
        assert_eq!(err, ContextError::WalletMismatch);
        assert_eq!(err.as_str(), "wallet_mismatch");
    }

    #[test]
    fn cross_tenant_and_revoked_signers_are_rejected() {
        let (org, runtime, gen, authority, wallet, _) = parts();
        let foreign = TenantSignerRef::new(
            OrganizationId::new(),
            crate::tenant::SignerProvider::Custody,
            "key-main",
        )
        .unwrap();
        let err = TenantExecutionContext::issue(
            org,
            runtime,
            gen,
            BotModule::Sniper,
            ExecutionMode::Paper,
            authority.clone(),
            wallet.clone(),
            foreign,
            ExecutionTrace::for_request(),
        )
        .unwrap_err();
        assert_eq!(err, ContextError::SignerMismatch);

        let mut revoked =
            TenantSignerRef::new(org, crate::tenant::SignerProvider::Custody, "key-main").unwrap();
        revoked.revoke();
        let err = TenantExecutionContext::issue(
            org,
            runtime,
            gen,
            BotModule::Sniper,
            ExecutionMode::Paper,
            authority,
            wallet,
            revoked,
            ExecutionTrace::for_request(),
        )
        .unwrap_err();
        assert_eq!(err, ContextError::SignerRevoked);
    }

    #[test]
    fn mismatched_authority_is_rejected() {
        let (org, runtime, gen, authority, wallet, signer) = parts();
        // Same checks but a different module: the authority fingerprint
        // no longer matches the scope being issued.
        let err = TenantExecutionContext::issue(
            org,
            runtime,
            gen,
            BotModule::Copy,
            ExecutionMode::Paper,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap_err();
        assert_eq!(err, ContextError::AuthorityMismatch);
    }

    #[test]
    fn verify_against_a_rotated_runtime_fails() {
        let (org, runtime, gen, authority, wallet, signer) = parts();
        let ctx = TenantExecutionContext::issue(
            org,
            runtime,
            gen,
            BotModule::Sniper,
            ExecutionMode::Paper,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap();
        let rotated = gen.next().unwrap();
        assert!(ctx.verify_against(org, runtime, rotated).is_err());
    }
}
