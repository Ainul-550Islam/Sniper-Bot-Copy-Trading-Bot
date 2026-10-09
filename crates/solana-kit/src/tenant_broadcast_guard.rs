//! Tenant broadcast guard (PROMPT 4/10 file 27).
//!
//! [`TenantBroadcastGuard`] is the FINAL tenant-safety gate immediately
//! before network submission. An [`Executor`] built for a tenant carries
//! one via [`crate::execute::Executor::with_tenant_guard`]; from that
//! moment every transaction the executor is asked to run — built or
//! prebuilt, live or paper — must carry tenant metadata that matches the
//! guard's [`TenantSigningContext`] exactly:
//!
//! * same organization (never another tenant's money path);
//! * same runtime AND fencing generation (a rotated/fenced runtime's
//!   in-flight work dies here, not on-chain);
//! * same module (a Copy order cannot ride a Sniper-issued context);
//! * same funding wallet (the executor's wallet is the bound wallet);
//! * the signer binding is still active.
//!
//! A deny is fail-closed: the transaction is NOT broadcast, the execution
//! ledger records a policy veto, and the deny reason is machine-readable.
//! There is no code path in which a guard error degrades into an
//! unguarded broadcast.
//!
//! Deployment-global executors (the existing single-operator mode) simply
//! do not attach a guard: their behaviour is byte-for-byte unchanged.

use bot_core::obs::metrics;
use bot_core::tenant::{ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId};
use solana_sdk::pubkey::Pubkey;
use std::sync::Arc;

use crate::execute::ExecutionResult;
use crate::tenant_signing_context::{TenantSigningContext, TenantSigningDeny};
use crate::tenant_transaction::TenantTransactionMeta;
use crate::tx::{BuiltTx, TxRequest};

/// Why the guard refused a broadcast. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantBroadcastDeny {
    /// The request carries no tenant metadata (a tenant-bound executor
    /// cannot run anonymous work).
    MissingTenantMeta,
    /// The tenant metadata belongs to another organization.
    OrganizationMismatch {
        expected: OrganizationId,
        presented: OrganizationId,
    },
    /// The runtime id does not match the guard's context.
    RuntimeMismatch,
    /// The fencing generation does not match — the runtime was rotated or
    /// fenced after this work was authorized.
    GenerationMismatch,
    /// The module does not match the context's module.
    ModuleMismatch {
        expected: ModuleKind,
        presented: ModuleKind,
    },
    /// The executor's funding wallet is not the wallet bound to the
    /// context.
    WalletMismatch,
    /// The signer binding is revoked.
    SignerRevoked,
}

impl TenantBroadcastDeny {
    /// Stable machine-readable label (logs, metrics, audit events).
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantBroadcastDeny::MissingTenantMeta => "missing_tenant_meta",
            TenantBroadcastDeny::OrganizationMismatch { .. } => "organization_mismatch",
            TenantBroadcastDeny::RuntimeMismatch => "runtime_mismatch",
            TenantBroadcastDeny::GenerationMismatch => "generation_mismatch",
            TenantBroadcastDeny::ModuleMismatch { .. } => "module_mismatch",
            TenantBroadcastDeny::WalletMismatch => "wallet_mismatch",
            TenantBroadcastDeny::SignerRevoked => "signer_revoked",
        }
    }

    /// Human-readable detail for the execution ledger's veto record.
    /// Always starts with the stable label so log grep and audit queries
    /// work on the machine-readable form too.
    pub fn detail(&self) -> String {
        let label = self.as_str();
        match self {
            TenantBroadcastDeny::MissingTenantMeta => {
                format!("{label}: request carries no tenant metadata")
            }
            TenantBroadcastDeny::OrganizationMismatch {
                expected,
                presented,
            } => format!("{label}: organization {presented} is not the bound tenant {expected}"),
            TenantBroadcastDeny::RuntimeMismatch => {
                format!("{label}: runtime id does not match the bound context")
            }
            TenantBroadcastDeny::GenerationMismatch => {
                format!("{label}: fencing generation is stale (runtime rotated)")
            }
            TenantBroadcastDeny::ModuleMismatch {
                expected,
                presented,
            } => format!("{label}: module {presented} is not the context module {expected}"),
            TenantBroadcastDeny::WalletMismatch => {
                format!("{label}: funding wallet is not the bound wallet")
            }
            TenantBroadcastDeny::SignerRevoked => format!("{label}: signer binding is revoked"),
        }
    }
}

