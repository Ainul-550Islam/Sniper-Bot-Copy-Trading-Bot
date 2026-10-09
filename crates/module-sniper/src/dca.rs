//! Scheduled DCA with budget caps (GAP-MAP v2, P2).
//!
//! Dollar-cost-averaging into a mint: buy a fixed SOL amount on a fixed
//! interval until a TOTAL budget is spent, then stop. The module owns the
//! schedule model and the budget accounting; execution re-enters the normal
//! entry pipeline, and the scheduler hook lives with the server's
//! `tenant_background` job loop (same split as `limit_orders`).
//!
//! Money-safety rules encoded here:
//! * the budget cap is checked BEFORE a run is released — a schedule can
//!   never spend more than `budget_sol` in total, even across restarts
//!   (spend is persisted through the store);
//! * a run is released at most once per interval: `next_run_at` advances by
//!   WHOLE intervals, so a missed window does not machine-gun a burst of
//!   catch-up buys (at most one run per pass);
//! * all accounting is checked/saturating — no wraps, no floats in the
//!   budget comparison beyond the f64 convention shared with the rest of
//!   the module's SOL amounts.

use std::collections::HashMap;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use bot_core::error::{BotError, BotResult};

/// Schedule lifecycle.
/// One lamport expressed in SOL. The smallest amount that can be sent.
const LAMPORT_SOL: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DcaStatus {
    /// Running on its interval until the budget is exhausted.
    Active,
    /// Budget fully spent — terminal.
    Completed,
    /// Paused by the tenant (runs are skipped, the clock keeps an honest
    /// `next_run_at` so resume does not fire a catch-up burst).
    Paused,
    /// Cancelled — terminal.
    Cancelled,
}

impl DcaStatus {
    /// True while the scheduler should consider the schedule at all.
    pub fn runnable(self) -> bool {
        matches!(self, DcaStatus::Active)
    }
}

/// One DCA schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DcaSchedule {
    pub id: String,
    /// Tenant scope (every query org-scoped; tenant_query gate applies).
    pub organization_id: String,
    /// The mint being accumulated.
    pub mint: String,
    /// SOL spent per run.
    pub amount_per_run_sol: f64,
    /// Seconds between runs.
    pub interval_secs: u64,
    /// TOTAL SOL budget across all runs (`0` = unlimited is NOT supported —
    /// an unlimited DCA is an operational accident waiting to happen; the
    /// validator rejects it).
    pub budget_sol: f64,
    /// SOL spent so far (persisted — survives restarts).
    pub spent_sol: f64,
    pub status: DcaStatus,
    pub created_at: DateTime<Utc>,
    /// When the next run is due.
    pub next_run_at: DateTime<Utc>,
}

impl DcaSchedule {
    /// Validate before storing. Fails closed.
    pub fn validate(&self) -> BotResult<()> {
        if self.mint.trim().is_empty() {
            return Err(BotError::invalid("dca: mint is required"));
        }
        if !self.amount_per_run_sol.is_finite() || self.amount_per_run_sol <= 0.0 {
            return Err(BotError::invalid("dca: amount_per_run must be positive"));
        }
        if self.interval_secs == 0 {
            return Err(BotError::invalid("dca: interval must be > 0"));
        }
        if !self.budget_sol.is_finite() || self.budget_sol <= 0.0 {
            return Err(BotError::invalid(
                "dca: budget must be positive (unlimited schedules are not supported)",
            ));
        }
        if self.budget_sol < self.amount_per_run_sol {
            return Err(BotError::invalid(
                "dca: budget cannot be smaller than one run",
            ));
        }
        Ok(())
    }

    /// SOL still available to spend. Never negative.
    pub fn remaining_budget_sol(&self) -> f64 {
        let remaining = self.budget_sol - self.spent_sol;
        // Below one lamport nothing can be sent; treat it as exhausted so
        // float dust (e.g. 0.9999999999999999 of a 1.0 budget) never creates
        // a phantom extra run.
        if remaining < LAMPORT_SOL {
            0.0
        } else {
            remaining
        }
    }

