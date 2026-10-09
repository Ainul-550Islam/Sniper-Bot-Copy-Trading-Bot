//! Tenant module instance identity (PROMPT 4/10 §A file 1).
//!
//! [`TenantModuleInstance`] is the tenant-specific identity of ONE
//! running module: which organization, which runtime, which fencing
//! generation, which module, which mode, the wallet/signer bindings the
//! gateway resolved, and whether the module is enabled for the tenant.
//!
//! It does NOT duplicate the tenant identity types: every field is
//! derived from the issued [`TenantExecutionContext`] (the single
//! source of truth) plus the tenant's runtime record. It carries only
//! PUBLIC reference material — addresses and key references, never key
//! material.
//!
//! The instance is the unit the [`crate::module_runtime`] bridge
//! registers, fences and hands to the factory: an engine may only be
//! built for an instance whose runtime record is STILL live (heartbeat
//! fresh, lease live, status active) and whose identity matches the
//! issued context exactly.

use chrono::{DateTime, Utc};

use bot_core::execution::TenantExecutionContext;
use bot_core::models::{BotModule, ExecutionMode};
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId, SignerProvider};

use crate::runtime_registry::model::{RuntimeStatus, TenantRuntimeRecord};

/// Why an instance cannot be used against a runtime record or context.
/// Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceIdentityError {
    /// The record belongs to another tenant.
    OrganizationMismatch,
    /// The record is a different runtime instance.
    RuntimeMismatch,
    /// The record's generation is not the instance's generation.
    GenerationMismatch,
    /// The runtime record is not active (stopped/retired/…).
    RuntimeNotActive,
    /// The lease expired (fencing).
    LeaseExpired,
    /// The presented context does not match this instance.
    ContextMismatch,
    /// The module is not enabled for the tenant.
    ModuleDisabled,
    /// The module is not a runtime engine module (staking contract).
    NotARuntimeModule,
}

impl InstanceIdentityError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            InstanceIdentityError::OrganizationMismatch => "organization_mismatch",
            InstanceIdentityError::RuntimeMismatch => "runtime_mismatch",
            InstanceIdentityError::GenerationMismatch => "generation_mismatch",
            InstanceIdentityError::RuntimeNotActive => "runtime_not_active",
            InstanceIdentityError::LeaseExpired => "lease_expired",
            InstanceIdentityError::ContextMismatch => "context_mismatch",
            InstanceIdentityError::ModuleDisabled => "module_disabled",
            InstanceIdentityError::NotARuntimeModule => "not_a_runtime_module",
        }
    }
}

impl std::fmt::Display for InstanceIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tenant module instance: {}", self.as_str())
    }
}

impl std::error::Error for InstanceIdentityError {}

/// One tenant's one module instance identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantModuleInstance {
    organization_id: OrganizationId,
    runtime_id: RuntimeId,
    generation: RuntimeGeneration,
    module: BotModule,
    mode: ExecutionMode,
    /// PUBLIC wallet binding (address only — never key material).
    wallet_address: String,
    /// Display-only wallet label.
    wallet_label: Option<String>,
    /// PUBLIC signer binding (provider + key reference, never a key).
    signer_provider: SignerProvider,
    signer_key_ref: String,
    /// Whether the tenant's configuration enables this module.
    enabled: bool,
}

impl TenantModuleInstance {
    /// Derive the instance identity from an ISSUED execution context.
    ///
    /// The context is the single source of truth: organization, runtime,
    /// generation, module and mode come from its scope; the wallet and
    /// signer bindings are the PUBLIC references the gateway resolved.
    pub fn from_context(context: &TenantExecutionContext, enabled: bool) -> Self {
        TenantModuleInstance {
            organization_id: context.organization_id(),
            runtime_id: context.runtime_id(),
            generation: context.generation(),
            module: context.scope().module(),
            mode: context.scope().mode(),
            wallet_address: context.wallet().address().to_string(),
            wallet_label: context.wallet().label().map(|l| l.to_string()),
            signer_provider: context.signer().provider(),
            signer_key_ref: context.signer().key_ref().to_string(),
            enabled,
        }
    }

    /// The acting tenant.
    pub fn organization_id(&self) -> OrganizationId {
        self.organization_id
    }

    /// The executing runtime instance.
    pub fn runtime_id(&self) -> RuntimeId {
        self.runtime_id
    }

    /// The fencing generation.
    pub fn generation(&self) -> RuntimeGeneration {
        self.generation
    }

    /// The module this instance runs.
    pub fn module(&self) -> BotModule {
        self.module
    }