impl std::fmt::Display for TenantBroadcastDeny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail())
    }
}

impl std::error::Error for TenantBroadcastDeny {}

/// The final tenant gate in front of signing and broadcast.
#[derive(Debug, Clone)]
pub struct TenantBroadcastGuard {
    context: TenantSigningContext,
    funding_wallet: Pubkey,
}

impl TenantBroadcastGuard {
    /// Build the guard for one tenant module instance: the signing context
    /// its executions were issued under, and the funding wallet the
    /// executor was constructed with (verified to be the bound wallet).
    pub fn new(
        context: TenantSigningContext,
        funding_wallet: Pubkey,
    ) -> Result<Self, TenantSigningDeny> {
        context.authorize_wallet(&funding_wallet)?;
        Ok(TenantBroadcastGuard {
            context,
            funding_wallet,
        })
    }

    /// The tenant signing context this guard enforces.
    pub fn context(&self) -> &TenantSigningContext {
        &self.context
    }

    /// The bound funding wallet.
    pub fn funding_wallet(&self) -> &Pubkey {
        &self.funding_wallet
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.context.organization_id()
    }

    /// The executing runtime.
    pub fn runtime_id(&self) -> RuntimeId {
        self.context.runtime_id()
    }

    /// The fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.context.generation()
    }

    /// Verify tenant metadata against every guard rule. This is the check
    /// the executor runs immediately before signing/broadcast.
    pub fn authorize_meta(&self, meta: &TenantTransactionMeta) -> Result<(), TenantBroadcastDeny> {
        if !self.context.execution_context().signer().is_active() {
            return Err(TenantBroadcastDeny::SignerRevoked);
        }
        if self.context.wallet_address() != self.funding_wallet.to_string() {
            return Err(TenantBroadcastDeny::WalletMismatch);
        }
        if meta.organization_id() != self.context.organization_id() {
            return Err(TenantBroadcastDeny::OrganizationMismatch {
                expected: self.context.organization_id(),
                presented: meta.organization_id(),
            });
        }
        if meta.runtime_id() != self.context.runtime_id() {
            return Err(TenantBroadcastDeny::RuntimeMismatch);
        }
        if meta.generation() != self.context.generation() {
            return Err(TenantBroadcastDeny::GenerationMismatch);
        }
        if meta.module() != self.context.module() {
            return Err(TenantBroadcastDeny::ModuleMismatch {
                expected: self.context.module(),
                presented: meta.module(),
            });
        }
        Ok(())
    }

    /// Authorize a [`TxRequest`] (built path): it must carry tenant
    /// metadata that satisfies [`TenantBroadcastGuard::authorize_meta`].
    pub fn authorize(&self, req: &TxRequest) -> Result<(), TenantBroadcastDeny> {
        let meta = req
            .tenant
            .as_ref()
            .ok_or(TenantBroadcastDeny::MissingTenantMeta)?;
        self.authorize_meta(meta)
    }

    /// Authorize a prebuilt transaction (Jupiter / externally signed
    /// path): the metadata captured at build time must satisfy the same
    /// rules.
    pub fn authorize_prebuilt(&self, built: &BuiltTx) -> Result<(), TenantBroadcastDeny> {
        let meta = built
            .tenant
            .as_ref()
            .ok_or(TenantBroadcastDeny::MissingTenantMeta)?;
        self.authorize_meta(meta)
    }

    /// Record a deny (counter + structured log line). Called by the
    /// executor's veto path so a denied broadcast is observable.
    pub fn record_deny(&self, deny: &TenantBroadcastDeny) {
        metrics::global()
            .counter(
                "bot_tenant_broadcast_denies_total",
                "Tenant broadcasts refused by the final guard, by deny reason.",
                &[("reason", deny.as_str())],
            )
            .inc();
        tracing::warn!(
            organization = %self.context.organization_id(),
            runtime = %self.context.runtime_id(),
            generation = %self.context.generation(),
            reason = deny.as_str(),
            "{}", deny.detail()
        );
    }

    /// The veto result the executor returns for a denied request: a
    /// policy-veto failure that never left the process.
    pub fn veto_result(&self, label: &str, deny: &TenantBroadcastDeny) -> ExecutionResult {
        let mut r = ExecutionResult::empty(label, "", false);
        r.error = Some(deny.detail());
        r.state = bot_core::execution::ExecutionState::Failed;
        r.failure = Some(bot_core::execution::FailureClass::PolicyVeto);
        r
    }
}

