//! Tenant signing context (PROMPT 4/10 file 26).
//!
//! [`TenantSigningContext`] carries tenant/runtime/wallet/signer identity
//! from the core [`bot_core::execution::TenantExecutionContext`] into the
//! existing solana-kit signing layer. It is an ADAPTER, not a second tenant
//! identity model: the single source of truth stays the core execution
//! context (which can only be constructed through
//! [`bot_core::execution::TenantExecutionContext::issue`] with its
//! fail-closed coherence checks — authority fingerprint, wallet ownership,
//! signer ownership, signer liveness).
//!
//! What this adapter adds on top of the core context:
//!
//! * wallet binding against a concrete [`Wallet`] / [`Pubkey`] — the
//!   deployment wallet that will fund the transaction must be the wallet
//!   the context was issued for;
//! * signer resolution through the existing [`SignerRegistry`] — the
//!   registry identity must resolve, and its public key must match the
//!   bound wallet when the signer acts as the fee payer;
//! * request authorization for [`TxRequest`]s that carry
//!   [`crate::tenant_transaction::TenantTransactionMeta`].
//!
//! No secrets live here: the context holds public keys, typed ids and the
//! redacted signer reference only.

use bot_core::execution::TenantExecutionContext;
use bot_core::models::ExecutionMode;
use bot_core::tenant::{ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId};
use solana_sdk::pubkey::Pubkey;

use crate::signer::SignerRegistry;
use crate::tenant_transaction::TenantTransactionMeta;
use crate::tokens::Wallet;

/// Why a tenant signing authorization failed. Closed vocabulary; every
/// variant is a DENY — there is no fallback to a global/anonymous signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantSigningDeny {
    /// The wallet presented for signing belongs to another tenant (or is
    /// not the wallet the context was issued for).
    WalletMismatch,
    /// The signer identity bound to the context does not resolve in the
    /// registry (unknown/removed signer).
    SignerUnresolved,
    /// The resolved signer's public key does not match the bound wallet.
    SignerKeyMismatch,
    /// The signer binding is revoked (fail closed even if the registry
    /// still knows the key).
    SignerRevoked,
    /// The request carries tenant metadata for a different
    /// organization/runtime/generation/module.
    ContextMismatch,
    /// The request carries no tenant metadata at all.
    MissingTenantMeta,
}

impl TenantSigningDeny {
    /// Stable machine-readable label (logs, metrics, audit events).
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantSigningDeny::WalletMismatch => "wallet_mismatch",
            TenantSigningDeny::SignerUnresolved => "signer_unresolved",
            TenantSigningDeny::SignerKeyMismatch => "signer_key_mismatch",
            TenantSigningDeny::SignerRevoked => "signer_revoked",
            TenantSigningDeny::ContextMismatch => "context_mismatch",
            TenantSigningDeny::MissingTenantMeta => "missing_tenant_meta",
        }
    }
}

impl std::fmt::Display for TenantSigningDeny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = self.as_str();
        match self {
            TenantSigningDeny::WalletMismatch => {
                write!(
                    f,
                    "tenant signing denied: {label} (wallet not bound to this tenant)"
                )
            }
            TenantSigningDeny::SignerUnresolved => {
                write!(
                    f,
                    "tenant signing denied: {label} (signer identity not in registry)"
                )
            }
            TenantSigningDeny::SignerKeyMismatch => {
                write!(
                    f,
                    "tenant signing denied: {label} (signer key differs from bound wallet)"
                )
            }
            TenantSigningDeny::SignerRevoked => {
                write!(f, "tenant signing denied: {label} (signer binding revoked)")
            }
            TenantSigningDeny::ContextMismatch => write!(
                f,
                "tenant signing denied: {label} (request metadata belongs to another context)"
            ),
            TenantSigningDeny::MissingTenantMeta => write!(
                f,
                "tenant signing denied: {label} (tenant-bound executor requires tenant metadata)"
            ),
        }
    }
}

impl std::error::Error for TenantSigningDeny {}

/// The tenant identity under which signing (and, through the broadcast
/// guard, network submission) is authorized.
///
/// Constructed from an already-issued core execution context; cloning is
/// cheap and the context is immutable.
#[derive(Debug, Clone)]
pub struct TenantSigningContext {
    execution: TenantExecutionContext,
}

