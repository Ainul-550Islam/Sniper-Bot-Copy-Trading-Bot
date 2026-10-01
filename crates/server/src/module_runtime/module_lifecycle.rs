//! Tenant module lifecycle (PROMPT 4/10 §A file 5).
//!
//! The explicit state machine every tenant module instance follows:
//!
//! ```text
//!            start              started
//!   Idle ───────────▶ Starting ───────────▶ Running
//!                        │                     │   │
//!              fail/stop│              drain   │   │ fail/stop
//!                        ▼                     ▼   ▼
//!                     Stopped ◀──────────── Draining (→ Drained → Stopped)
//!                        │
//!             (terminal) │ fail from any phase → Failed (terminal)
//! ```
//!
//! Rules:
//!
//! * transitions are VALIDATED — an invalid event is refused, never
//!   ignored;
//! * a start is only authorized for an instance whose fence verifies
//!   against the LIVE runtime record (a rotated/stale runtime never
//!   starts a new engine);
//! * `Draining` finishes in-flight work but permits no new execution;
//! * `Stopped` and `Failed` are terminal — the only way forward is a
//!   NEW instance (new generation).
//!
//! The machine is pure: it decides transitions, it does not perform
//! them. The supervisor/task layer applies the verdicts.

use chrono::{DateTime, Utc};

use crate::module_runtime::tenant_module_instance::{InstanceIdentityError, TenantModuleInstance};
use crate::runtime_registry::model::TenantRuntimeRecord;

/// Lifecycle phase of one tenant module instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModulePhase {
    /// Registered, not yet starting.
    Idle,
    /// Engine construction in progress.
    Starting,
    /// Engine running under its fence.
    Running,
    /// Stopping: in-flight work completes, nothing new starts.
    Draining,
    /// Drain acknowledged; the engine task has exited.
    Drained,
    /// Stopped cleanly (or never started).
    Stopped,
    /// Terminal failure (construction error, guard attach failure,
    /// unrecoverable error). Fail-closed: never auto-restarted as the
    /// SAME instance.
    Failed,
}

impl ModulePhase {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ModulePhase::Idle => "idle",
            ModulePhase::Starting => "starting",
            ModulePhase::Running => "running",
            ModulePhase::Draining => "draining",
            ModulePhase::Drained => "drained",
            ModulePhase::Stopped => "stopped",
            ModulePhase::Failed => "failed",
        }
    }

    /// Terminal phases never come back; a new engine needs a NEW
    /// instance (new generation).
    pub fn is_terminal(self) -> bool {
        matches!(self, ModulePhase::Stopped | ModulePhase::Failed)
    }

    /// May the engine start NEW work in this phase? `Running` only —
    /// draining completes what is in flight.
    pub fn permits_execution(self) -> bool {
        matches!(self, ModulePhase::Running)
    }
}

/// Why a lifecycle event was refused. Closed vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    /// The event is not valid from the current phase.
    InvalidTransition {
        from: &'static str,
        event: &'static str,
    },
    /// The instance is not enabled for the tenant.
    ModuleDisabled,
    /// The instance is not a runtime engine module.
    NotARuntimeModule,
    /// The fence check against the live record failed (carries the
    /// identity reason).
    Fence(InstanceIdentityError),
}

impl LifecycleError {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleError::InvalidTransition { .. } => "invalid_transition",
            LifecycleError::ModuleDisabled => "module_disabled",
            LifecycleError::NotARuntimeModule => "not_a_runtime_module",
            LifecycleError::Fence(_) => "fence_failed",
        }
    }
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LifecycleError::InvalidTransition { from, event } => {
                write!(f, "module lifecycle: {event} is not valid from {from}")
            }
            LifecycleError::ModuleDisabled => {
                write!(f, "module lifecycle: module is disabled for the tenant")
            }
            LifecycleError::NotARuntimeModule => {
                write!(f, "module lifecycle: not a runtime engine module")
            }
            LifecycleError::Fence(e) => write!(f, "module lifecycle: fence failed: {}", e.as_str()),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// A lifecycle event applied to a phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// Begin engine construction.
    Start,
    /// Construction finished; the engine runs.
    Started,
    /// Begin a graceful drain (no new executions).
    Drain,
    /// The drain completed; the engine task exited.
    Drained,
    /// Stop without draining (immediate shutdown).
    Stop,
    /// Terminal failure.
    Fail,
}