    /// True when nothing more can be spent.
    pub fn budget_exhausted(&self) -> bool {
        self.remaining_budget_sol() <= 0.0
    }

    /// The SOL amount for the NEXT run: the configured size, clamped to the
    /// remaining budget so the FINAL run tops the spend up to exactly the
    /// cap instead of overshooting it.
    pub fn next_run_amount_sol(&self) -> f64 {
        self.amount_per_run_sol.min(self.remaining_budget_sol())
    }

    /// Is a run due at `now`?
    pub fn due(&self, now: DateTime<Utc>) -> bool {
        self.status.runnable() && !self.budget_exhausted() && now >= self.next_run_at
    }
}

/// A run released by the scheduler.
#[derive(Debug, Clone, PartialEq)]
pub struct DcaRun {
    pub schedule_id: String,
    pub organization_id: String,
    pub mint: String,
    /// SOL to spend THIS run (already clamped to the remaining budget).
    pub amount_sol: f64,
    /// True when this run exhausts the budget (the store moves the schedule
    /// to `Completed` atomically with the spend).
    pub final_run: bool,
}

/// Persistence boundary (org-scoped implementations).
#[async_trait]
pub trait DcaStore: Send + Sync {
    async fn insert(&self, schedule: &DcaSchedule) -> BotResult<String>;
    /// All runnable/paused schedules for one tenant (terminal ones excluded).
    async fn list_open(&self, organization_id: &str) -> BotResult<Vec<DcaSchedule>>;
    /// Record one executed run: adds `amount_sol` to `spent_sol`, advances
    /// `next_run_at`, and — when the budget is now exhausted — flips the
    /// status to `Completed`.
    async fn record_run(&self, organization_id: &str, schedule_id: &str, amount_sol: f64, now: DateTime<Utc>) -> BotResult<()>;
    /// Set a terminal/paused status.
    async fn set_status(&self, organization_id: &str, schedule_id: &str, status: DcaStatus) -> BotResult<()>;
}

/// One scheduler pass for one tenant. Releases at most one run per due
/// schedule (no catch-up bursts) and returns them for execution.
pub async fn run_once<S: DcaStore + ?Sized>(
    store: &S,
    organization_id: &str,
    now: DateTime<Utc>,
) -> BotResult<Vec<DcaRun>> {
    let schedules = store.list_open(organization_id).await?;
    let mut runs = Vec::new();
    for schedule in schedules {
        if !schedule.due(now) {
            continue;
        }
        let amount = schedule.next_run_amount_sol();
        if amount <= 0.0 {
            continue;
        }
        let final_run = (schedule.spent_sol + amount) >= schedule.budget_sol - LAMPORT_SOL;
        store
            .record_run(organization_id, &schedule.id, amount, now)
            .await?;
        runs.push(DcaRun {
            schedule_id: schedule.id.clone(),
            organization_id: organization_id.to_string(),
            mint: schedule.mint.clone(),
            amount_sol: amount,
            final_run,
        });
    }
    Ok(runs)
}

// ---------------------------------------------------------------------------
// In-memory store (tests + operator-mode local runs)
// ---------------------------------------------------------------------------

/// Thread-safe in-memory [`DcaStore`].
#[derive(Default)]
pub struct InMemoryDcaStore {
    inner: Mutex<HashMap<String, Vec<DcaSchedule>>>,
}

#[async_trait]
impl DcaStore for InMemoryDcaStore {
    async fn insert(&self, schedule: &DcaSchedule) -> BotResult<String> {
        schedule.validate()?;
        let mut inner = self.inner.lock().await;
        let schedules = inner.entry(schedule.organization_id.clone()).or_default();
        let id = if schedule.id.is_empty() {
            format!("dca-{}-{}", schedules.len() + 1, schedule.mint)
        } else {
            if schedules.iter().any(|s| s.id == schedule.id) {
                return Err(BotError::invalid(format!("dca id {} already exists", schedule.id)));
            }
            schedule.id.clone()
        };
        let mut stored = schedule.clone();
        stored.id = id.clone();
        schedules.push(stored);
        Ok(id)
    }

