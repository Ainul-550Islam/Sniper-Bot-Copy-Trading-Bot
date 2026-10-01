//! Runtime lifecycle / provisioning / resolution service (STEP 3 file 40).
//!
//! [`RuntimeRegistryService`] is the ONE entry point the rest of the
//! server uses for tenant runtimes: register this process as a tenant's
//! runtime (the deployment organization at startup, a tenant on
//! provision), resolve the tenant's current runtime, verify fences, and
//! reap stale ones. It wraps a [`RuntimeStore`] (PostgreSQL when the
//! database is attached, in-memory otherwise) and applies the
//! [`LeasePolicy`] on top.
//!
//! Integration discipline: this service does NOT schedule work, does not
//! claim executions (that is `bot_core::ownership` / execution_claims)
//! and does not duplicate the process-wide HA layer (`bot_core::ha`).
//! It answers exactly: "which runtime may execute for this tenant right
//! now, and is this claimed one still it?"

use std::sync::Arc;

use chrono::{DateTime, Utc};
use tracing::{info, warn};

use bot_core::error::BotResult;
use bot_core::tenant::{OrganizationId, RuntimeId};

use super::fencing::{verify as verify_fence, FenceVerdict};
use super::lease::{evaluate, LeasePolicy};
use super::model::{FenceToken, RuntimeStatus, TenantRuntimeRecord};
use super::store::RuntimeStore;

/// The registry service.
#[derive(Clone)]
pub struct RuntimeRegistryService {
    store: Arc<dyn RuntimeStore>,
    policy: LeasePolicy,
}

impl RuntimeRegistryService {
    /// Build over a store with the default lease policy.
    pub fn new(store: Arc<dyn RuntimeStore>) -> Self {
        RuntimeRegistryService {
            store,
            policy: LeasePolicy::default(),
        }
    }

    /// Build with an explicit policy (tests, tuned deployments).
    pub fn with_policy(store: Arc<dyn RuntimeStore>, policy: LeasePolicy) -> Self {
        RuntimeRegistryService { store, policy }
    }

    /// The underlying store (heartbeat loop, reaping, tests).
    pub fn store(&self) -> &Arc<dyn RuntimeStore> {
        &self.store
    }

    /// The active lease policy.
    pub fn policy(&self) -> &LeasePolicy {
        &self.policy
    }