impl LifecycleEvent {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            LifecycleEvent::Start => "start",
            LifecycleEvent::Started => "started",
            LifecycleEvent::Drain => "drain",
            LifecycleEvent::Drained => "drained",
            LifecycleEvent::Stop => "stop",
            LifecycleEvent::Fail => "fail",
        }
    }
}

/// The pure transition table.
pub fn transition(
    phase: ModulePhase,
    event: LifecycleEvent,
) -> Result<ModulePhase, LifecycleError> {
    match (phase, event) {
        (ModulePhase::Idle, LifecycleEvent::Start) => Ok(ModulePhase::Starting),
        (ModulePhase::Starting, LifecycleEvent::Started) => Ok(ModulePhase::Running),
        (ModulePhase::Starting, LifecycleEvent::Fail) => Ok(ModulePhase::Failed),
        (ModulePhase::Starting, LifecycleEvent::Stop) => Ok(ModulePhase::Stopped),
        (ModulePhase::Running, LifecycleEvent::Drain) => Ok(ModulePhase::Draining),
        (ModulePhase::Running, LifecycleEvent::Stop) => Ok(ModulePhase::Stopped),
        (ModulePhase::Running, LifecycleEvent::Fail) => Ok(ModulePhase::Failed),
        (ModulePhase::Draining, LifecycleEvent::Drained) => Ok(ModulePhase::Drained),
        (ModulePhase::Draining, LifecycleEvent::Fail) => Ok(ModulePhase::Failed),
        (ModulePhase::Drained, LifecycleEvent::Stop) => Ok(ModulePhase::Stopped),
        (ModulePhase::Drained, LifecycleEvent::Fail) => Ok(ModulePhase::Failed),
        // Terminal phases accept nothing.
        (ModulePhase::Stopped, _) | (ModulePhase::Failed, _) => {
            Err(LifecycleError::InvalidTransition {
                from: phase.as_str(),
                event: event.as_str(),
            })
        }
        // Everything else is invalid from this phase.
        _ => Err(LifecycleError::InvalidTransition {
            from: phase.as_str(),
            event: event.as_str(),
        }),
    }
}