/// Convenience: build a guard-shared executor attachment.
///
/// `executor.with_tenant_guard(Arc::new(guard))` is the plain form; this
/// helper exists for the module factories that construct the context and
/// wallet together.
pub fn guard_for(
    context: TenantSigningContext,
    funding_wallet: &crate::tokens::Wallet,
) -> Result<Arc<TenantBroadcastGuard>, TenantSigningDeny> {
    Ok(Arc::new(TenantBroadcastGuard::new(
        context,
        funding_wallet.pubkey,
    )?))
}

#[cfg(test)]
mod tests {
    //! Guard authorization tests: wrong organization / runtime /
    //! generation / module / wallet are all denied; the matching context
    //! is authorized. Contexts are issued through the real core path.

    use super::*;
    use std::str::FromStr;

    use bot_core::execution::{
        AuthorityChecklist, ExecutionTrace, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
    };
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };

    fn issued(
        module: BotModule,
        org: OrganizationId,
        runtime: RuntimeId,
        generation: RuntimeGeneration,
    ) -> TenantSigningContext {
        issued_for_wallet(module, org, runtime, generation, &bound_address())
    }

    fn issued_for_wallet(
        module: BotModule,
        org: OrganizationId,
        runtime: RuntimeId,
        generation: RuntimeGeneration,
        address: &str,
    ) -> TenantSigningContext {
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            generation,
            module,
            ExecutionMode::Paper,
        )
        .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = chrono::Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet = TenantWalletRef::new(org, address).unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "sniper-key")
                .unwrap();
        TenantSigningContext::new(
            TenantExecutionContext::issue(
                org,
                runtime,
                generation,
                module,
                ExecutionMode::Paper,
                authority,
                wallet,
                signer,
                ExecutionTrace::for_request(),
            )
            .unwrap(),
        )
    }

    fn parts() -> (OrganizationId, RuntimeId, RuntimeGeneration) {
        (
            OrganizationId::new(),
            RuntimeId::new(),
            RuntimeGeneration::first(),
        )
    }

    /// A REAL pubkey (32-byte base58): generated, so `Pubkey::from_str`
    /// round-trips exactly like a production wallet address.
    fn bound_address() -> String {
        use solana_sdk::signer::Signer;
        solana_sdk::signature::Keypair::new().pubkey().to_string()
    }

    fn loaded_wallet() -> crate::tokens::Wallet {
        use solana_sdk::signer::Signer;
        let kp = solana_sdk::signature::Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let wallet = crate::tokens::Wallet::load(&b58).expect("wallet must load");
        assert_eq!(wallet.pubkey, kp.pubkey());
        wallet
    }

    fn guard_for_module(module: BotModule) -> TenantBroadcastGuard {
        let (org, runtime, gen) = parts();
        let address = bound_address();
        let ctx = issued_for_wallet(module, org, runtime, gen, &address);
        let wallet = Pubkey::from_str(&address).unwrap();
        TenantBroadcastGuard::new(ctx, wallet).unwrap()
    }

    fn request_with(meta: Option<TenantTransactionMeta>) -> TxRequest {
        let mut req = TxRequest::new("snipe-entry");
        if let Some(m) = meta {
            req = req.tenant(m);
        }
        req
    }

    #[test]
    fn guard_construction_rejects_a_foreign_funding_wallet() {
        let (org, runtime, gen) = parts();
        let ctx = issued(BotModule::Sniper, org, runtime, gen);
        let err = TenantBroadcastGuard::new(ctx, Pubkey::new_unique());
        assert_eq!(err.err().map(|e| e.as_str()), Some("wallet_mismatch"));
    }

    #[test]
    fn missing_metadata_is_denied() {
        let guard = guard_for_module(BotModule::Sniper);
        let deny = guard.authorize(&request_with(None)).unwrap_err();
        assert_eq!(deny.as_str(), "missing_tenant_meta");
        let result = guard.veto_result("snipe-entry", &deny);
        assert!(result.error.as_deref().unwrap().contains(deny.as_str()));
        assert!(!result.succeeded());
    }

    #[test]
    fn matching_metadata_is_authorized() {
        let guard = guard_for_module(BotModule::Sniper);
        let meta = TenantTransactionMeta::from_context(
            guard.context().execution_context(),
            "sniper",
            Some("intent-1"),
        );
        assert!(guard.authorize(&request_with(Some(meta))).is_ok());
    }

    #[test]
    fn another_tenants_metadata_is_denied() {
        let guard = guard_for_module(BotModule::Sniper);
        let (other_org, runtime, gen) = parts();
        let foreign_ctx = issued(BotModule::Sniper, other_org, runtime, gen);
        let foreign_meta =
            TenantTransactionMeta::from_context(foreign_ctx.execution_context(), "sniper", None);
        let deny = guard
            .authorize(&request_with(Some(foreign_meta)))
            .unwrap_err();
        assert_eq!(deny.as_str(), "organization_mismatch");
        assert!(deny.detail().contains(&other_org.to_string()));
    }

    #[test]
    fn stale_generation_is_denied() {
        let guard = guard_for_module(BotModule::Sniper);
        let (org, runtime, gen) = (
            guard.organization_id(),
            guard.runtime_id(),
            guard.generation(),
        );
        let rotated_ctx = issued(BotModule::Sniper, org, runtime, gen.next().unwrap());
        let stale_meta =
            TenantTransactionMeta::from_context(rotated_ctx.execution_context(), "sniper", None);
        let deny = guard
            .authorize(&request_with(Some(stale_meta)))
            .unwrap_err();
        assert_eq!(deny.as_str(), "generation_mismatch");
    }

    #[test]
    fn cross_module_metadata_is_denied() {
        let guard = guard_for_module(BotModule::Sniper);
        let ctx = issued(
            BotModule::Copy,
            guard.organization_id(),
            guard.runtime_id(),
            guard.generation(),
        );
        let meta = TenantTransactionMeta::from_context(ctx.execution_context(), "copy", None);
        let deny = guard.authorize(&request_with(Some(meta))).unwrap_err();
        assert_eq!(deny.as_str(), "module_mismatch");
    }

    #[test]
    fn foreign_runtime_is_denied() {
        let guard = guard_for_module(BotModule::Sniper);
        let ctx = issued(
            BotModule::Sniper,
            guard.organization_id(),
            RuntimeId::new(),
            guard.generation(),
        );
        let meta = TenantTransactionMeta::from_context(ctx.execution_context(), "sniper", None);
        let deny = guard.authorize(&request_with(Some(meta))).unwrap_err();
        assert_eq!(deny.as_str(), "runtime_mismatch");
    }

    #[test]
    fn guard_helper_builds_a_shared_guard() {
        // The bound wallet address is whatever the loaded wallet's pubkey
        // is — the helper must accept exactly that pairing.
        let wallet = loaded_wallet();
        let org = OrganizationId::new();
        let runtime = RuntimeId::new();
        let gen = RuntimeGeneration::first();
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            gen,
            BotModule::Sniper,
            ExecutionMode::Paper,
        )
        .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = chrono::Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet_ref = TenantWalletRef::new(org, wallet.pubkey.to_string()).unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "sniper-key")
                .unwrap();
        let ctx = TenantSigningContext::new(
            TenantExecutionContext::issue(
                org,
                runtime,
                gen,
                BotModule::Sniper,
                ExecutionMode::Paper,
                authority,
                wallet_ref,
                signer,
                ExecutionTrace::for_request(),
            )
            .unwrap(),
        );
        let shared = guard_for(ctx, &wallet).unwrap();
        assert_eq!(shared.organization_id(), org);
    }
}