impl TenantSigningContext {
    /// Adapt a core execution context. The context is used verbatim — no
    /// re-derivation of identity, no duplication of tenant types.
    pub fn new(execution: TenantExecutionContext) -> Self {
        TenantSigningContext { execution }
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.execution.organization_id()
    }

    /// The executing runtime.
    pub fn runtime_id(&self) -> RuntimeId {
        self.execution.runtime_id()
    }

    /// The fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.execution.generation()
    }

    /// The module this context authorizes (Sniper / Copy / …).
    pub fn module(&self) -> ModuleKind {
        self.execution.scope().module()
    }

    /// The execution mode (paper/live) the context was issued for.
    pub fn mode(&self) -> ExecutionMode {
        self.execution.scope().mode()
    }

    /// True when the context authorizes live money movement.
    pub fn is_live(&self) -> bool {
        self.execution.scope().is_live()
    }

    /// The bound public wallet address (base58, public data).
    pub fn wallet_address(&self) -> &str {
        self.execution.wallet().address()
    }

    /// The bound signer identity (registry key, public reference).
    pub fn signer_identity(&self) -> &str {
        self.execution.signer().key_ref()
    }

    /// The bound signer's provider kind.
    pub fn signer_provider(&self) -> bot_core::tenant::SignerProvider {
        self.execution.signer().provider()
    }

    /// The correlation trace id.
    pub fn trace_id(&self) -> String {
        self.execution.trace().trace_id().to_string()
    }

    /// The underlying core context (for callers that need to persist it
    /// alongside a tenant execution record).
    pub fn execution_context(&self) -> &TenantExecutionContext {
        &self.execution
    }

    /// Re-verify the context against a freshly-resolved runtime identity
    /// (defense in depth before a money step: the runtime may have been
    /// rotated or fenced since the context was issued).
    pub fn verify_against(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), TenantSigningDeny> {
        self.execution
            .verify_against(organization_id, runtime_id, generation)
            .map_err(|_| TenantSigningDeny::ContextMismatch)
    }

    /// Authorize the funding wallet: the concrete wallet handed to the
    /// executor must be the wallet this context was issued for.
    pub fn authorize_wallet(&self, wallet_pubkey: &Pubkey) -> Result<(), TenantSigningDeny> {
        if self.execution.wallet().address() == wallet_pubkey.to_string().as_str() {
            Ok(())
        } else {
            Err(TenantSigningDeny::WalletMismatch)
        }
    }

    /// Resolve the bound signer through the existing registry and verify
    /// its public key matches the bound wallet.
    ///
    /// This is the single place where a tenant-bound executor obtains its
    /// signer: never `registry.get(...)` with an unvalidated identity, and
    /// never the deployment-global primary signer by accident.
    pub fn authorize_signer(
        &self,
        registry: &SignerRegistry,
    ) -> Result<std::sync::Arc<dyn crate::signer::TransactionSigner>, TenantSigningDeny> {
        if !self.execution.signer().is_active() {
            return Err(TenantSigningDeny::SignerRevoked);
        }
        let signer = registry
            .get(self.signer_identity())
            .ok_or(TenantSigningDeny::SignerUnresolved)?;
        if let Some((_, pubkey)) = registry
            .public_keys()
            .into_iter()
            .find(|(identity, _)| *identity == self.signer_identity())
        {
            if pubkey.to_string() != self.execution.wallet().address() {
                return Err(TenantSigningDeny::SignerKeyMismatch);
            }
        }
        Ok(signer)
    }

    /// Authorize a transaction request: it must carry tenant metadata that
    /// matches this context exactly (organization, runtime, generation,
    /// module).
    pub fn authorize_request(&self, meta: &TenantTransactionMeta) -> Result<(), TenantSigningDeny> {
        if meta.organization_id() != self.organization_id()
            || meta.runtime_id() != self.runtime_id()
            || meta.generation() != self.generation()
            || meta.module() != self.module()
        {
            return Err(TenantSigningDeny::ContextMismatch);
        }
        Ok(())
    }

    /// Authorize a [`Wallet`] value directly (convenience wrapper over
    /// [`TenantSigningContext::authorize_wallet`]).
    pub fn authorize_funding_wallet(&self, wallet: &Wallet) -> Result<(), TenantSigningDeny> {
        self.authorize_wallet(&wallet.pubkey)
    }
}