    async fn list_open(&self, organization_id: &str) -> BotResult<Vec<DcaSchedule>> {
        let inner = self.inner.lock().await;
        Ok(inner
            .get(organization_id)
            .map(|schedules| {
                schedules
                    .iter()
                    .filter(|s| !matches!(s.status, DcaStatus::Completed | DcaStatus::Cancelled))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn record_run(&self, organization_id: &str, schedule_id: &str, amount_sol: f64, now: DateTime<Utc>) -> BotResult<()> {
        let mut inner = self.inner.lock().await;
        let schedules = inner
            .get_mut(organization_id)
            .ok_or_else(|| BotError::invalid("dca: unknown organization"))?;
        let schedule = schedules
            .iter_mut()
            .find(|s| s.id == schedule_id)
            .ok_or_else(|| BotError::invalid(format!("dca {schedule_id} not found")))?;
        // Budget is a HARD ceiling: a buggy caller cannot overspend it.
        let allowed = amount_sol.min(schedule.remaining_budget_sol());
        if allowed <= 0.0 {
            return Err(BotError::invalid(
                "dca: run refused — budget already exhausted",
            ));
        }
        schedule.spent_sol += allowed;
        // Advance by WHOLE intervals past `now` (no catch-up bursts).
        let interval = Duration::seconds(schedule.interval_secs.max(1) as i64);
        let mut next = schedule.next_run_at + interval;
        while next <= now {
            next += interval;
        }
        schedule.next_run_at = next;
        if schedule.budget_exhausted() {
            schedule.status = DcaStatus::Completed;
        }
        Ok(())
    }

    async fn set_status(&self, organization_id: &str, schedule_id: &str, status: DcaStatus) -> BotResult<()> {
        let mut inner = self.inner.lock().await;
        let schedules = inner
            .get_mut(organization_id)
            .ok_or_else(|| BotError::invalid("dca: unknown organization"))?;
        let schedule = schedules
            .iter_mut()
            .find(|s| s.id == schedule_id)
            .ok_or_else(|| BotError::invalid(format!("dca {schedule_id} not found")))?;
        schedule.status = status;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule(amount: f64, interval: u64, budget: f64, next_run: DateTime<Utc>) -> DcaSchedule {
        DcaSchedule {
            id: String::new(),
            organization_id: "org-1".into(),
            mint: "MintX".into(),
            amount_per_run_sol: amount,
            interval_secs: interval,
            budget_sol: budget,
            spent_sol: 0.0,
            status: DcaStatus::Active,
            created_at: Utc::now(),
            next_run_at: next_run,
        }
    }

    fn t(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn validation_rejects_unsafe_schedules() {
        assert!(schedule(0.1, 60, 1.0, t(0)).validate().is_ok());
        assert!(schedule(0.1, 0, 1.0, t(0)).validate().is_err(), "zero interval");
        assert!(schedule(0.1, 60, 0.0, t(0)).validate().is_err(), "zero budget");
        assert!(schedule(2.0, 60, 1.0, t(0)).validate().is_err(), "budget < one run");
        assert!(schedule(f64::NAN, 60, 1.0, t(0)).validate().is_err());
    }

    #[test]
    fn budget_math_never_goes_negative() {
        let mut s = schedule(0.3, 60, 1.0, t(0));
        s.spent_sol = 0.9;
        assert!((s.remaining_budget_sol() - 0.1).abs() < 1e-12);
        assert!((s.next_run_amount_sol() - 0.1).abs() < 1e-12, "final run clamps");
        s.spent_sol = 1.5; // overspend recorded by a bug elsewhere
        assert_eq!(s.remaining_budget_sol(), 0.0, "never negative");
        assert!(s.budget_exhausted());
    }

    #[test]
    fn due_requires_active_status_and_budget_and_time() {
        let mut s = schedule(0.1, 60, 1.0, t(100));
        assert!(!s.due(t(99)));
        assert!(s.due(t(100)));
        s.status = DcaStatus::Paused;
        assert!(!s.due(t(200)), "paused schedules are never due");
        s.status = DcaStatus::Active;
        s.spent_sol = 1.0;
        assert!(!s.due(t(200)), "exhausted budget is never due");
    }

    #[tokio::test]
    async fn runs_respect_interval_and_clamp_the_final_run() {
        let store = InMemoryDcaStore::default();
        let id = store.insert(&schedule(0.3, 60, 1.0, t(100))).await.unwrap();

        // Not due before next_run_at.
        assert!(run_once(&store, "org-1", t(99)).await.unwrap().is_empty());

        // Run 1 at t=100.
        let runs = run_once(&store, "org-1", t(100)).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert!((runs[0].amount_sol - 0.3).abs() < 1e-12);
        assert!(!runs[0].final_run);

        // Immediately after: next_run_at advanced, nothing due.
        assert!(run_once(&store, "org-1", t(130)).await.unwrap().is_empty());

        // Budget 1.0 at 0.3/run: spend goes 0.3 (t=100), 0.6 (t=160),
        // 0.9 (t=220). The fourth run (t=280) has only 0.1 left, so it
        // clamps to the remainder and exhausts the budget.
        assert_eq!(run_once(&store, "org-1", t(160)).await.unwrap().len(), 1);
        let third = run_once(&store, "org-1", t(220)).await.unwrap();
        assert_eq!(third.len(), 1);
        assert!((third[0].amount_sol - 0.3).abs() < 1e-12);
        assert!(!third[0].final_run);
        let last = run_once(&store, "org-1", t(280)).await.unwrap();
        assert_eq!(last.len(), 1);
        assert!((last[0].amount_sol - 0.1).abs() < 1e-9, "final run clamps to remaining budget");
        assert!(last[0].final_run);
        // Schedule completed: no further runs, ever.
        assert!(run_once(&store, "org-1", t(1_000)).await.unwrap().is_empty());
        assert!(store.list_open("org-1").await.unwrap().iter().all(|s| s.id != id));
    }

    #[tokio::test]
    async fn missed_windows_do_not_burst() {
        let store = InMemoryDcaStore::default();
        store.insert(&schedule(0.1, 60, 10.0, t(100))).await.unwrap();
        // A full hour of missed windows -> exactly ONE run this pass.
        let runs = run_once(&store, "org-1", t(3_700)).await.unwrap();
        assert_eq!(runs.len(), 1, "at most one run per pass");
        // And the next run is one whole interval AFTER now, not in the past.
        let open = store.list_open("org-1").await.unwrap();
        assert_eq!(open.len(), 1);
        assert!(open[0].next_run_at > t(3_700));
    }

    #[tokio::test]
    async fn overspend_is_refused_by_the_store() {
        let store = InMemoryDcaStore::default();
        let id = store.insert(&schedule(0.5, 60, 1.0, t(100))).await.unwrap();
        store.record_run("org-1", &id, 0.5, t(100)).await.unwrap();
        store.record_run("org-1", &id, 0.5, t(160)).await.unwrap();
        // Budget now exactly exhausted -> further runs are an error.
        assert!(store.record_run("org-1", &id, 0.5, t(220)).await.is_err());
    }

    #[tokio::test]
    async fn paused_schedule_skips_and_resumes_cleanly() {
        let store = InMemoryDcaStore::default();
        let id = store.insert(&schedule(0.1, 60, 1.0, t(100))).await.unwrap();
        store.set_status("org-1", &id, DcaStatus::Paused).await.unwrap();
        assert!(run_once(&store, "org-1", t(500)).await.unwrap().is_empty());
        store.set_status("org-1", &id, DcaStatus::Active).await.unwrap();
        let runs = run_once(&store, "org-1", t(501)).await.unwrap();
        assert_eq!(runs.len(), 1, "resume fires the due run");
    }
}
