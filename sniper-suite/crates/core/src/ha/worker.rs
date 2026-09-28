//! Worker identity, registration, heartbeat and the deterministic worker
//! state machine (TASK 6 §1, §13).
//!
//! A *worker* is one running process of the suite. Its identity is the
//! process's `replica_id` (`[ha].replica_id`, else `{host}-{pid}-{rand}` —
//! [`crate::state::AppState::replica_id`]), so logs, metrics, execution
//! claims, ledger rows and the worker registry all name the same thing. A
//! restart is a NEW worker generation: the previous life's registration row
//! keeps its identity but gets a fresh `generation`, and the old life's
//! leases expire and are taken over (never reused silently).
//!
//! # State machine
//!
//! ```text
//!            register
//!   Starting ─────────► Recovering ──recovery ok──► Ready ──work──► Running
//!      │                    │                         ▲   │            │
//!      │                    │ recovery failed         │   └─ degraded ─┤
//!      │                    ▼                         │      (Degraded)│
//!      │            RecoveryRequired ◄────────────────┴────────────────┤
//!      │                    │                                          │
//!      │                    │ operator / retry                         │
//!      │                    └────────► Recovering                      │
//!      │                                                               │
//!      └──────────────── shutdown ──► Draining ──► Stopped ◄───────────┘
//!                                          ▲
//!                       lease lost ──► LeaseLost ──reacquired──► Recovering
//! ```
//!
//! Rules that make it deterministic:
//!
//! * `Stopped` is terminal for the process life — nothing leaves it.
//! * `LeaseLost` and `RecoveryRequired` are NOT ready: the worker keeps
//!   running (it must still drain and answer probes) but reports
//!   `ready = false`, so a load balancer / operator sees it immediately.
//! * `Degraded` is ready-but-impaired: the worker still owns its leases and
//!   may finish in-flight work, but a dependency it needs for NEW work is
//!   unhealthy.
//! * Every transition is explicit; an illegal transition is rejected with
//!   [`WorkerTransitionError`] and changes nothing.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

/// Deterministic worker states (TASK 6 §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    /// Process started, not registered yet.
    Starting,
    /// Registered; rebuilding durable state (ledger, risk, cursors, orders).
    Recovering,
    /// Recovery finished, dependencies healthy, leases held — may accept work.
    Ready,
    /// Actively processing work.
    Running,
    /// Ready but impaired: a dependency needed for NEW work is unhealthy.
    Degraded,
    /// Lost a lease it requires; must not mutate fenced state.
    LeaseLost,
    /// Recovery could not complete; manual attention required.
    RecoveryRequired,
    /// Shutting down: no new work, finishing in-flight, releasing leases.
    Draining,
    /// Terminal for this process life.
    Stopped,
}

impl WorkerState {
    /// Every state, stable order (metrics tables, docs, tests).
    pub const ALL: [WorkerState; 9] = [
        WorkerState::Starting,
        WorkerState::Recovering,
        WorkerState::Ready,
        WorkerState::Running,
        WorkerState::Degraded,
        WorkerState::LeaseLost,
        WorkerState::RecoveryRequired,
        WorkerState::Draining,
        WorkerState::Stopped,
    ];

    /// Stable lowercase label (metrics, journal, audit).
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkerState::Starting => "starting",
            WorkerState::Recovering => "recovering",
            WorkerState::Ready => "ready",
            WorkerState::Running => "running",
            WorkerState::Degraded => "degraded",
            WorkerState::LeaseLost => "lease_lost",
            WorkerState::RecoveryRequired => "recovery_required",
            WorkerState::Draining => "draining",
            WorkerState::Stopped => "stopped",
        }
    }

    /// Inverse of [`WorkerState::as_str`].
    pub fn parse(s: &str) -> Option<WorkerState> {
        WorkerState::ALL.iter().copied().find(|w| w.as_str() == s)
    }

    /// May this state accept NEW work?
    pub fn accepts_work(&self) -> bool {
        matches!(self, WorkerState::Ready | WorkerState::Running)
    }

    /// Does this state report READY on the readiness probe? `Degraded` is
    /// deliberately NOT ready: it still serves and drains, but it must not
    /// be handed new traffic.
    pub fn is_ready(&self) -> bool {
        matches!(self, WorkerState::Ready | WorkerState::Running)
    }

    /// Terminal for this process life.
    pub fn is_terminal(&self) -> bool {
        matches!(self, WorkerState::Stopped)
    }

    /// The legal state machine (see the module header). Deliberately strict:
    /// nothing leaves `Stopped`, and reaching `Ready` always goes through
    /// `Recovering` so durable state is never assumed.
    pub fn can_transition_to(&self, to: WorkerState) -> bool {
        use WorkerState::*;
        if *self == to {
            return false;
        }
        match (self, to) {
            (Stopped, _) => false,
            // Shutdown is reachable from every live state.
            (_, Draining) => true,
            (Draining, Stopped) => true,
            (_, Stopped) => false,
            // Startup path.
            (Starting, Recovering) => true,
            (Starting, RecoveryRequired) => true,
            (Recovering, Ready) => true,
            (Recovering, RecoveryRequired) => true,
            (Recovering, LeaseLost) => true,
            // Work / impairment.
            (Ready, Running) | (Running, Ready) => true,
            (Ready, Degraded) | (Running, Degraded) => true,
            (Degraded, Ready) | (Degraded, Running) => true,
            // Ownership loss from any serving state.
            (Ready, LeaseLost) | (Running, LeaseLost) | (Degraded, LeaseLost) => true,
            // Recovery demand from any serving state.
            (Ready, RecoveryRequired)
            | (Running, RecoveryRequired)
            | (Degraded, RecoveryRequired)
            | (LeaseLost, RecoveryRequired) => true,
            // Re-entry: a lost lease or a failed recovery re-runs recovery.
            (LeaseLost, Recovering) | (RecoveryRequired, Recovering) => true,
            _ => false,
        }
    }
}