    /// The execution mode (paper/simulate/live).
    pub fn mode(&self) -> ExecutionMode {
        self.mode
    }

    /// The PUBLIC wallet binding address.
    pub fn wallet_address(&self) -> &str {
        &self.wallet_address
    }

    /// The display-only wallet label.
    pub fn wallet_label(&self) -> Option<&str> {
        self.wallet_label.as_deref()
    }

    /// The PUBLIC signer provider binding.
    pub fn signer_provider(&self) -> SignerProvider {
        self.signer_provider
    }

    /// The PUBLIC signer key reference (never a key).
    pub fn signer_key_ref(&self) -> &str {
        &self.signer_key_ref
    }

    /// Whether the tenant's configuration enables this module.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Is this module one the runtime bridge can run as an engine?
    /// Telegram is a control plane (no engine instance here) and the
    /// staking contract is an on-chain program, not a process module.
    pub fn is_runtime_module(&self) -> bool {
        matches!(
            self.module,
            BotModule::Sniper | BotModule::Copy | BotModule::Polymarket
        )
    }

    /// Full identity check against a freshly-read runtime record: the
    /// tenant, runtime and generation must match, the record must still
    /// be ACTIVE, and its lease (if any) must still be live. This is
    /// the fence every engine construction passes through.
    pub fn verify_record(
        &self,
        record: &TenantRuntimeRecord,
        now: DateTime<Utc>,
    ) -> Result<(), InstanceIdentityError> {
        if record.organization_id != self.organization_id {
            return Err(InstanceIdentityError::OrganizationMismatch);
        }
        if record.runtime_id != self.runtime_id {
            return Err(InstanceIdentityError::RuntimeMismatch);
        }
        if record.generation != self.generation {
            return Err(InstanceIdentityError::GenerationMismatch);
        }
        if record.status != RuntimeStatus::Active {
            return Err(InstanceIdentityError::RuntimeNotActive);
        }
        if !record.is_lease_live(now) {
            return Err(InstanceIdentityError::LeaseExpired);
        }
        Ok(())
    }

    /// Identity check against the issued context the engine will run
    /// under: every identity field must agree — a context of another
    /// tenant/runtime/generation/module never drives this instance.
    pub fn verify_context(
        &self,
        context: &TenantExecutionContext,
    ) -> Result<(), InstanceIdentityError> {
        if context.organization_id() != self.organization_id {
            return Err(InstanceIdentityError::OrganizationMismatch);
        }
        if context.runtime_id() != self.runtime_id {
            return Err(InstanceIdentityError::RuntimeMismatch);
        }
        if context.generation() != self.generation {
            return Err(InstanceIdentityError::GenerationMismatch);
        }
        if context.scope().module() != self.module {
            return Err(InstanceIdentityError::ContextMismatch);
        }
        if context.wallet().address() != self.wallet_address {
            return Err(InstanceIdentityError::ContextMismatch);
        }
        if context.signer().key_ref() != self.signer_key_ref {
            return Err(InstanceIdentityError::ContextMismatch);
        }
        Ok(())
    }