    /// The tenant's current LIVE runtime, if it is lease-live.
    pub async fn current(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<TenantRuntimeRecord>> {
        let Some(record) = self.store.live_for(organization_id).await? else {
            return Ok(None);
        };
        if evaluate(&record, &self.policy, Utc::now()).is_live() {
            Ok(Some(record))
        } else {
            Ok(None)
        }
    }

    /// Register (or re-register) a runtime for a tenant.
    ///
    /// * No live runtime → insert at the next generation (1 on first
    ///   registration).
    /// * Live runtime with a FRESH heartbeat (a healthy peer is already
    ///   serving this tenant) → refuse: split brain is never created
    ///   deliberately. The caller rotates explicitly if it means to
    ///   supersede.
    /// * Live runtime with a STALE heartbeat → rotate (the peer died);
    ///   the stale row is stopped and a new active row takes over.
    pub async fn ensure_active(
        &self,
        organization_id: OrganizationId,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> BotResult<EnsureOutcome> {
        match self.store.live_for(organization_id).await? {
            None => {
                let generation = self.store.next_generation(organization_id).await?;
                let record = TenantRuntimeRecord::new_active(
                    organization_id,
                    RuntimeId::new(),
                    generation,
                    worker_id,
                    now,
                );
                self.store.insert(&record).await?;
                Ok(EnsureOutcome::Registered(record))
            }
            Some(existing) => {
                if existing.worker_id == worker_id
                    || !existing.is_heartbeat_fresh(now, self.policy.stale_after)
                {
                    // Our own row (restart of the same worker identity) or
                    // a dead peer: rotation is the sanctioned takeover.
                    let record = self
                        .store
                        .rotate(organization_id, RuntimeId::new(), worker_id, now)
                        .await?;
                    Ok(EnsureOutcome::Rotated(record))
                } else {
                    Err(bot_core::error::BotError::db(format!(
                        "runtime registration refused: tenant already has a healthy runtime \
                         (worker {}, heartbeat {})",
                        existing.worker_id, existing.heartbeat_at
                    )))
                }
            }
        }
    }

    /// Explicit rotation: supersede the tenant's live runtime (if any)
    /// with a fresh one. Operator failover and provisioning use this.
    pub async fn rotate(
        &self,
        organization_id: OrganizationId,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> BotResult<TenantRuntimeRecord> {
        self.store
            .rotate(organization_id, RuntimeId::new(), worker_id, now)
            .await
    }

    /// Record a heartbeat for a runtime.
    pub async fn heartbeat(&self, runtime_id: RuntimeId, now: DateTime<Utc>) -> BotResult<bool> {
        self.store.heartbeat(runtime_id, now).await
    }

    /// Verify a claimed fence token (the shared check for guards and
    /// background jobs).
    pub async fn verify(&self, claimed: &FenceToken) -> BotResult<FenceVerdict> {
        verify_fence(&self.store, claimed).await
    }

    /// Mark a runtime draining (graceful handover: refuses new work).
    pub async fn drain(&self, runtime_id: RuntimeId, now: DateTime<Utc>) -> BotResult<()> {
        self.store
            .set_status(runtime_id, RuntimeStatus::Draining, now)
            .await
    }

    /// Mark a runtime stopped.
    pub async fn stop(&self, runtime_id: RuntimeId, now: DateTime<Utc>) -> BotResult<()> {
        self.store
            .set_status(runtime_id, RuntimeStatus::Stopped, now)
            .await
    }

    /// Recovery pass: stop live runtimes whose heartbeat went stale.
    /// Returns the reaped records. Failures are logged, not fatal — the
    /// next sweep retries.
    pub async fn reap_stale(&self, now: DateTime<Utc>) -> BotResult<Vec<TenantRuntimeRecord>> {
        let stale = self
            .store
            .stale_since(now - self.policy.stale_after)
            .await?;
        let mut reaped = Vec::new();
        for record in stale {
            match self
                .store
                .set_status(record.runtime_id, RuntimeStatus::Stopped, now)
                .await
            {
                Ok(()) => {
                    info!(
                        runtime = %record.runtime_id,
                        organization = %record.organization_id,
                        last_heartbeat = %record.heartbeat_at,
                        "reaped stale tenant runtime"
                    );
                    reaped.push(record);
                }
                Err(e) => {
                    warn!(
                        runtime = %record.runtime_id,
                        error = %e,
                        "could not reap stale tenant runtime; next sweep retries"
                    );
                }
            }
        }
        Ok(reaped)
    }
}

/// The outcome of [`RuntimeRegistryService::ensure_active`].
#[derive(Debug, Clone, PartialEq)]
pub enum EnsureOutcome {
    /// A new runtime was registered (no live runtime existed).
    Registered(TenantRuntimeRecord),
    /// The stale/own live runtime was superseded by a fresh one.
    Rotated(TenantRuntimeRecord),
}

impl EnsureOutcome {
    /// The runtime record either way.
    pub fn record(&self) -> &TenantRuntimeRecord {
        match self {
            EnsureOutcome::Registered(r) | EnsureOutcome::Rotated(r) => r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::store::MemoryRuntimeStore;
    use bot_core::tenant::RuntimeGeneration;

    fn service() -> RuntimeRegistryService {
        RuntimeRegistryService::new(Arc::new(MemoryRuntimeStore::new()))
    }

    #[tokio::test]
    async fn first_registration_is_generation_one() {
        let svc = service();
        let org = OrganizationId::new();
        let outcome = svc
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        assert!(matches!(outcome, EnsureOutcome::Registered(_)));
        assert_eq!(outcome.record().generation, RuntimeGeneration::first());
        assert!(outcome.record().status.is_live());
    }

    #[tokio::test]
    async fn a_healthy_peer_refuses_split_brain() {
        let svc = service();
        let org = OrganizationId::new();
        svc.ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        let err = svc.ensure_active(org, "worker-b", Utc::now()).await;
        assert!(err.is_err(), "a second healthy runtime must be refused");
    }

    #[tokio::test]
    async fn a_stale_peer_is_rotated_not_duplicated() {
        let svc = service();
        let org = OrganizationId::new();
        let first = svc
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        // The peer's heartbeat is now ancient history.
        let later = Utc::now() + chrono::Duration::seconds(600);
        let second = svc.ensure_active(org, "worker-b", later).await.unwrap();
        assert!(matches!(second, EnsureOutcome::Rotated(_)));
        assert_eq!(second.record().generation.raw(), 2);

        // The old runtime's fence is stale; the new one verifies.
        let old_verdict = svc.verify(&first.record().fence_token()).await.unwrap();
        assert!(!old_verdict.is_current());
        let new_verdict = svc.verify(&second.record().fence_token()).await.unwrap();
        assert!(new_verdict.is_current());
    }

    #[tokio::test]
    async fn reaping_stops_stale_runtimes_only() {
        let svc = service();
        let org = OrganizationId::new();
        let healthy = svc
            .ensure_active(org, "worker-a", Utc::now())
            .await
            .unwrap();
        let reaped = svc.reap_stale(Utc::now()).await.unwrap();
        assert!(reaped.is_empty(), "a fresh runtime must not be reaped");

        // Advance the clock far beyond the staleness threshold.
        let later = Utc::now() + chrono::Duration::seconds(3600);
        let reaped = svc.reap_stale(later).await.unwrap();
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].runtime_id, healthy.record().runtime_id);
        assert!(svc.current(org).await.unwrap().is_none());
    }
}