impl std::fmt::Display for WorkerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An illegal state transition. Nothing changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerTransitionError {
    /// State the worker is in.
    pub from: WorkerState,
    /// State that was requested.
    pub to: WorkerState,
}

impl std::fmt::Display for WorkerTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "illegal worker transition {} -> {}",
            self.from.as_str(),
            self.to.as_str()
        )
    }
}

impl std::error::Error for WorkerTransitionError {}

/// How a worker participates in the cluster (TASK 6 §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HaMode {
    /// One worker owns everything; leases still apply (they make a restart
    /// safe), but no takeover is expected.
    Single,
    /// Several workers share the durable state; exactly one holds each
    /// singleton lease, the others stand by and take over on expiry.
    ActivePassive,
    /// Several workers process work concurrently; per-execution ownership
    /// (TASK 1–4 claims) keeps them from doing the same unit twice.
    ActiveActive,
}

impl HaMode {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            HaMode::Single => "single",
            HaMode::ActivePassive => "active_passive",
            HaMode::ActiveActive => "active_active",
        }
    }

    /// Inverse of [`HaMode::as_str`]; unknown input is `None`.
    pub fn parse(s: &str) -> Option<HaMode> {
        match s.trim() {
            "single" => Some(HaMode::Single),
            "active_passive" | "active-passive" => Some(HaMode::ActivePassive),
            "active_active" | "active-active" => Some(HaMode::ActiveActive),
            _ => None,
        }
    }

    /// True when more than one worker may run against the same state.
    pub fn is_clustered(&self) -> bool {
        matches!(self, HaMode::ActivePassive | HaMode::ActiveActive)
    }
}

/// One worker's durable registration row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerRegistration {
    /// Stable worker identity (the process `replica_id`).
    pub worker_id: String,
    /// Monotonic per-identity life counter: a restart registers the same
    /// `worker_id` with `generation + 1`. Work fenced by an older
    /// generation is deterministically rejected.
    pub generation: i64,
    /// HA mode the worker runs in.
    pub mode: HaMode,
    /// Current state.
    pub state: WorkerState,
    /// Host / container name (operator visibility; no secrets).
    pub host: String,
    /// Process id.
    pub pid: i64,
    /// Build version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// When this generation registered.
    pub started_at: DateTime<Utc>,
    /// Last heartbeat.
    pub last_seen_at: DateTime<Utc>,
    /// Short human detail of the current state.
    pub detail: String,
}

impl WorkerRegistration {
    /// A fresh registration in [`WorkerState::Starting`].
    pub fn new(
        worker_id: impl Into<String>,
        generation: i64,
        mode: HaMode,
        host: impl Into<String>,
        pid: i64,
        version: impl Into<String>,
        now: DateTime<Utc>,
    ) -> Self {
        WorkerRegistration {
            worker_id: worker_id.into(),
            generation,
            mode,
            state: WorkerState::Starting,
            host: host.into(),
            pid,
            version: version.into(),
            started_at: now,
            last_seen_at: now,
            detail: "registered".into(),
        }
    }

    /// Has this worker missed its heartbeat deadline at `now`?
    pub fn is_stale(&self, now: DateTime<Utc>, timeout: ChronoDuration) -> bool {
        now.signed_duration_since(self.last_seen_at) > timeout
    }

    /// Seconds since the last heartbeat (never negative).
    pub fn age_secs(&self, now: DateTime<Utc>) -> i64 {
        now.signed_duration_since(self.last_seen_at)
            .num_seconds()
            .max(0)
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "worker={} generation={} mode={} state={} host={} pid={} version={} last_seen={} detail={}",
            self.worker_id,
            self.generation,
            self.mode.as_str(),
            self.state,
            self.host,
            self.pid,
            self.version,
            self.last_seen_at.to_rfc3339(),
            self.detail
        )
    }
}

/// Classification of one worker seen in the registry (TASK 6 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerHealth {
    /// Heartbeat fresh.
    Live,
    /// Heartbeat older than the timeout — its leases may be taken over once
    /// they expire. Detection alone never takes anything over (§1: no
    /// worker may silently assume another's work).
    Stale,
    /// Registered as `Stopped` (clean shutdown).
    Stopped,
}

