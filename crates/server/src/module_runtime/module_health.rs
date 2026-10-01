//! Tenant module health (PROMPT 4/10 §A file 6).
//!
//! Classifies a tenant module instance's health from the facts the
//! runtime bridge observes:
//!
//! * the lifecycle phase;
//! * whether the fence still verifies against the live runtime record;
//! * whether the tenant broadcast guard attached successfully;
//! * how stale the module's last heartbeat/progress is.
//!
//! Classification is fail-closed: anything that would let a module
//! move money without a valid fence or guard is
//! [`HealthState::FailClosed`], and a fail-closed module NEVER permits
//! execution. [`HealthState::Degraded`] means the module may continue
//! (e.g. its heartbeat is stale but the fence is live) — the reason is
//! carried for observability, and the supervisor may still choose to
//! drain it.

use chrono::{DateTime, Utc};

use crate::module_runtime::module_lifecycle::ModulePhase;

/// The health verdict for one tenant module instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HealthState {
    /// Fence live, guard attached, progress fresh.
    Healthy,
    /// The module may continue, but something needs attention (the
    /// reason is carried). Degraded is NOT fail-closed: the fence and
    /// guard still hold.
    Degraded,
    /// The module must not execute. Either the fence is stale, the
    /// guard did not attach, or the phase is terminal/failed — exactly
    /// the states where money must not move.
    FailClosed,
}

impl HealthState {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthState::Healthy => "healthy",
            HealthState::Degraded => "degraded",
            HealthState::FailClosed => "fail_closed",
        }
    }

    /// May the module execute in this state? Fail-closed never may.
    pub fn permits_execution(self) -> bool {
        !matches!(self, HealthState::FailClosed)
    }
}

/// Why a module is not healthy. Closed vocabulary; every label is
/// machine-readable for the observability plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthReason {
    /// Everything verified.
    Ok,
    /// The runtime record no longer matches (rotated/foreign/expired).
    FenceStale,
    /// The runtime's heartbeat is older than the budget.
    HeartbeatStale,
    /// The module's own progress signal is stale (still within the
    /// fence, but the engine has not reported).
    ModuleProgressStale,
    /// The tenant broadcast guard did not attach — the engine can
    /// never be allowed to run like this.
    GuardNotAttached,
    /// The lifecycle phase is terminal or failed.
    PhaseTerminal,
    /// The phase does not permit new executions (draining).
    PhaseDraining,
}

impl HealthReason {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            HealthReason::Ok => "ok",
            HealthReason::FenceStale => "fence_stale",
            HealthReason::HeartbeatStale => "heartbeat_stale",
            HealthReason::ModuleProgressStale => "module_progress_stale",
            HealthReason::GuardNotAttached => "guard_not_attached",
            HealthReason::PhaseTerminal => "phase_terminal",
            HealthReason::PhaseDraining => "phase_draining",
        }
    }
}

/// The observed facts one health classification consumes.
#[derive(Debug, Clone, Copy)]
pub struct HealthObservation {
    /// The module's current lifecycle phase.
    pub phase: ModulePhase,
    /// Does the fence verify against the freshly-read runtime record?
    pub fence_ok: bool,
    /// Is the RUNTIME's heartbeat fresh?
    pub runtime_heartbeat_fresh: bool,
    /// Has the module itself reported progress within its budget?
    pub module_progress_fresh: bool,
    /// Did the tenant broadcast guard attach to the module's
    /// executors? `false` is an automatic fail-closed verdict.
    pub guard_attached: bool,
    /// When this observation was taken.
    pub observed_at: DateTime<Utc>,
}

impl HealthObservation {
    /// Observe a running module with all checks green.
    pub fn healthy_running(observed_at: DateTime<Utc>) -> Self {
        HealthObservation {
            phase: ModulePhase::Running,
            fence_ok: true,
            runtime_heartbeat_fresh: true,
            module_progress_fresh: true,
            guard_attached: true,
            observed_at,
        }
    }

