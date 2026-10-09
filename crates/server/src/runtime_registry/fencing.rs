//! Lease/generation fencing logic (STEP 3 file 43).
//!
//! THE rule: a worker may execute for a tenant only while its fence
//! token (runtime id + generation) matches the registry's current LIVE
//! runtime. Anything else — an older generation (the worker was rotated
//! away), a different runtime id, a missing runtime, a dead status, a
//! stale heartbeat — is STALE and must be denied.
//!
//! `verify` is the registry-side check the execution guard, the
//! background job guard and the heartbeat loop all share. It never
//! trusts the claim; it always reads the registry.

use std::sync::Arc;

use chrono::Utc;

use bot_core::error::{BotError, BotResult};
use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

use super::lease::{evaluate, LeasePolicy, LeaseVerdict};
use super::model::{FenceToken, TenantRuntimeRecord};
use super::store::RuntimeStore;

/// The verdict of a fence verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceVerdict {
    /// The token matches the tenant's current live runtime.
    Current(TenantRuntimeRecord),
    /// The tenant has a live runtime, but it is a different instance —
    /// the claimant was superseded.
    Superseded {
        /// The runtime the claimant thought it was.
        claimed: FenceToken,
        /// The registry's current live runtime id.
        current: RuntimeId,
    },
    /// The tenant's runtime holds a newer generation — the claimant is a
    /// stale worker from before a rotation.
    StaleGeneration {
        /// The generation the claimant holds.
        claimed: RuntimeGeneration,
        /// The generation the registry's live runtime holds.
        current: RuntimeGeneration,
    },
    /// The tenant has no live runtime at all.
    NoLiveRuntime {
        /// The tenant in question.
        organization_id: OrganizationId,
    },
    /// The tenant's live runtime exists but is not lease-live (stale
    /// heartbeat or expired hard lease). Treated as unusable — the
    /// reaper will stop it; execution waits for a healthy runtime.
    NotLeaseLive(LeaseVerdict),
}

impl FenceVerdict {
    /// Is this the only verdict that authorizes execution?
    pub fn is_current(&self) -> bool {
        matches!(self, FenceVerdict::Current(_))
    }

    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            FenceVerdict::Current(_) => "current",
            FenceVerdict::Superseded { .. } => "superseded",
            FenceVerdict::StaleGeneration { .. } => "stale_generation",
            FenceVerdict::NoLiveRuntime { .. } => "no_live_runtime",
            FenceVerdict::NotLeaseLive(_) => "not_lease_live",
        }
    }

    /// A log-safe one-line explanation (ids and generations only).
    pub fn explain(&self) -> String {
        match self {
            FenceVerdict::Current(r) => {
                format!("fence current (runtime {} {})", r.runtime_id, r.generation)
            }
            FenceVerdict::Superseded { claimed, current } => format!(
                "fence superseded: claimed runtime {} but the tenant's live runtime is {}",
                claimed.runtime_id, current
            ),
            FenceVerdict::StaleGeneration { claimed, current } => {
                format!("fence stale: claimed {claimed} but the live runtime holds {current}")
            }
            FenceVerdict::NoLiveRuntime { organization_id } => {
                format!("fence unavailable: organization {organization_id} has no live runtime")
            }
            FenceVerdict::NotLeaseLive(v) => {
                format!("fence unusable: runtime not lease-live ({})", v.as_str())
            }
        }
    }
}

/// Verify a claimed fence token against the registry.
pub async fn verify(
    store: &Arc<dyn RuntimeStore>,
    claimed: &FenceToken,
) -> BotResult<FenceVerdict> {
    verify_with_policy(store, claimed, &LeasePolicy::default()).await
}