/// Authorize a START for an instance: enabled, a runtime module, and
/// fenced against the LIVE record. Engines are never started for a
/// stale, rotated or foreign runtime.
pub fn authorize_start(
    instance: &TenantModuleInstance,
    record: &TenantRuntimeRecord,
    now: DateTime<Utc>,
) -> Result<(), LifecycleError> {
    if !instance.enabled() {
        return Err(LifecycleError::ModuleDisabled);
    }
    if !instance.is_runtime_module() {
        return Err(LifecycleError::NotARuntimeModule);
    }
    instance
        .verify_record(record, now)
        .map_err(LifecycleError::Fence)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::execution::TenantExecutionContext;

    #[test]
    fn the_happy_path_is_idle_start_started_drain_drained_stop() {
        let mut phase = ModulePhase::Idle;
        for (event, want) in [
            (LifecycleEvent::Start, ModulePhase::Starting),
            (LifecycleEvent::Started, ModulePhase::Running),
            (LifecycleEvent::Drain, ModulePhase::Draining),
            (LifecycleEvent::Drained, ModulePhase::Drained),
            (LifecycleEvent::Stop, ModulePhase::Stopped),
        ] {
            phase = transition(phase, event).expect("valid transition");
            assert_eq!(phase, want);
        }
        assert!(phase.is_terminal());
    }

    #[test]
    fn running_permits_execution_and_everything_else_does_not() {
        assert!(ModulePhase::Running.permits_execution());
        for phase in [
            ModulePhase::Idle,
            ModulePhase::Starting,
            ModulePhase::Draining,
            ModulePhase::Drained,
            ModulePhase::Stopped,
            ModulePhase::Failed,
        ] {
            assert!(
                !phase.permits_execution(),
                "{} must not permit execution",
                phase.as_str()
            );
        }
    }

    #[test]
    fn terminal_phases_accept_nothing() {
        for phase in [ModulePhase::Stopped, ModulePhase::Failed] {
            for event in [
                LifecycleEvent::Start,
                LifecycleEvent::Started,
                LifecycleEvent::Drain,
                LifecycleEvent::Drained,
                LifecycleEvent::Stop,
                LifecycleEvent::Fail,
            ] {
                let err = transition(phase, event).unwrap_err();
                assert_eq!(err.as_str(), "invalid_transition");
            }
        }
    }

    #[test]
    fn invalid_transitions_are_refused_not_ignored() {
        // Cannot "start" twice, cannot drain before running, cannot
        // report started from idle.
        assert_eq!(
            transition(ModulePhase::Starting, LifecycleEvent::Start)
                .unwrap_err()
                .as_str(),
            "invalid_transition"
        );
        assert_eq!(
            transition(ModulePhase::Idle, LifecycleEvent::Drain)
                .unwrap_err()
                .as_str(),
            "invalid_transition"
        );
        assert_eq!(
            transition(ModulePhase::Idle, LifecycleEvent::Started)
                .unwrap_err()
                .as_str(),
            "invalid_transition"
        );
        // A running engine may also stop or fail directly.
        assert_eq!(
            transition(ModulePhase::Running, LifecycleEvent::Stop).unwrap(),
            ModulePhase::Stopped
        );
        assert_eq!(
            transition(ModulePhase::Running, LifecycleEvent::Fail).unwrap(),
            ModulePhase::Failed
        );
    }

    #[test]
    fn a_start_requires_an_enabled_runtime_module_with_a_live_fence() {
        use crate::module_runtime::tenant_module_instance::TenantModuleInstance;
        use bot_core::execution::{AuthorityChecklist, ExecutionTrace, AUTHORITY_CHECK_ORDER};
        use bot_core::models::{BotModule, ExecutionMode};
        use bot_core::tenant::{
            OrganizationId, RuntimeGeneration, RuntimeId, SignerProvider, TenantSignerRef,
            TenantWalletRef,
        };
        let now = Utc::now();
        let org = OrganizationId::new();
        let runtime = RuntimeId::new();
        let generation = RuntimeGeneration::first();
        let scope = bot_core::execution::ExecutionScope::new(
            org,
            runtime,
            generation,
            BotModule::Sniper,
            ExecutionMode::Paper,
        )
        .unwrap();
        let mut checklist = AuthorityChecklist::new();
        for name in AUTHORITY_CHECK_ORDER {
            checklist.record(name, now).unwrap();
        }
        let authority = checklist.finish(&scope, now).unwrap();
        use solana_sdk::signer::Signer;
        let address = solana_sdk::signature::Keypair::new().pubkey().to_string();
        let wallet = TenantWalletRef::new(org, &address).unwrap();
        let signer = TenantSignerRef::new(org, SignerProvider::Local, "k").unwrap();
        let context = TenantExecutionContext::issue(
            org,
            runtime,
            generation,
            BotModule::Sniper,
            ExecutionMode::Paper,
            authority,
            wallet,
            signer,
            ExecutionTrace::for_request(),
        )
        .unwrap();

        let record = TenantRuntimeRecord::new_active(org, runtime, generation, "w", now);
        let enabled = TenantModuleInstance::from_context(&context, true);
        assert!(authorize_start(&enabled, &record, now).is_ok());

        // Disabled: refused.
        let disabled = TenantModuleInstance::from_context(&context, false);
        assert_eq!(
            authorize_start(&disabled, &record, now)
                .unwrap_err()
                .as_str(),
            "module_disabled"
        );

        // Rotated record: fenced.
        let mut rotated = record.clone();
        rotated.generation = generation.next().unwrap();
        assert_eq!(
            authorize_start(&enabled, &rotated, now)
                .unwrap_err()
                .as_str(),
            "fence_failed"
        );
    }
}