impl WorkerHealth {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkerHealth::Live => "live",
            WorkerHealth::Stale => "stale",
            WorkerHealth::Stopped => "stopped",
        }
    }

    /// Classify one registration.
    pub fn of(reg: &WorkerRegistration, now: DateTime<Utc>, timeout: ChronoDuration) -> Self {
        if reg.state == WorkerState::Stopped {
            WorkerHealth::Stopped
        } else if reg.is_stale(now, timeout) {
            WorkerHealth::Stale
        } else {
            WorkerHealth::Live
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(state: WorkerState, last_seen: DateTime<Utc>) -> WorkerRegistration {
        let mut r =
            WorkerRegistration::new("w1", 1, HaMode::Single, "host", 42, "0.1.0", last_seen);
        r.state = state;
        r.last_seen_at = last_seen;
        r
    }

    #[test]
    fn state_labels_round_trip() {
        for s in WorkerState::ALL {
            assert_eq!(WorkerState::parse(s.as_str()), Some(s));
        }
        assert_eq!(WorkerState::parse("nope"), None);
        assert_eq!(WorkerState::ALL.len(), 9);
    }

    #[test]
    fn readiness_and_work_acceptance_are_explicit() {
        assert!(WorkerState::Ready.is_ready());
        assert!(WorkerState::Running.is_ready());
        for s in [
            WorkerState::Starting,
            WorkerState::Recovering,
            WorkerState::Degraded,
            WorkerState::LeaseLost,
            WorkerState::RecoveryRequired,
            WorkerState::Draining,
            WorkerState::Stopped,
        ] {
            assert!(!s.is_ready(), "{s} must not report ready");
            if s != WorkerState::Degraded {
                assert!(!s.accepts_work(), "{s} must not accept work");
            }
        }
        assert!(!WorkerState::Degraded.accepts_work());
    }

    #[test]
    fn transition_matrix_is_deterministic() {
        use WorkerState::*;
        // Startup path.
        assert!(Starting.can_transition_to(Recovering));
        assert!(Recovering.can_transition_to(Ready));
        assert!(Ready.can_transition_to(Running));
        // Never skip recovery.
        assert!(!Starting.can_transition_to(Ready));
        assert!(!Starting.can_transition_to(Running));
        // Impairment and ownership loss.
        assert!(Running.can_transition_to(Degraded));
        assert!(Degraded.can_transition_to(Running));
        assert!(Running.can_transition_to(LeaseLost));
        assert!(LeaseLost.can_transition_to(Recovering));
        assert!(RecoveryRequired.can_transition_to(Recovering));
        assert!(!LeaseLost.can_transition_to(Ready), "must re-recover first");
        // Shutdown from every live state; nothing leaves Stopped.
        for s in WorkerState::ALL {
            if s == Stopped || s == Draining {
                continue;
            }
            assert!(s.can_transition_to(Draining), "{s} -> draining");
        }
        assert!(Draining.can_transition_to(Stopped));
        for s in WorkerState::ALL {
            assert!(!Stopped.can_transition_to(s), "nothing leaves stopped");
            assert!(!s.can_transition_to(s), "self-transition is not a change");
            if s != Draining {
                assert!(!s.can_transition_to(Stopped), "{s} must drain first");
            }
        }
    }

    #[test]
    fn staleness_is_a_pure_function_of_the_clock() {
        let t0 = Utc::now();
        let r = reg(WorkerState::Running, t0);
        let timeout = ChronoDuration::seconds(30);
        assert!(!r.is_stale(t0 + ChronoDuration::seconds(29), timeout));
        assert!(r.is_stale(t0 + ChronoDuration::seconds(31), timeout));
        assert_eq!(r.age_secs(t0 + ChronoDuration::seconds(31)), 31);
        assert_eq!(r.age_secs(t0 - ChronoDuration::seconds(5)), 0);
        assert_eq!(
            WorkerHealth::of(&r, t0 + ChronoDuration::seconds(10), timeout),
            WorkerHealth::Live
        );
        assert_eq!(
            WorkerHealth::of(&r, t0 + ChronoDuration::seconds(31), timeout),
            WorkerHealth::Stale
        );
        let stopped = reg(WorkerState::Stopped, t0 - ChronoDuration::hours(1));
        assert_eq!(
            WorkerHealth::of(&stopped, t0, timeout),
            WorkerHealth::Stopped
        );
    }

    #[test]
    fn ha_modes_round_trip() {
        for m in [HaMode::Single, HaMode::ActivePassive, HaMode::ActiveActive] {
            assert_eq!(HaMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(HaMode::parse("active-passive"), Some(HaMode::ActivePassive));
        assert_eq!(HaMode::parse("nope"), None);
        assert!(!HaMode::Single.is_clustered());
        assert!(HaMode::ActivePassive.is_clustered());
        assert!(HaMode::ActiveActive.is_clustered());
    }
}
