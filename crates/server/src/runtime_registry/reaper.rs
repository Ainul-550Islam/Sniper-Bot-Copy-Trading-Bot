//! The stale-runtime reaper (STEP 3 file 44).
//!
//! [`spawn_reaper`] runs the registry's recovery pass on a fixed
//! cadence: live runtimes whose heartbeat went stale are stopped, their
//! fence tokens die with them, and the next guard verification for the
//! tenant answers "no live runtime" until a new runtime registers.
//! The single pass is [`reap_once`] (or clock-explicit
//! [`reap_once_at`] for tests and operator tooling).
//!
//! Division of labour, preserved exactly:
//!
//! * `service.rs`'s [`RuntimeRegistryService::reap_stale`] performs the
//!   stop transitions and logs per-record failures — the ONE
//!   transition path, unchanged.
//! * This file adds the CADENCE (the periodic task), the REPORT
//!   ([`ReaperReport`] — what an operator or the supervisor sees) and
//!   the failure roll-up through [`crate::runtime_registry::store::log_reap_failures`].
//!
//! Safety properties:
//!
//! * **Idempotent** — a runtime already reaped by another replica is
//!   simply absent from the stale set on this pass; terminal statuses
//!   stick in the store, so double-stopping is a no-op.
//! * **Bounded** — one pass is a bounded query plus one update per
//!   stale runtime; no pass may loop, block or panic the worker.
//! * **Never worsens liveness** — a failed pass only delays recovery
//!   to the next tick; it never marks a healthy runtime stale (the
//!   staleness cut-off is the lease policy's, applied store-side).
//! * **Fail-soft, not silent** — every failure is logged; the report
//!   counts it so sweeps remain observable.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tracing::{info, warn};

use bot_core::error::BotResult;
use bot_core::tenant::RuntimeId;

use crate::runtime_registry::service::RuntimeRegistryService;
use crate::runtime_registry::store::log_reap_failures;

/// Default reaping cadence. Deliberately SHORTER than the lease
/// policy's default `stale_after` (90 s) so a dead worker is cleaned up
/// within roughly one staleness window, not two.
pub const DEFAULT_REAP_INTERVAL: Duration = Duration::from_secs(30);

/// What one reaping pass did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaperReport {
    /// How many stale live runtimes the pass FOUND (before acting).
    pub swept: usize,
    /// How many of those were successfully stopped.
    pub reaped: usize,
    /// How many could not be stopped this pass (logged; the next pass
    /// retries them — terminal-sticky statuses make that safe).
    pub failed: usize,
    /// When the pass ran.
    pub at: DateTime<Utc>,
}

impl ReaperReport {
    /// An empty pass (nothing was stale).
    pub fn empty(at: DateTime<Utc>) -> Self {
        ReaperReport {
            swept: 0,
            reaped: 0,
            failed: 0,
            at,
        }
    }

    /// Nothing found, nothing done?
    pub fn is_empty(&self) -> bool {
        self.swept == 0
    }

    /// The internal ledger must always balance.
    pub fn is_balanced(&self) -> bool {
        self.swept == self.reaped + self.failed
    }

    /// One-line summary for logs and the supervisor's sweep report.
    pub fn summary(&self) -> String {
        format!(
            "reaper pass at {}: swept {} stale runtime(s), reaped {}, failed {}",
            self.at, self.swept, self.reaped, self.failed
        )
    }
}

impl Default for ReaperReport {
    fn default() -> Self {
        ReaperReport::empty(Utc::now())
    }
}

/// One reaping pass at the current time.
pub async fn reap_once(service: &Arc<RuntimeRegistryService>) -> BotResult<ReaperReport> {
    reap_once_at(service, Utc::now()).await
}

/// One reaping pass at an explicit time (tests advance the clock;
/// operators can run an on-demand pass through the same code).
pub async fn reap_once_at(
    service: &Arc<RuntimeRegistryService>,
    now: DateTime<Utc>,
) -> BotResult<ReaperReport> {
    // 1. What is stale RIGHT NOW, before acting. The staleness
    //    cut-off is the service's lease policy, applied store-side —
    //    this pass never invents its own liveness rules.
    let stale = service
        .store()
        .stale_since(now - service.policy().stale_after)
        .await?;
    if stale.is_empty() {
        return Ok(ReaperReport::empty(now));
    }

    // 2. Perform the stops through the ONE sanctioned transition path.
    //    `reap_stale` logs per-record failures itself; it returns only
    //    the runtimes it actually stopped.
    let reaped = service.reap_stale(now).await?;

    // 3. Roll the failures up (found-stale but not stopped) through
    //    the store's helper so every consumer sees the same shape.
    let reaped_ids: Vec<RuntimeId> = reaped.iter().map(|r| r.runtime_id).collect();
    let failed_ids: Vec<RuntimeId> = stale
        .iter()
        .map(|r| r.runtime_id)
        .filter(|id| !reaped_ids.contains(id))
        .collect();
    if !failed_ids.is_empty() {
        log_reap_failures(
            &failed_ids,
            &bot_core::error::BotError::db(
                "stale runtime could not be stopped on this pass; the next pass retries",
            ),
        );
    }

    let report = ReaperReport {
        swept: stale.len(),
        reaped: reaped.len(),
        failed: failed_ids.len(),
        at: now,
    };
    debug_assert!(report.is_balanced(), "reaper report must balance");
    if report.reaped > 0 {
        info!(
            swept = report.swept,
            reaped = report.reaped,
            failed = report.failed,
            "reaper pass cleaned up stale tenant runtimes"
        );
    }
    Ok(report)
}