#[cfg(test)]
mod tests {
    //! Context-authorization unit tests. The contexts are built through the
    //! REAL core issuance path (authority checklist over the full check
    //! order), so a denied authorization here means a denied authorization
    //! in production.

    use super::*;
    use std::str::FromStr;

    use bot_core::execution::{
        AuthorityChecklist, ExecutionTrace, TenantExecutionContext, AUTHORITY_CHECK_ORDER,
    };
    use bot_core::models::BotModule;
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };
    use chrono::Utc;

    fn bound_address() -> String {
        use solana_sdk::signer::Signer;
        solana_sdk::signature::Keypair::new().pubkey().to_string()
    }

    fn context(module: BotModule) -> TenantSigningContext {
        let org = OrganizationId::new();
        let runtime = RuntimeId::new();
        let gen = RuntimeGeneration::first();
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            gen,
            module,
            ExecutionMode::Paper,
        )
        .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet = TenantWalletRef::new(org, bound_address()).unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "sniper-key")
                .unwrap();
        TenantSigningContext::new(
            TenantExecutionContext::issue(
                org,
                runtime,
                gen,
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

    #[test]
    fn adapter_exposes_the_core_identity_without_duplication() {
        let ctx = context(BotModule::Sniper);
        assert_eq!(ctx.module(), ModuleKind::Sniper);
        // Base58 of a 32-byte pubkey is 43 OR 44 chars (a leading zero
        // byte shortens the encoding by one) — what must hold is that
        // the address is a real, round-tripping pubkey.
        let address = ctx.wallet_address();
        let decoded = Pubkey::from_str(address).expect("valid base58 pubkey");
        assert_eq!(decoded.to_string(), address, "address round-trips");
        assert!(
            address.len() == 43 || address.len() == 44,
            "a real base58 pubkey length, got {}",
            address.len()
        );
        assert_eq!(ctx.signer_identity(), "sniper-key");
        assert!(!ctx.is_live());
    }

    #[test]
    fn foreign_wallet_is_denied() {
        let ctx = context(BotModule::Sniper);
        let foreign = Pubkey::new_unique();
        assert_eq!(
            ctx.authorize_wallet(&foreign),
            Err(TenantSigningDeny::WalletMismatch)
        );
    }

    #[test]
    fn bound_wallet_is_authorized() {
        let ctx = context(BotModule::Sniper);
        let bound = Pubkey::from_str(ctx.wallet_address()).unwrap();
        assert!(ctx.authorize_wallet(&bound).is_ok());
    }

    #[test]
    fn rotated_runtime_fails_reverification() {
        let ctx = context(BotModule::Sniper);
        let rotated = ctx.generation().next().unwrap();
        assert_eq!(
            ctx.verify_against(ctx.organization_id(), ctx.runtime_id(), rotated),
            Err(TenantSigningDeny::ContextMismatch)
        );
        assert!(ctx
            .verify_against(ctx.organization_id(), ctx.runtime_id(), ctx.generation())
            .is_ok());
    }

    #[test]
    fn signer_must_resolve_in_the_registry() {
        let ctx = context(BotModule::Sniper);
        let empty = SignerRegistry::new();
        assert_eq!(
            ctx.authorize_signer(&empty).err().map(|e| e.as_str()),
            Some("signer_unresolved")
        );
    }

    #[test]
    fn request_metadata_must_match_the_context() {
        let ctx = context(BotModule::Sniper);
        let meta = TenantTransactionMeta::from_context(
            ctx.execution_context(),
            "sniper",
            Some("intent-1"),
        );
        assert!(ctx.authorize_request(&meta).is_ok());

        let other = context(BotModule::Copy);
        let foreign_meta =
            TenantTransactionMeta::from_context(other.execution_context(), "copy", None);
        assert_eq!(
            ctx.authorize_request(&foreign_meta),
            Err(TenantSigningDeny::ContextMismatch)
        );
    }
}
