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
//! * same runtime AND fencing generation as the guard's captured context;
//!   this comparison alone does not fetch the current runtime fence;
//! * same module (a Copy order cannot ride a Sniper-issued context);
//! * same funding wallet (the executor's wallet is the bound wallet);
//! * the signer binding in the captured context is active (this is not a
//!   live revocation lookup).
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
    /// The runtime fence check refused: this runtime is no longer the tenant's
    /// current live runtime (rotated, superseded, drained, heartbeat lapsed), or
    /// the fence could not be read. Always fail-closed.
    RuntimeFenced {
        detail: String,
        source: &'static str,
    },
    /// The signer binding is not active according to a LIVE source. Distinct
    /// from [`TenantBroadcastDeny::SignerRevoked`], which is the captured flag:
    /// here the binding was revoked after the module was issued.
    SignerInactive {
        detail: String,
        source: &'static str,
    },
    /// The signer the executor would ACTUALLY sign with does not control the
    /// bound wallet. The captured context said the binding was active and the
    /// binding row may still be active, but the key behind it has changed
    /// (rotated under the same reference) — so the next order would be signed
    /// by a key the tenant never approved.
    SignerKeyMismatch {
        detail: String,
        source: &'static str,
    },
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
            TenantBroadcastDeny::RuntimeFenced { .. } => "runtime_fenced",
            TenantBroadcastDeny::SignerInactive { .. } => "signer_inactive",
            TenantBroadcastDeny::SignerKeyMismatch { .. } => "signer_key_mismatch",
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
            TenantBroadcastDeny::RuntimeFenced { detail, source } => {
                format!("{label}: runtime is not the current live runtime for this tenant ({source}: {detail})")
            }
            TenantBroadcastDeny::SignerInactive { detail, source } => {
                format!("{label}: signer binding is not active ({source}: {detail})")
            }
            TenantBroadcastDeny::SignerKeyMismatch { detail, source } => {
                format!("{label}: resolved signing key does not control the bound wallet ({source}: {detail})")
            }
        }
    }
}

impl std::fmt::Display for TenantBroadcastDeny {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail())
    }
}

impl std::error::Error for TenantBroadcastDeny {}

/// A live check of the tenant's CURRENT runtime fence.
///
/// The guard's captured context says what was true when the module was built.
/// A tenant runtime can be rotated or fenced afterwards, and a worker that was
/// already running must not execute one more order "because it was already
/// running". This trait is the seam the guard uses to ask the runtime registry
/// for the truth immediately before signing (implemented in the server crate by
/// `RuntimeRegistryFence`, which delegates to `RuntimeRegistryService::verify`).
///
/// `Err` is never a pass: an unverifiable fence refuses the broadcast.
#[async_trait::async_trait]
pub trait RuntimeFenceProbe: Send + Sync + std::fmt::Debug {
    async fn fence_is_current(
        &self,
        organization_id: OrganizationId,
        runtime_id: RuntimeId,
        generation: RuntimeGeneration,
    ) -> Result<(), String>;

    /// Stable name for logs and the veto detail.
    fn probe_name(&self) -> &'static str {
        "runtime_registry"
    }
}

/// A live check that the tenant's SIGNER BINDING is still active.
///
/// The captured context records whether the signer was active when the module
/// was issued. A binding can be revoked afterwards (offboarding, key
/// compromise, custody rotation) and a worker that was already running must not
/// keep signing with it. `authorize_meta` can only see the captured flag, so —
/// exactly like the runtime fence — the live answer comes through a seam that
/// the server implements over the durable binding registry.
///
/// `Err` is never a pass: an unverifiable binding refuses the broadcast.
#[async_trait::async_trait]
pub trait SignerBindingProbe: Send + Sync + std::fmt::Debug {
    /// `Ok(())` only when the binding is live and active.
    async fn signer_is_active(
        &self,
        organization_id: OrganizationId,
        provider: bot_core::tenant::SignerProvider,
        key_ref: &str,
    ) -> Result<(), String>;