/// Spawn the periodic reaper. Runs until the task is aborted (process
/// shutdown); each pass is bounded and a failed pass only delays to
/// the next tick. Aborting the handle is the shutdown path — the loop
/// itself has no exit condition by design.
///
/// The first pass waits one full interval — startup has just
/// registered this process's own runtimes with fresh heartbeats, so
/// there is nothing to reap in the first moments of a boot. An empty
/// pass is silent (nothing to report); every other outcome is already
/// logged by [`reap_once`] and the store's failure helper.
pub fn spawn_reaper(
    service: Arc<RuntimeRegistryService>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = reap_once(&service).await {
                warn!(
                    error = %e,
                    interval_ms = interval.as_millis() as u64,
                    "reaper pass failed; the next tick retries"
                );
            }
        }
    })
}

/// Spawn the periodic reaper on the default cadence
/// ([`DEFAULT_REAP_INTERVAL`]).
pub fn spawn_default_reaper(service: Arc<RuntimeRegistryService>) -> tokio::task::JoinHandle<()> {
    spawn_reaper(service, DEFAULT_REAP_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::model::TenantRuntimeRecord;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use bot_core::tenant::{OrganizationId, RuntimeGeneration};
    use chrono::Duration as ChronoDuration;

    fn service() -> Arc<RuntimeRegistryService> {
        Arc::new(RuntimeRegistryService::new(Arc::new(
            MemoryRuntimeStore::new(),
        )))
    }

    /// Insert a runtime whose heartbeat is already ancient history —
    /// the shape a dead worker leaves behind.
    async fn dead_worker(service: &Arc<RuntimeRegistryService>, org: OrganizationId) -> RuntimeId {
        let record = TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "worker-that-died",
            Utc::now() - ChronoDuration::seconds(600),
        );
        let runtime_id = record.runtime_id;
        service.store().insert(&record).await.unwrap();
        runtime_id
    }

    #[tokio::test]
    async fn a_fresh_runtime_is_never_swept() {
        let service = service();
        let org = OrganizationId::new();
        service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();

        let report = reap_once(&service).await.unwrap();
        assert!(report.is_empty());
        assert!(report.is_balanced());
        assert!(report.summary().contains("swept 0"));
        // And the live runtime is untouched.
        assert!(service.current(org).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_stale_runtime_is_reaped_in_one_pass() {
        let service = service();
        let org = OrganizationId::new();
        let runtime_id = dead_worker(&service, org).await;

        let report = reap_once(&service).await.unwrap();
        assert_eq!(report.swept, 1);
        assert_eq!(report.reaped, 1);
        assert_eq!(report.failed, 0);
        assert!(report.is_balanced());

        // The tenant has no live runtime any more, and the row itself
        // is terminal (stopped, not deleted).
        assert!(service.current(org).await.unwrap().is_none());
        let row = service.store().get(runtime_id).await.unwrap().unwrap();
        assert!(row.status.is_terminal());
    }

    #[tokio::test]
    async fn reaping_is_idempotent_across_passes() {
        let service = service();
        let org = OrganizationId::new();
        dead_worker(&service, org).await;

        let first = reap_once(&service).await.unwrap();
        assert_eq!(first.reaped, 1);
        // A second pass over the same state finds nothing to do.
        let second = reap_once(&service).await.unwrap();
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn reap_once_at_honours_the_explicit_clock() {
        let service = service();
        let org = OrganizationId::new();
        service
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();

        // At the current time the runtime is fresh…
        let now = reap_once_at(&service, Utc::now()).await.unwrap();
        assert!(now.is_empty());

        // …and far in the future the same row is stale.
        let later = reap_once_at(&service, Utc::now() + ChronoDuration::seconds(3_600))
            .await
            .unwrap();
        assert_eq!(later.reaped, 1);
    }

    #[tokio::test]
    async fn the_periodic_reaper_cleans_up_on_its_cadence() {
        let service = service();
        let org = OrganizationId::new();
        dead_worker(&service, org).await;

        let handle = spawn_reaper(service.clone(), Duration::from_millis(20));
        // Give the reaper a few cadences to act.
        tokio::time::sleep(Duration::from_millis(120)).await;

        assert!(
            service.current(org).await.unwrap().is_none(),
            "the dead worker must be reaped within a few cadences"
        );
        handle.abort();
    }

    #[tokio::test]
    async fn an_empty_tenant_registry_reports_an_empty_pass() {
        let service = service();
        let report = reap_once(&service).await.unwrap();
        assert_eq!(report, ReaperReport::empty(report.at));
        assert_eq!(report.swept, 0);
    }
}
