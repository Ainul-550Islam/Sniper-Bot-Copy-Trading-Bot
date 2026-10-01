//! Runtime heartbeat (STEP 3 file 42).
//!
//! The heartbeat task is a REAL tenant-aware background job: it carries
//! the deployment's [`FenceToken`] (see `tenant_background::job_context`),
//! re-verifies it through the job guard before every beat, and only then
//! records the heartbeat. A runtime that lost its fence (rotated away
//! while the task still runs) stops beating — the registry reaps it.

use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use bot_core::error::BotResult;
use bot_core::tenant::RuntimeId;

use super::lease::LeasePolicy;
use super::model::FenceToken;
use super::store::RuntimeStore;

/// Record one heartbeat for a runtime.
///
/// `Ok(false)` = the runtime is no longer live in the registry — the
/// caller (the loop, a worker) MUST stop acting for the tenant.
pub async fn record(
    store: &Arc<dyn RuntimeStore>,
    runtime_id: RuntimeId,
    now: DateTime<Utc>,
) -> BotResult<bool> {
    store.heartbeat(runtime_id, now).await
}

/// The tenant-aware heartbeat loop for one runtime.
///
/// Every tick re-verifies the fence token against the registry (the
/// same check the execution guard performs — never trust that a still
/// running task still owns the tenant), then records the heartbeat.
/// The loop exits when:
///
/// * the fence went stale (the runtime was rotated/superseded), or
/// * the heartbeat write reports the runtime is no longer live, or
/// * a store error repeats (the loop logs and retries a bounded number
///   of times, then gives up so a broken store cannot spin).
///
/// The returned handle is aborted by the caller on shutdown (the same
/// lifecycle the other background workers use).
pub fn spawn_heartbeat_loop(
    store: Arc<dyn RuntimeStore>,
    token: FenceToken,
    policy: LeasePolicy,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval =
            StdDuration::from_secs(policy.heartbeat_interval.num_seconds().max(1) as u64);
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick completes immediately; skip it so a heartbeat is
        // only recorded after a full interval has passed.
        ticker.tick().await;
        let mut consecutive_errors = 0u32;
        const MAX_CONSECUTIVE_ERRORS: u32 = 10;
        loop {
            ticker.tick().await;
            // Fence first: a rotated runtime must not "revive" itself.
            match super::fencing::verify(&store, &token).await {
                Ok(verdict) if verdict.is_current() => {}
                Ok(verdict) => {
                    info!(
                        runtime = %token.runtime_id,
                        verdict = verdict.as_str(),
                        "runtime heartbeat loop stopping: fence lost"
                    );
                    return;
                }
                Err(e) => {
                    consecutive_errors += 1;
                    warn!(error = %e, "runtime fence verification failed");
                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        warn!("runtime heartbeat loop giving up: store unreachable");
                        return;
                    }
                    continue;
                }
            }
            let now = Utc::now();
            match store.heartbeat(token.runtime_id, now).await {
                Ok(true) => consecutive_errors = 0,
                Ok(false) => {
                    info!(
                        runtime = %token.runtime_id,
                        "runtime heartbeat loop stopping: registry reports the runtime is not live"
                    );
                    return;
                }
                Err(e) => {
                    consecutive_errors += 1;
                    warn!(error = %e, "runtime heartbeat write failed");
                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        warn!("runtime heartbeat loop giving up: store unreachable");
                        return;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::{MemoryRuntimeStore, RuntimeStore};
    use bot_core::tenant::{OrganizationId, RuntimeGeneration};
    use chrono::Duration;

    #[tokio::test]
    async fn heartbeats_extend_liveness_until_the_runtime_stops() {
        let store: Arc<dyn RuntimeStore> = Arc::new(MemoryRuntimeStore::new());
        let org = OrganizationId::new();
        let rec = super::super::model::TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "worker",
            Utc::now(),
        );
        store.insert(&rec).await.unwrap();

        let later = Utc::now() + Duration::seconds(30);
        assert!(record(&store, rec.runtime_id, later).await.unwrap());
        assert_eq!(
            store
                .get(rec.runtime_id)
                .await
                .unwrap()
                .unwrap()
                .heartbeat_at,
            later
        );

        store
            .set_status(
                rec.runtime_id,
                crate::runtime_registry::model::RuntimeStatus::Stopped,
                later,
            )
            .await
            .unwrap();
        assert!(!record(&store, rec.runtime_id, later).await.unwrap());
    }

    #[tokio::test]
    async fn heartbeat_loop_exits_when_the_fence_is_lost() {
        let store: Arc<dyn RuntimeStore> = Arc::new(MemoryRuntimeStore::new());
        let org = OrganizationId::new();
        let rec = super::super::model::TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "worker",
            Utc::now(),
        );
        store.insert(&rec).await.unwrap();

        let policy = LeasePolicy {
            heartbeat_interval: Duration::seconds(1),
            ..LeasePolicy::default()
        };
        let handle = spawn_heartbeat_loop(store.clone(), rec.fence_token(), policy);

        // Rotate: the fence token goes stale, the loop must exit on its
        // next tick instead of reviving the dead runtime.
        store
            .rotate(org, RuntimeId::new(), "successor", Utc::now())
            .await
            .unwrap();
        let exited = tokio::time::timeout(StdDuration::from_secs(6), handle).await;
        assert!(
            exited.is_ok(),
            "heartbeat loop must exit after losing the fence"
        );
        // And the dead runtime was NOT revived: the successor is live.
        let live = store.live_for(org).await.unwrap().unwrap();
        assert_ne!(live.runtime_id, rec.runtime_id);
    }
}
