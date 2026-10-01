//! Tenant-bound transaction metadata (PROMPT 4/10 file 28).
//!
//! [`TenantTransactionMeta`] is the public, secret-free identity block a
//! tenant-scoped transaction carries from the moment it is requested to
//! the moment it is persisted: organization, runtime, fencing generation,
//! module and intent id. It is attached to a [`TxRequest`] via
//! [`TxRequest::tenant`] and travels with the built transaction so the
//! broadcast guard can verify it immediately before network submission.
//!
//! [`TenantTransaction`] is the same metadata wrapped around a completed
//! execution result — the value a tenant executor hands to the
//! tenant-scoped persistence layer. Its `Debug`/`Display` render public
//! data only; there is no field anywhere in this module that can hold a
//! key, seed or credential.
//!
//! The metadata is deliberately NOT part of [`TxRequest::intent_digest`]:
//! the digest pins the transaction CONTENT (wallet, label, instructions)
//! so the same logical retry keeps its intent id across attempts. Tenant
//! identity constrains WHO may submit, not WHAT the transaction is.

use bot_core::execution::TenantExecutionContext;
use bot_core::tenant::{ModuleKind, OrganizationId, RuntimeGeneration, RuntimeId};
use serde::{Deserialize, Serialize};

use crate::execute::ExecutionResult;

/// Public tenant identity for one transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantTransactionMeta {
    /// The acting tenant.
    organization_id: OrganizationId,
    /// The executing runtime.
    runtime_id: RuntimeId,
    /// The fencing generation the transaction was authorized under.
    generation: RuntimeGeneration,
    /// The module submitting the transaction.
    module: ModuleKind,
    /// Producing module label as recorded on the wire (e.g. `sniper`).
    module_label: String,
    /// The execution-intent id, when one was pinned.
    intent_id: Option<String>,
    /// The correlation trace id.
    trace_id: String,
}

impl TenantTransactionMeta {
    /// Build the metadata from an issued core execution context.
    ///
    /// The context is the ONLY accepted source of the identity fields —
    /// callers cannot assemble a meta from loose parts (which would allow
    /// a caller-chosen organization id).
    pub fn from_context(
        context: &TenantExecutionContext,
        module_label: &str,
        intent_id: Option<&str>,
    ) -> Self {
        TenantTransactionMeta {
            organization_id: context.organization_id(),
            runtime_id: context.runtime_id(),
            generation: context.generation(),
            module: context.scope().module(),
            module_label: module_label.to_string(),
            intent_id: intent_id.map(|s| s.to_string()),
            trace_id: context.trace().trace_id().to_string(),
        }
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The executing runtime.
    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    /// The fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.generation
    }

    /// The module the transaction belongs to.
    pub fn module(&self) -> ModuleKind {
        self.module
    }

    /// The producing module label.
    pub fn module_label(&self) -> &str {
        &self.module_label
    }

    /// The pinned intent id, when present.
    pub fn intent_id(&self) -> Option<&str> {
        self.intent_id.as_deref()
    }

    /// The correlation trace id.
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }
}

/// A tenant-bound execution outcome: the existing [`ExecutionResult`]
/// together with the tenant identity that produced it.
///
/// This is what the tenant executors persist through the tenant-scoped
/// trading repositories — the repository layer receives the organization
/// from HERE (and re-checks it against its own write scope), never from a
/// client-supplied parameter.
#[derive(Debug, Clone)]
pub struct TenantTransaction {
    meta: TenantTransactionMeta,
    label: String,
    intent_id: String,
    signature: Option<String>,
    paper: bool,
    succeeded: bool,
    error: Option<String>,
    /// The raw terminal status — sinks must distinguish CONFIRMED
    /// (landing proven) from SENT (broadcast, landing unproven).
    status: crate::execute::ExecStatus,
}