    /// Guard for engine construction: enabled, a runtime module, and
    /// fenced against the live record.
    pub fn authorize_engine(
        &self,
        record: &TenantRuntimeRecord,
        now: DateTime<Utc>,
    ) -> Result<(), InstanceIdentityError> {
        if !self.enabled {
            return Err(InstanceIdentityError::ModuleDisabled);
        }
        if !self.is_runtime_module() {
            return Err(InstanceIdentityError::NotARuntimeModule);
        }
        self.verify_record(record, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
    use bot_core::tenant::{TenantSignerRef, TenantWalletRef};

    fn issued(
        org: OrganizationId,
        runtime: RuntimeId,
        generation: RuntimeGeneration,
        module: BotModule,
        address: &str,
    ) -> TenantExecutionContext {
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            generation,
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
        let wallet = TenantWalletRef::new(org, address).unwrap();
        let signer = TenantSignerRef::new(org, SignerProvider::Local, "mod-key").unwrap();
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
        .unwrap()
    }

    fn live_record(instance: &TenantModuleInstance, now: DateTime<Utc>) -> TenantRuntimeRecord {
        TenantRuntimeRecord::new_active(
            instance.organization_id(),
            instance.runtime_id(),
            instance.generation(),
            "test-worker",
            now,
        )
    }

    fn keypair_address() -> String {
        use solana_sdk::signer::Signer;
        solana_sdk::signature::Keypair::new().pubkey().to_string()
    }

    #[test]
    fn an_instance_is_derived_from_and_verified_against_its_context() {
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let address = keypair_address();
        let context = issued(
            org,
            runtime,
            RuntimeGeneration::first(),
            BotModule::Sniper,
            &address,
        );
        let instance = TenantModuleInstance::from_context(&context, true);
        assert_eq!(instance.organization_id(), org);
        assert_eq!(instance.module(), BotModule::Sniper);
        assert_eq!(instance.wallet_address(), address);
        assert_eq!(instance.signer_key_ref(), "mod-key");
        assert!(instance.enabled());
        assert!(instance.verify_context(&context).is_ok());
    }

    #[test]
    fn a_foreign_context_never_matches_the_instance() {
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let address = keypair_address();
        let context = issued(
            org,
            runtime,
            RuntimeGeneration::first(),
            BotModule::Copy,
            &address,
        );
        let instance = TenantModuleInstance::from_context(&context, true);
        // Another tenant's context.
        let foreign = issued(
            OrganizationId::new(),
            runtime,
            RuntimeGeneration::first(),
            BotModule::Copy,
            &address,
        );
        assert_eq!(
            instance.verify_context(&foreign).unwrap_err().as_str(),
            "organization_mismatch"
        );
        // Same tenant, rotated generation.
        let stale = issued(
            org,
            runtime,
            RuntimeGeneration::first().next().unwrap(),
            BotModule::Copy,
            &address,
        );
        assert_eq!(
            instance.verify_context(&stale).unwrap_err().as_str(),
            "generation_mismatch"
        );
        // Same identity, different module.
        let other_module = issued(
            org,
            runtime,
            RuntimeGeneration::first(),
            BotModule::Sniper,
            &address,
        );
        assert_eq!(
            instance.verify_context(&other_module).unwrap_err().as_str(),
            "context_mismatch"
        );
    }

    #[test]
    fn the_fence_rejects_stale_rotated_and_foreign_records() {
        let now = Utc::now();
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let address = keypair_address();
        let context = issued(
            org,
            runtime,
            RuntimeGeneration::first(),
            BotModule::Sniper,
            &address,
        );
        let instance = TenantModuleInstance::from_context(&context, true);

        // Live, matching record: OK.
        assert!(instance
            .verify_record(&live_record(&instance, now), now)
            .is_ok());

        // Rotated generation: the record moved on, this instance is fenced out.
        let mut rotated = live_record(&instance, now);
        rotated.generation = RuntimeGeneration::first().next().unwrap();
        assert_eq!(
            instance.verify_record(&rotated, now).unwrap_err().as_str(),
            "generation_mismatch"
        );

        // Another runtime instance.
        let mut foreign = live_record(&instance, now);
        foreign.runtime_id = RuntimeId::new();
        assert_eq!(
            instance.verify_record(&foreign, now).unwrap_err().as_str(),
            "runtime_mismatch"
        );

        // Stopped record.
        let mut stopped = live_record(&instance, now);
        stopped.status = RuntimeStatus::Stopped;
        assert_eq!(
            instance.verify_record(&stopped, now).unwrap_err().as_str(),
            "runtime_not_active"
        );

        // Expired lease.
        let mut expired = live_record(&instance, now);
        expired.lease_expires_at = Some(now - chrono::Duration::seconds(1));
        assert_eq!(
            instance.verify_record(&expired, now).unwrap_err().as_str(),
            "lease_expired"
        );
    }

    #[test]
    fn engine_authorization_requires_enabled_runtime_module_and_live_fence() {
        let now = Utc::now();
        let (org, runtime) = (OrganizationId::new(), RuntimeId::new());
        let address = keypair_address();
        let context = issued(
            org,
            runtime,
            RuntimeGeneration::first(),
            BotModule::Sniper,
            &address,
        );
        // Disabled module: refused.
        let disabled = TenantModuleInstance::from_context(&context, false);
        assert_eq!(
            disabled
                .authorize_engine(&live_record(&disabled, now), now)
                .unwrap_err()
                .as_str(),
            "module_disabled"
        );
        // Enabled trading module with a live fence: allowed.
        let enabled = TenantModuleInstance::from_context(&context, true);
        assert!(enabled
            .authorize_engine(&live_record(&enabled, now), now)
            .is_ok());
        // Telegram is a control plane, not a runtime engine module.
        let telegram = issued(
            org,
            runtime,
            RuntimeGeneration::first(),
            BotModule::Telegram,
            &address,
        );
        let control = TenantModuleInstance::from_context(&telegram, true);
        assert_eq!(
            control
                .authorize_engine(&live_record(&control, now), now)
                .unwrap_err()
                .as_str(),
            "not_a_runtime_module"
        );
    }
}
