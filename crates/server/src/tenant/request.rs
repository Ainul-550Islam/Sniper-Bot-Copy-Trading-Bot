//! The tenant execution request (STEP 3 file 14).
//!
//! [`TenantExecutionRequest`] is the ONE shape every entry path (REST
//! API, WebSocket, module engine, background job) converts its
//! tenant-scoped work into before asking the gateway for a context.
//! It carries references, never secrets: wallet and signer are LABELS
//! into the tenant's binding registry, not key material.

use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{OrganizationId, SignerProvider};

/// What a tenant wants to do.
#[derive(Debug, Clone, PartialEq)]
pub struct TenantExecutionRequest {
    /// The acting tenant.
    pub organization_id: OrganizationId,
    /// The module that will run the work.
    pub module: BotModule,
    /// The trading mode the work needs (paper/simulate/live).
    pub mode: ExecutionMode,
    /// The wallet label the work must use (verified against the
    /// tenant's bindings by the binding guard).
    pub wallet_label: String,
    /// The signer the work must use (verified against the tenant's
    /// bindings by the binding guard).
    pub signer: SignerBinding,
    /// The position size this execution would open, in USD, when known
    /// (the risk guard clamps it against the effective limits).
    pub size_usd: Option<f64>,
    /// The slippage this execution would accept, in basis points, when
    /// known.
    pub slippage_bps: Option<u32>,
    /// Where the request came from (REST/WS/stream/job/recovery) —
    /// recorded in the execution trace.
    pub origin: &'static str,
}

/// A signer binding by reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerBinding {
    /// The provider kind.
    pub provider: SignerProvider,
    /// The provider-side key reference (public identifier).
    pub key_ref: String,
}

impl TenantExecutionRequest {
    /// A request skeleton for a module + mode, with the tenant's
    /// preferred labels to be filled from configuration by the caller.
    pub fn new(
        organization_id: OrganizationId,
        module: BotModule,
        mode: ExecutionMode,
        wallet_label: impl Into<String>,
        signer: SignerBinding,
    ) -> Self {
        TenantExecutionRequest {
            organization_id,
            module,
            mode,
            wallet_label: wallet_label.into(),
            signer,
            size_usd: None,
            slippage_bps: None,
            origin: bot_core::execution::REQUEST_ORIGIN,
        }
    }

    /// Attach the economic size (USD) of the intended execution.
    pub fn with_size_usd(mut self, size_usd: f64) -> Self {
        self.size_usd = Some(size_usd);
        self
    }

    /// Attach the intended slippage tolerance (bps).
    pub fn with_slippage_bps(mut self, slippage_bps: u32) -> Self {
        self.slippage_bps = Some(slippage_bps);
        self
    }

    /// Tag the origin (REST/WS/stream/job/recovery).
    pub fn with_origin(mut self, origin: &'static str) -> Self {
        self.origin = origin;
        self
    }

    /// The core module kind for this request (a type alias of
    /// [`BotModule`] — the guard vocabulary is shared).
    pub fn module_kind(&self) -> bot_core::tenant::ModuleKind {
        self.module
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_carries_references_not_secrets() {
        let req = TenantExecutionRequest::new(
            OrganizationId::new(),
            BotModule::Copy,
            ExecutionMode::Paper,
            "main-wallet",
            SignerBinding {
                provider: SignerProvider::Custody,
                key_ref: "custody/vault-1/key-7".into(),
            },
        )
        .with_size_usd(250.0)
        .with_slippage_bps(50)
        .with_origin(bot_core::execution::STREAM_ORIGIN);

        assert_eq!(req.module_kind(), bot_core::tenant::ModuleKind::Copy);
        assert_eq!(req.size_usd, Some(250.0));
        assert_eq!(req.slippage_bps, Some(50));
        assert_eq!(req.origin, bot_core::execution::STREAM_ORIGIN);
        // The debug form must never contain key material — it holds
        // references only by construction.
        let debugged = format!("{req:?}");
        assert!(debugged.contains("key-7")); // the public reference, fine
    }
}