impl TenantTransaction {
    /// Bind an execution result to its tenant metadata.
    pub fn new(meta: TenantTransactionMeta, result: &ExecutionResult) -> Self {
        TenantTransaction {
            meta,
            label: result.label.clone(),
            intent_id: result.intent_id.clone(),
            signature: result.broadcast_signature().map(|s| s.to_string()),
            paper: result.paper,
            succeeded: result.succeeded(),
            error: result.error.clone(),
            status: result.status,
        }
    }

    /// The tenant metadata.
    pub fn meta(&self) -> &TenantTransactionMeta {
        &self.meta
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.meta.organization_id
    }

    /// The transaction label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The execution-intent id.
    pub fn intent_id(&self) -> &str {
        &self.intent_id
    }

    /// The broadcast signature, when the transaction left the process (or
    /// the simulated signature in paper mode).
    pub fn signature(&self) -> Option<&str> {
        self.signature.as_deref()
    }

    /// Whether this was a paper execution.
    pub fn paper(&self) -> bool {
        self.paper
    }

    /// Whether the execution succeeded (including paper fills).
    pub fn succeeded(&self) -> bool {
        self.succeeded
    }

    /// The error message, when the execution failed.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The raw terminal status. Persistence layers MUST use this to
    /// distinguish a proven landing (`Confirmed`) from an unproven
    /// broadcast (`Sent`/`SendUnknown`) — `succeeded()` treats both as
    /// success for gate purposes.
    pub fn exec_status(&self) -> crate::execute::ExecStatus {
        self.status
    }
}

impl std::fmt::Display for TenantTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tenant {} module {} intent {} label {} sig {} paper {} ok {}",
            self.meta.organization_id,
            self.meta.module.as_str(),
            self.intent_id,
            self.label,
            self.signature.as_deref().unwrap_or("-"),
            self.paper,
            self.succeeded
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::models::{BotModule, ExecutionMode};
    use bot_core::tenant::{
        OrganizationId, RuntimeGeneration, RuntimeId, TenantSignerRef, TenantWalletRef,
    };
    use chrono::Utc;

    fn issued_context() -> TenantExecutionContext {
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
        let now = Utc::now();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        let wallet =
            TenantWalletRef::new(org, "9WxBLegADTxPyxrXPpWcs1kR9Yyq3ZBcxHtniQS0FzqM").unwrap();
        let signer =
            TenantSignerRef::new(org, bot_core::tenant::SignerProvider::Local, "sniper-key")
                .unwrap();
        TenantExecutionContext::issue(
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
        .unwrap()
    }

    #[test]
    fn meta_is_derived_only_from_an_issued_context() {
        let ctx = issued_context();
        let meta = TenantTransactionMeta::from_context(&ctx, "sniper", Some("intent-42"));
        assert_eq!(meta.organization_id(), ctx.organization_id());
        assert_eq!(meta.runtime_id(), ctx.runtime_id());
        assert_eq!(meta.generation(), ctx.generation());
        assert_eq!(meta.module(), ModuleKind::Sniper);
        assert_eq!(meta.module_label(), "sniper");
        assert_eq!(meta.intent_id(), Some("intent-42"));
        assert!(!meta.trace_id().is_empty());
    }

    #[test]
    fn meta_serializes_without_secrets() {
        let ctx = issued_context();
        let meta = TenantTransactionMeta::from_context(&ctx, "sniper", None);
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("organization_id"));
        assert!(!json.to_lowercase().contains("secret"));
        assert!(!json.to_lowercase().contains("private"));
        assert!(!json.to_lowercase().contains("seed"));
        let back: TenantTransactionMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn tenant_transaction_wraps_a_paper_result() {
        let ctx = issued_context();
        let meta = TenantTransactionMeta::from_context(&ctx, "sniper", Some("intent-7"));
        let result = ExecutionResult::empty("snipe-entry", "intent-7", true);
        let bound = TenantTransaction::new(meta, &result);
        assert_eq!(bound.organization_id(), ctx.organization_id());
        assert_eq!(bound.intent_id(), "intent-7");
        assert!(bound.paper());
        assert!(!bound.succeeded());
        assert!(bound
            .to_string()
            .contains(&ctx.organization_id().to_string()));
    }
}