    /// Stable name for logs and the veto detail.
    fn probe_name(&self) -> &'static str {
        "binding_registry"
    }
}

/// The live checks a guard can carry, attached together at construction.
///
/// `None` for either field means "that source was not wired" — the guard then
/// falls back to the captured context for that rule, which is documented
/// behaviour, not a bypass. An error from a wired probe is always a refusal.
#[derive(Debug, Clone, Default)]
pub struct LivenessProbes {
    /// Live runtime-registry check (rotation / drain / lease lapse).
    pub fence: Option<Arc<dyn RuntimeFenceProbe>>,
    /// Live signer-binding check (revocation after issuance).
    pub signer: Option<Arc<dyn SignerBindingProbe>>,
}

impl LivenessProbes {
    /// Attach a runtime fence probe.
    #[must_use]
    pub fn with_fence(mut self, probe: Arc<dyn RuntimeFenceProbe>) -> Self {
        self.fence = Some(probe);
        self
    }

    /// Attach a signer binding probe.
    #[must_use]
    pub fn with_signer(mut self, probe: Arc<dyn SignerBindingProbe>) -> Self {
        self.signer = Some(probe);
        self
    }

    /// Whether neither source is wired.
    pub fn is_empty(&self) -> bool {
        self.fence.is_none() && self.signer.is_none()
    }
}

