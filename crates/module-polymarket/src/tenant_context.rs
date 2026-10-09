//! Polymarket tenant context adapter (PROMPT 4/10 §D).
//!
//! [`PolyTenantContext`] adapts the ONE core
//! [`bot_core::execution::TenantExecutionContext`] into the
//! polymarket pipeline. It does NOT duplicate the tenant identity
//! types — every identity field delegates to the core context, which
//! itself can only exist through `TenantExecutionContext::issue` and
//! its fail-closed coherence checks (same discipline as the sniper
//! and copy adapters).
//!
//! What the adapter adds for polymarket specifically:
//!
//! * **module enforcement** — a Sniper- or Copy-issued context can
//!   never drive the Polymarket engine (the scope's module must be
//!   `Polymarket`);
//! * **the venue wallet binding** — Polymarket money moves through an
//!   EVM address (the order's maker/funder on Polygon), so the bound
//!   wallet for this module is that address and every signed order's
//!   maker must match it before anything is posted;
//! * **tenant-local dedup keys** — signal/intent dedup is scoped per
//!   organization + runtime: two tenants trading the same market
//!   deduplicate independently;
//! * **paper-default policy** — a tenant polymarket runs paper unless
//!   the context was issued for live mode.

use bot_core::execution::TenantExecutionContext;
use bot_core::models::ExecutionMode;
use bot_core::tenant::{ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId};

/// Why a context cannot drive the polymarket engine. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolyContextError {
    /// The context was issued for a different module.
    WrongModule(ModuleKind),
    /// A re-verification against the live runtime identity failed
    /// (rotation/fence).
    StaleRuntime,
    /// The bound wallet is not a usable EVM address for this venue.
    InvalidVenueWallet,
}

impl PolyContextError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            PolyContextError::WrongModule(_) => "wrong_module",
            PolyContextError::StaleRuntime => "stale_runtime",
            PolyContextError::InvalidVenueWallet => "invalid_venue_wallet",
        }
    }
}

impl std::fmt::Display for PolyContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolyContextError::WrongModule(m) => {
                write!(f, "polymarket tenant context: module {m} is not polymarket")
            }
            PolyContextError::StaleRuntime => {
                write!(f, "polymarket tenant context: runtime identity is stale")
            }
            PolyContextError::InvalidVenueWallet => {
                write!(
                    f,
                    "polymarket tenant context: the bound wallet is not an EVM address"
                )
            }
        }
    }
}

impl std::error::Error for PolyContextError {}

/// The polymarket engine's view of a tenant execution context.
#[derive(Debug, Clone)]
pub struct PolyTenantContext {
    context: TenantExecutionContext,
}