    /// Classify the observation. The order of the checks IS the
    /// policy:
    ///
    /// 1. terminal/failed phase ⇒ FailClosed;
    /// 2. draining phase ⇒ Degraded (finish in-flight, nothing new);
    /// 3. guard not attached ⇒ FailClosed (money must never move
    ///    unguarded);
    /// 4. fence stale ⇒ FailClosed (rotated/foreign runtime);
    /// 5. runtime heartbeat stale ⇒ FailClosed (the runtime lost its
    ///    mandate);
    /// 6. module progress stale (fence still live) ⇒ Degraded;
    /// 7. otherwise Healthy.
    pub fn classify(&self) -> (HealthState, HealthReason) {
        if self.phase.is_terminal() {
            return (HealthState::FailClosed, HealthReason::PhaseTerminal);
        }
        if !self.guard_attached {
            return (HealthState::FailClosed, HealthReason::GuardNotAttached);
        }
        if !self.fence_ok {
            return (HealthState::FailClosed, HealthReason::FenceStale);
        }
        if !self.runtime_heartbeat_fresh {
            return (HealthState::FailClosed, HealthReason::HeartbeatStale);
        }
        if self.phase == ModulePhase::Draining {
            return (HealthState::Degraded, HealthReason::PhaseDraining);
        }
        if !self.module_progress_fresh {
            return (HealthState::Degraded, HealthReason::ModuleProgressStale);
        }
        (HealthState::Healthy, HealthReason::Ok)
    }
}

/// One recorded health verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleHealth {
    state: HealthState,
    reason: HealthReason,
    observed_at: DateTime<Utc>,
}

impl ModuleHealth {
    /// Classify an observation.
    pub fn from_observation(observation: &HealthObservation) -> Self {
        let (state, reason) = observation.classify();
        ModuleHealth {
            state,
            reason,
            observed_at: observation.observed_at,
        }
    }

    /// The verdict.
    pub fn state(&self) -> HealthState {
        self.state
    }

    /// Why the verdict was reached.
    pub fn reason(&self) -> HealthReason {
        self.reason
    }

    /// When the verdict was reached.
    pub fn observed_at(&self) -> DateTime<Utc> {
        self.observed_at
    }

    /// May the module execute under this verdict?
    pub fn permits_execution(&self) -> bool {
        self.state.permits_execution()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs() -> HealthObservation {
        HealthObservation::healthy_running(Utc::now())
    }

    #[test]
    fn a_fully_verified_running_module_is_healthy() {
        let health = ModuleHealth::from_observation(&obs());
        assert_eq!(health.state(), HealthState::Healthy);
        assert_eq!(health.reason(), HealthReason::Ok);
        assert!(health.permits_execution());
    }

    #[test]
    fn an_unguarded_module_is_fail_closed_no_matter_what_else_holds() {
        let mut o = obs();
        o.guard_attached = false;
        // Even running, fenced, fresh:
        let health = ModuleHealth::from_observation(&o);
        assert_eq!(health.state(), HealthState::FailClosed);
        assert_eq!(health.reason(), HealthReason::GuardNotAttached);
        assert!(!health.permits_execution());
    }

    #[test]
    fn a_stale_fence_or_runtime_heartbeat_is_fail_closed() {
        let mut o = obs();
        o.fence_ok = false;
        assert_eq!(
            ModuleHealth::from_observation(&o).reason(),
            HealthReason::FenceStale
        );
        let mut o = obs();
        o.runtime_heartbeat_fresh = false;
        let health = ModuleHealth::from_observation(&o);
        assert_eq!(health.state(), HealthState::FailClosed);
        assert_eq!(health.reason(), HealthReason::HeartbeatStale);
        assert!(!health.permits_execution());
    }

    #[test]
    fn terminal_and_draining_phases_classify_correctly() {
        let mut o = obs();
        o.phase = ModulePhase::Failed;
        let health = ModuleHealth::from_observation(&o);
        assert_eq!(health.state(), HealthState::FailClosed);
        assert_eq!(health.reason(), HealthReason::PhaseTerminal);

        let mut o = obs();
        o.phase = ModulePhase::Draining;
        let health = ModuleHealth::from_observation(&o);
        assert_eq!(health.state(), HealthState::Degraded);
        assert_eq!(health.reason(), HealthReason::PhaseDraining);
        assert!(health.permits_execution());
    }

    #[test]
    fn stale_module_progress_with_a_live_fence_is_only_degraded() {
        let mut o = obs();
        o.module_progress_fresh = false;
        let health = ModuleHealth::from_observation(&o);
        assert_eq!(health.state(), HealthState::Degraded);
        assert_eq!(health.reason(), HealthReason::ModuleProgressStale);
        // Degraded still executes — the fence and guard hold.
        assert!(health.permits_execution());
    }
}