/// The final tenant gate in front of signing and broadcast.
#[derive(Debug, Clone)]
pub struct TenantBroadcastGuard {
    context: TenantSigningContext,
    funding_wallet: Pubkey,
    fence_probe: Option<Arc<dyn RuntimeFenceProbe>>,
    signer_probe: Option<Arc<dyn SignerBindingProbe>>,
    /// The signer key resolved when the guard was built (in-process registry).
    /// Present only when the caller supplied a registry, i.e. when the executor
    /// can actually sign through it.
    expected_signer_key: Option<Pubkey>,
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
            fence_probe: None,
            signer_probe: None,
            expected_signer_key: None,
        })
    }

    /// Attach a live fence probe. Without one the guard only compares against
    /// its captured context (documented behaviour, still fail-closed) — with one
    /// every broadcast also asks the registry whether this runtime is still the
    /// tenant's current live runtime at the generation it claims.
    #[must_use]
    pub fn with_fence_probe(mut self, probe: Arc<dyn RuntimeFenceProbe>) -> Self {
        self.fence_probe = Some(probe);
        self
    }

    /// Whether this guard re-checks the runtime fence against a live registry.
    pub fn has_fence_probe(&self) -> bool {
        self.fence_probe.is_some()
    }

    /// Attach a live signer-binding probe. Without one the guard trusts the
    /// captured `is_active()` flag on the signer binding (documented
    /// behaviour); with one a revoked binding refuses the broadcast even
    /// though the captured flag still says active.
    #[must_use]
    pub fn with_signer_probe(mut self, probe: Arc<dyn SignerBindingProbe>) -> Self {
        self.signer_probe = Some(probe);
        self
    }

    /// Whether this guard re-checks the signer binding against a live source.
    pub fn has_signer_probe(&self) -> bool {
        self.signer_probe.is_some()
    }

    /// Fix the signer key this guard will accept (see
    /// [`Self::authorize_signer_identity`]). Called by the executors with the
    /// key their signer registry resolves for this context.
    #[must_use]
    pub fn with_expected_signer_key(mut self, key: Pubkey) -> Self {
        self.expected_signer_key = Some(key);
        self
    }

    /// Whether this guard pins the resolved signer key.
    pub fn has_signer_key_binding(&self) -> bool {
        self.expected_signer_key.is_some()
    }

    /// Attach every supplied live source at once (what the executors call).
    #[must_use]
    pub fn with_liveness(mut self, probes: LivenessProbes) -> Self {
        self.fence_probe = probes.fence;
        self.signer_probe = probes.signer;
        self
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

    /// Verify the signer the executor resolved actually controls the bound
    /// wallet, and that it is the SAME signer identity the guard captured.
    ///
    /// This is the check that catches a key rotated under an unchanged
    /// reference: the binding row can stay "active", the captured context can
    /// stay valid, and the executor can still resolve a DIFFERENT key for the
    /// same identity — at which point the next order would be signed by a key
    /// nobody approved for this tenant.
    ///
    /// `resolved` is the key the executor's signer registry returns for this
    /// context's signer identity; `None` means the identity is not registered at
    /// all (a signing attempt would fail later — refuse now, deterministically).
    pub fn authorize_signer_identity(
        &self,
        resolved: Option<Pubkey>,
    ) -> Result<(), TenantBroadcastDeny> {
        let Some(expected) = self.expected_signer_key else {
            // No registry was supplied: the executor signs with its funding
            // wallet, whose binding was verified at construction.
            return Ok(());
        };
        let Some(resolved) = resolved else {
            return Err(TenantBroadcastDeny::SignerKeyMismatch {
                detail: format!(
                    "signer identity {} is not registered",
                    self.context.signer_identity()
                ),
                source: "signer_registry",
            });
        };
        if resolved != expected {
            return Err(TenantBroadcastDeny::SignerKeyMismatch {
                detail: format!(
                    "signer identity {} resolves to {resolved}, expected {expected}",
                    self.context.signer_identity()
                ),
                source: "signer_registry",
            });
        }
        if resolved.to_string() != self.context.wallet_address() {
            return Err(TenantBroadcastDeny::SignerKeyMismatch {
                detail: format!(
                    "resolved signer {resolved} does not control the bound wallet {}",
                    self.context.wallet_address()
                ),
                source: "signer_registry",
            });
        }
        Ok(())
    }

    /// The COMPLETE gate: the captured-context checks in
    /// [`Self::authorize_meta`] PLUS, when a probe is attached, a live check
    /// that this runtime is still the tenant's current live runtime at this
    /// generation. This is what the executor runs at broadcast time.
    ///
    /// Order matters: a metadata mismatch is reported as such (it is a wiring
    /// bug) before any I/O; then the live fence is consulted.
    pub async fn authorize_live(
        &self,
        meta: &TenantTransactionMeta,
    ) -> Result<(), TenantBroadcastDeny> {
        self.authorize_meta(meta)?;
        self.authorize_signer_identity(self.expected_signer_key)?;
        if let Some(probe) = &self.signer_probe {
            match probe
                .signer_is_active(
                    self.context.organization_id(),
                    self.context.signer_provider(),
                    self.context.signer_identity(),
                )
                .await
            {
                Ok(()) => {}
                Err(detail) => {
                    return Err(TenantBroadcastDeny::SignerInactive {
                        detail,
                        source: probe.probe_name(),
                    })
                }
            }
        }
        let Some(probe) = &self.fence_probe else {
            return Ok(());
        };
        match probe
            .fence_is_current(
                self.context.organization_id(),
                self.context.runtime_id(),
                self.context.generation(),
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(detail) => Err(TenantBroadcastDeny::RuntimeFenced {
                detail,
                source: probe.probe_name(),
            }),
        }
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

    /// Authorize a [`TxRequest`] including the live fence (executor broadcast
    /// path). Equivalent to `authorize` on the metadata, plus the probe.
    pub async fn authorize_tx(&self, req: &TxRequest) -> Result<(), TenantBroadcastDeny> {
        let meta = req
            .tenant
            .as_ref()
            .ok_or(TenantBroadcastDeny::MissingTenantMeta)?;
        self.authorize_live(meta).await
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

    /// Prebuilt transaction including the live fence. A transaction built
    /// before a rotation is precisely what the live check must catch.
    pub async fn authorize_prebuilt_live(
        &self,
        built: &BuiltTx,
    ) -> Result<(), TenantBroadcastDeny> {
        let meta = built
            .tenant
            .as_ref()
            .ok_or(TenantBroadcastDeny::MissingTenantMeta)?;
        self.authorize_live(meta).await
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

    /// A probe that reports whatever the test tells it to.
    #[derive(Debug)]
    struct ScriptedProbe {
        answer: std::sync::Mutex<Result<(), String>>,
        calls: std::sync::Mutex<usize>,
    }

    impl ScriptedProbe {
        fn new(answer: Result<(), String>) -> Arc<Self> {
            Arc::new(ScriptedProbe {
                answer: std::sync::Mutex::new(answer),
                calls: std::sync::Mutex::new(0),
            })
        }
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
        fn set(&self, answer: Result<(), String>) {
            *self.answer.lock().unwrap() = answer;
        }
    }

    #[async_trait::async_trait]
    impl RuntimeFenceProbe for ScriptedProbe {
        async fn fence_is_current(
            &self,
            _organization_id: OrganizationId,
            _runtime_id: RuntimeId,
            _generation: RuntimeGeneration,
        ) -> Result<(), String> {
            *self.calls.lock().unwrap() += 1;
            self.answer.lock().unwrap().clone()
        }
        fn probe_name(&self) -> &'static str {
            "scripted"
        }
    }

    /// Gap: "tenant runtime freshness at broadcast".
    ///
    /// `authorize_meta` compares metadata to the CAPTURED context. When the
    /// tenant's runtime is rotated or fenced after the module was built, the
    /// captured context still matches its own metadata, so the old guard
    /// authorized the broadcast — a worker from before a rotation could execute
    /// one more order "because it was already running". With a fence probe
    /// attached the same call now consults the live registry and refuses.
    #[tokio::test]
    async fn a_rotated_runtime_is_refused_at_broadcast_time() {
        let (org, runtime, generation) = parts();
        let address = bound_address();
        let ctx = issued_for_wallet(BotModule::Sniper, org, runtime, generation, &address);
        let wallet: Pubkey = Pubkey::from_str(&address).expect("wallet");
        let meta = TenantTransactionMeta::from_context(ctx.execution_context(), "sniper", None);

        // Baseline: no probe → captured-context rules only.
        let guard = TenantBroadcastGuard::new(ctx.clone(), wallet).expect("guard");
        assert!(!guard.has_fence_probe());
        assert!(guard.authorize_meta(&meta).is_ok());
        assert!(guard.authorize_live(&meta).await.is_ok());

        // The runtime is still current: allowed, and the probe really ran.
        let probe = ScriptedProbe::new(Ok(()));
        let guard = TenantBroadcastGuard::new(ctx.clone(), wallet)
            .expect("guard")
            .with_fence_probe(probe.clone());
        assert!(guard.has_fence_probe());
        assert!(guard.authorize_live(&meta).await.is_ok());
        assert_eq!(probe.calls(), 1, "the live fence must be consulted");

        // Rotated: the registry says this runtime is no longer current.
        probe.set(Err("stale_generation: claimed 1, current 2".into()));
        let deny = guard
            .authorize_live(&meta)
            .await
            .expect_err("a rotated runtime must not broadcast");
        assert_eq!(deny.as_str(), "runtime_fenced");
        assert!(deny.detail().contains("stale_generation"), "{deny}");
        // …and the same call is what the metadata-only check cannot see.
        assert!(
            guard.authorize_meta(&meta).is_ok(),
            "the captured context cannot detect the rotation by itself"
        );

        // An unreadable fence is a refusal, never a pass.
        probe.set(Err("registry unavailable".into()));
        assert!(guard.authorize_live(&meta).await.is_err());

        // A metadata mismatch is still reported as such, before any probe I/O.
        let (other_org, other_runtime, other_gen) = parts();
        let other = issued_for_wallet(
            BotModule::Sniper,
            other_org,
            other_runtime,
            other_gen,
            &bound_address(),
        );
        let other_meta =
            TenantTransactionMeta::from_context(other.execution_context(), "sniper", None);
        let before = probe.calls();
        let deny = guard
            .authorize_live(&other_meta)
            .await
            .expect_err("another tenant is denied");
        assert_eq!(deny.as_str(), "organization_mismatch");
        assert_eq!(probe.calls(), before, "no fence I/O for a wiring mismatch");
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

    /// A scripted signer-binding source, with the same call counting so a test
    /// can prove the live source is really consulted.
    struct ScriptedSignerProbe {
        answer: std::sync::Mutex<Result<(), String>>,
        calls: std::sync::Mutex<usize>,
        seen: std::sync::Mutex<Option<(bot_core::tenant::SignerProvider, String)>>,
    }

    impl std::fmt::Debug for ScriptedSignerProbe {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("ScriptedSignerProbe")
        }
    }

    impl ScriptedSignerProbe {
        fn new(answer: Result<(), String>) -> Arc<Self> {
            Arc::new(ScriptedSignerProbe {
                answer: std::sync::Mutex::new(answer),
                calls: std::sync::Mutex::new(0),
                seen: std::sync::Mutex::new(None),
            })
        }
        fn calls(&self) -> usize {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait::async_trait]
    impl SignerBindingProbe for ScriptedSignerProbe {
        async fn signer_is_active(
            &self,
            _organization_id: OrganizationId,
            provider: bot_core::tenant::SignerProvider,
            key_ref: &str,
        ) -> Result<(), String> {
            *self.calls.lock().unwrap() += 1;
            *self.seen.lock().unwrap() = Some((provider, key_ref.to_string()));
            self.answer.lock().unwrap().clone()
        }
        fn probe_name(&self) -> &'static str {
            "scripted_bindings"
        }
    }

    /// Gap: signer revocation at broadcast.
    ///
    /// The captured `is_active()` flag on the signer binding is what
    /// `authorize_meta` sees. A binding revoked AFTER the module was issued keeps
    /// that flag true forever, so the old guard would keep signing. With a live
    /// signer probe the revocation is refused — with its own label, so an
    /// operator can tell a revocation apart from a runtime rotation.
    #[tokio::test]
    async fn a_revoked_binding_is_refused_at_broadcast_time() {
        let (org, runtime, generation) = parts();
        let address = bound_address();
        let ctx = issued_for_wallet(BotModule::Copy, org, runtime, generation, &address);
        let wallet: Pubkey = Pubkey::from_str(&address).expect("wallet");
        let meta = TenantTransactionMeta::from_context(ctx.execution_context(), "copy", None);

        // Baseline: the captured binding says active, so the metadata check passes.
        let guard = TenantBroadcastGuard::new(ctx.clone(), wallet).expect("guard");
        assert!(!guard.has_signer_probe());
        assert!(guard.authorize_meta(&meta).is_ok());
        assert!(guard.authorize_live(&meta).await.is_ok());

        // Live binding still active: allowed, and the LIVE source was consulted
        // with this tenant's own provider + key reference.
        let probe = ScriptedSignerProbe::new(Ok(()));
        let guard = TenantBroadcastGuard::new(ctx.clone(), wallet)
            .expect("guard")
            .with_signer_probe(probe.clone());
        assert!(guard.has_signer_probe());
        assert!(guard.authorize_live(&meta).await.is_ok());
        assert_eq!(
            probe.calls(),
            1,
            "the live signer binding must be consulted"
        );
        let (provider, key_ref) = probe.seen.lock().unwrap().clone().expect("seen");
        assert_eq!(key_ref, ctx.signer_identity());
        assert_eq!(provider, ctx.signer_provider());

        // Revoked after issuance: refused, with the signer label (not "fenced").
        *probe.answer.lock().unwrap() = Err("binding revoked".into());
        let deny = guard
            .authorize_live(&meta)
            .await
            .expect_err("a revoked binding must not broadcast");
        assert_eq!(deny.as_str(), "signer_inactive");
        assert_eq!(
            deny.detail(),
            "signer_inactive: signer binding is not active (scripted_bindings: binding revoked)"
        );
        // The captured flag still says active — that is exactly the blindness.
        assert!(
            guard.authorize_meta(&meta).is_ok(),
            "the captured is_active() flag cannot see a later revocation"
        );

        // An unreadable binding source refuses too.
        *probe.answer.lock().unwrap() = Err("registry unavailable".into());
        assert!(guard.authorize_live(&meta).await.is_err());

        // A revoked signer is reported BEFORE the runtime fence is consulted:
        // no pointless registry I/O about a runtime this order may not use.
        let runtime_probe = ScriptedProbe::new(Ok(()));
        let guard = TenantBroadcastGuard::new(ctx, wallet)
            .expect("guard")
            .with_signer_probe(ScriptedSignerProbe::new(Err("revoked".into())))
            .with_fence_probe(runtime_probe.clone());
        let deny = guard.authorize_live(&meta).await.expect_err("revoked");
        assert_eq!(deny.as_str(), "signer_inactive");
        assert_eq!(
            runtime_probe.calls(),
            0,
            "a revoked signer must not trigger a runtime fence lookup"
        );
    }

    /// Gap: a signing key rotated under an UNCHANGED reference.
    ///
    /// The binding row can stay active, the captured context can stay valid, and
    /// yet the registry the executor will sign through can resolve a different
    /// key — that order would be signed by a key the tenant never approved. The
    /// guard pins the resolved key at construction and re-checks it at broadcast.
    #[tokio::test]
    async fn a_key_rotated_under_the_same_reference_is_refused() {
        let (org, runtime, generation) = parts();
        let address = bound_address();
        let ctx = issued_for_wallet(BotModule::Sniper, org, runtime, generation, &address);
        let wallet: Pubkey = Pubkey::from_str(&address).expect("wallet");
        let meta = TenantTransactionMeta::from_context(ctx.execution_context(), "sniper", None);

        // No registry supplied → no key pinning; the funding wallet signs.
        let guard = TenantBroadcastGuard::new(ctx.clone(), wallet).expect("guard");
        assert!(!guard.has_signer_key_binding());
        assert!(guard.authorize_signer_identity(None).is_ok());

        // Registry supplied: the approved key is pinned.
        let approved: Pubkey = Pubkey::from_str(&address).expect("wallet");
        let guard = TenantBroadcastGuard::new(ctx.clone(), wallet)
            .expect("guard")
            .with_expected_signer_key(approved);
        assert!(guard.has_signer_key_binding());
        assert!(guard.authorize_signer_identity(Some(approved)).is_ok());
        assert!(guard.authorize_live(&meta).await.is_ok());

        // The identity vanishes from the registry: refused deterministically
        // rather than failing later inside signing.
        let deny = guard
            .authorize_signer_identity(None)
            .expect_err("an unregistered signer must not sign");
        assert_eq!(deny.as_str(), "signer_key_mismatch");
        assert!(deny.detail().contains("not registered"), "{deny}");

        // A DIFFERENT key resolves under the same identity: refused, and the
        // detail names both keys so an operator can see the rotation.
        let rotated = Pubkey::new_unique();
        let deny = guard
            .authorize_signer_identity(Some(rotated))
            .expect_err("a rotated key must not sign");
        assert_eq!(deny.as_str(), "signer_key_mismatch");
        assert!(deny.detail().contains("resolves to"), "{deny}");
        assert!(deny.detail().contains(&rotated.to_string()), "{deny}");

        // A key that does not control the bound wallet is refused too, even when
        // it matches the pin (a mis-wired registry must not slip through).
        let stranger = Pubkey::new_unique();
        let mismatched = TenantBroadcastGuard::new(ctx, wallet)
            .expect("guard")
            .with_expected_signer_key(stranger);
        let deny = mismatched
            .authorize_signer_identity(Some(stranger))
            .expect_err("a key that is not the bound wallet must not sign");
        assert_eq!(deny.as_str(), "signer_key_mismatch");
        assert!(
            deny.detail().contains("does not control the bound wallet"),
            "{deny}"
        );
    }
}