impl PolyTenantContext {
    /// Adapt a core context. Fails when the context belongs to another
    /// module.
    pub fn adapt(context: TenantExecutionContext) -> Result<Self, PolyContextError> {
        if context.scope().module() != ModuleKind::Polymarket {
            return Err(PolyContextError::WrongModule(context.scope().module()));
        }
        Ok(PolyTenantContext { context })
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

    /// The execution mode this engine may run under.
    pub fn mode(&self) -> ExecutionMode {
        self.context.scope().mode()
    }

    /// Tenant polymarket defaults to paper: only a context explicitly
    /// issued for live mode authorizes live submissions.
    pub fn is_live(&self) -> bool {
        self.context.scope().is_live()
    }

    /// The bound venue wallet: for the polymarket module this is the
    /// EVM address whose funds the signed orders move (the maker /
    /// funder on Polygon). Public value only — never a key.
    pub fn wallet_address(&self) -> &str {
        self.context.wallet().address()
    }

    /// The bound signer identity (public reference).
    pub fn signer_identity(&self) -> &str {
        self.context.signer().key_ref()
    }

    /// The underlying core context.
    pub fn execution_context(&self) -> &TenantExecutionContext {
        &self.context
    }

    /// Re-verify against a freshly-resolved runtime identity.
    pub fn verify_against(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), PolyContextError> {
        self.context
            .verify_against(organization_id, runtime_id, generation)
            .map_err(|_| PolyContextError::StaleRuntime)
    }

    /// A tenant-local dedup key for an order intent. Two tenants (or
    /// one tenant across a runtime rotation) deduplicate the SAME
    /// market intent independently.
    pub fn dedup_key(&self, intent_id: &str) -> String {
        format!(
            "poly:{}:{}:{}",
            self.context.organization_id(),
            self.context.runtime_id(),
            intent_id
        )
    }

    /// Whether the bound venue wallet is a well-formed EVM address
    /// (`0x` + 20 bytes). Polymarket orders can only move the funds
    /// of the address bound here, so a malformed binding must refuse
    /// before anything is signed.
    pub fn validate_venue_wallet(&self) -> Result<(), PolyContextError> {
        let addr = self.wallet_address().trim();
        // "0x" is ASCII, so byte 2 is a char boundary once the prefix
        // check passes; the remaining 40 chars must be hex.
        let well_formed = addr.len() == 42
            && addr.starts_with("0x")
            && addr[2..].chars().all(|c| c.is_ascii_hexdigit());
        if !well_formed {
            return Err(PolyContextError::InvalidVenueWallet);
        }
        Ok(())
    }

    /// Whether a candidate maker/funder address is the bound venue
    /// wallet (case-insensitive, as EVM addresses are).
    pub fn wallet_is(&self, candidate: &str) -> bool {
        candidate
            .trim()
            .eq_ignore_ascii_case(self.wallet_address().trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::BotModule;
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };
    use chrono::Utc;

    fn issued(module: BotModule, mode: ExecutionMode, wallet: &str) -> TenantExecutionContext {
        let org = OrganizationId::new();
        // ONE runtime identity for both the authority checklist and the
        // issuance (same discipline as the sniper adapter's tests).
        let runtime = RuntimeId::new();
        let generation = RuntimeGeneration::first();
        let scope =
            bot_core::execution::ExecutionScope::new(org, runtime, generation, module, mode)
                .unwrap();
        let mut checklist = AuthorityChecklist::new();
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet = TenantWalletRef::new(org, wallet).unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "poly-key").unwrap();
        TenantExecutionContext::issue(
            org,
            runtime,
            generation,
            module,
            mode,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap()
    }

    const EVM_WALLET: &str = "0xE111180000d2663C0091e4f400237545B87B996B";

    #[test]
    fn a_polymarket_context_adapts() {
        let ctx = PolyTenantContext::adapt(issued(
            BotModule::Polymarket,
            ExecutionMode::Paper,
            EVM_WALLET,
        ))
        .unwrap();
        assert!(!ctx.is_live());
        assert_eq!(ctx.wallet_address(), EVM_WALLET);
        assert_eq!(ctx.signer_identity(), "poly-key");
        ctx.validate_venue_wallet().unwrap();
        assert!(ctx.wallet_is(EVM_WALLET));
        assert!(ctx.wallet_is(EVM_WALLET.to_lowercase().as_str()));
        assert!(!ctx.wallet_is("0x000000000000000000000000000000000000dEaD"));
    }

    #[test]
    fn a_sniper_or_copy_context_is_refused() {
        let err =
            PolyTenantContext::adapt(issued(BotModule::Sniper, ExecutionMode::Paper, EVM_WALLET))
                .unwrap_err();
        assert_eq!(err.as_str(), "wrong_module");
        let err =
            PolyTenantContext::adapt(issued(BotModule::Copy, ExecutionMode::Paper, EVM_WALLET))
                .unwrap_err();
        assert_eq!(err.as_str(), "wrong_module");
    }

    #[test]
    fn live_mode_is_explicit_not_assumed() {
        let paper = PolyTenantContext::adapt(issued(
            BotModule::Polymarket,
            ExecutionMode::Paper,
            EVM_WALLET,
        ))
        .unwrap();
        assert!(!paper.is_live());
        let live = PolyTenantContext::adapt(issued(
            BotModule::Polymarket,
            ExecutionMode::Live,
            EVM_WALLET,
        ))
        .unwrap();
        assert!(live.is_live());
    }

    #[test]
    fn a_non_evm_wallet_binding_is_refused_for_this_venue() {
        // A Solana-style base58 address is a perfectly good tenant
        // wallet — for the sniper. For polymarket it cannot be the
        // order maker, so the venue validation fails closed.
        let ctx = PolyTenantContext::adapt(issued(
            BotModule::Polymarket,
            ExecutionMode::Paper,
            "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM",
        ))
        .unwrap();
        assert_eq!(
            ctx.validate_venue_wallet().unwrap_err().as_str(),
            "invalid_venue_wallet"
        );
    }

    #[test]
    fn dedup_keys_are_tenant_and_runtime_scoped() {
        let a = PolyTenantContext::adapt(issued(
            BotModule::Polymarket,
            ExecutionMode::Paper,
            EVM_WALLET,
        ))
        .unwrap();
        let b = PolyTenantContext::adapt(issued(
            BotModule::Polymarket,
            ExecutionMode::Paper,
            EVM_WALLET,
        ))
        .unwrap();
        assert_ne!(a.dedup_key("intent-1"), b.dedup_key("intent-1"));
        assert!(a.dedup_key("intent-1").starts_with("poly:"));
        assert_eq!(a.dedup_key("intent-1"), a.dedup_key("intent-1"));
    }

    #[test]
    fn stale_runtime_fails_reverification() {
        let ctx = PolyTenantContext::adapt(issued(
            BotModule::Polymarket,
            ExecutionMode::Paper,
            EVM_WALLET,
        ))
        .unwrap();
        let rotated = ctx.generation().next().unwrap();
        assert_eq!(
            ctx.verify_against(ctx.organization_id(), ctx.runtime_id(), rotated)
                .unwrap_err()
                .as_str(),
            "stale_runtime"
        );
        assert!(ctx
            .verify_against(ctx.organization_id(), ctx.runtime_id(), ctx.generation())
            .is_ok());
    }
}