/// Verify with an explicit lease policy (tests use tight thresholds).
pub async fn verify_with_policy(
    store: &Arc<dyn RuntimeStore>,
    claimed: &FenceToken,
    policy: &LeasePolicy,
) -> BotResult<FenceVerdict> {
    let current = store
        .live_for(claimed.organization_id)
        .await
        .map_err(|e| BotError::db(format!("fence lookup failed: {e}")))?;
    let Some(current) = current else {
        return Ok(FenceVerdict::NoLiveRuntime {
            organization_id: claimed.organization_id,
        });
    };
    let lease = evaluate(&current, policy, Utc::now());
    if !lease.is_live() {
        return Ok(FenceVerdict::NotLeaseLive(lease));
    }
    if current.runtime_id != claimed.runtime_id {
        return Ok(FenceVerdict::Superseded {
            claimed: *claimed,
            current: current.runtime_id,
        });
    }
    if current.generation != claimed.generation {
        return Ok(FenceVerdict::StaleGeneration {
            claimed: claimed.generation,
            current: current.generation,
        });
    }
    Ok(FenceVerdict::Current(current))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_registry::model::{RuntimeStatus, TenantRuntimeRecord};
    use crate::runtime_registry::store::{MemoryRuntimeStore, RuntimeStore};
    use bot_core::tenant::{OrganizationId, RuntimeGeneration, RuntimeId};

    async fn seeded(org: OrganizationId) -> (Arc<dyn RuntimeStore>, TenantRuntimeRecord) {
        let store: Arc<dyn RuntimeStore> = Arc::new(MemoryRuntimeStore::new());
        let rec = TenantRuntimeRecord::new_active(
            org,
            RuntimeId::new(),
            RuntimeGeneration::first(),
            "worker",
            Utc::now(),
        );
        store.insert(&rec).await.unwrap();
        (store, rec)
    }

    #[tokio::test]
    async fn current_token_verifies() {
        let org = OrganizationId::new();
        let (store, rec) = seeded(org).await;
        let verdict = verify(&store, &rec.fence_token()).await.unwrap();
        assert!(verdict.is_current());
        assert_eq!(verdict.as_str(), "current");
    }

    #[tokio::test]
    async fn rotated_runtime_is_stale() {
        let org = OrganizationId::new();
        let (store, rec) = seeded(org).await;
        let old_token = rec.fence_token();
        store
            .rotate(org, RuntimeId::new(), "successor", Utc::now())
            .await
            .unwrap();
        let verdict = verify(&store, &old_token).await.unwrap();
        assert!(!verdict.is_current());
        assert!(matches!(verdict, FenceVerdict::Superseded { .. }));
        assert!(verdict.explain().contains("superseded"));
    }

    #[tokio::test]
    async fn missing_runtime_is_denied() {
        let store: Arc<dyn RuntimeStore> = Arc::new(MemoryRuntimeStore::new());
        let token = FenceToken {
            organization_id: OrganizationId::new(),
            runtime_id: RuntimeId::new(),
            generation: RuntimeGeneration::first(),
        };
        let verdict = verify(&store, &token).await.unwrap();
        assert!(matches!(verdict, FenceVerdict::NoLiveRuntime { .. }));
        assert!(!verdict.is_current());
    }

    #[tokio::test]
    async fn stale_heartbeat_makes_the_fence_unusable() {
        let org = OrganizationId::new();
        let (_store, mut rec) = seeded(org).await;
        rec.heartbeat_at = Utc::now() - chrono::Duration::seconds(600);
        // Re-seed with the stale heartbeat.
        let store: Arc<dyn RuntimeStore> = Arc::new(MemoryRuntimeStore::new());
        store.insert(&rec).await.unwrap();
        let verdict = verify(&store, &rec.fence_token()).await.unwrap();
        assert!(matches!(verdict, FenceVerdict::NotLeaseLive(_)));
        assert!(!verdict.is_current());
    }

    #[tokio::test]
    async fn stopped_runtime_is_not_live() {
        let org = OrganizationId::new();
        let (store, rec) = seeded(org).await;
        store
            .set_status(rec.runtime_id, RuntimeStatus::Stopped, Utc::now())
            .await
            .unwrap();
        let verdict = verify(&store, &rec.fence_token()).await.unwrap();
        assert!(matches!(verdict, FenceVerdict::NoLiveRuntime { .. }));
    }
}
