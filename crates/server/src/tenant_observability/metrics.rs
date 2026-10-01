//! Per-tenant decision and execution counters (STEP 3 file 54).
//!
//! In-memory, atomically updated, snapshot-readable. The platform
//! exporter (obs.rs) can poll snapshots into its global gauges; the
//! per-tenant view avoids one global label explosion while keeping the
//! same vocabulary (the decision labels ARE the gateway's).

use std::collections::HashMap;
use std::sync::Arc;

use bot_core::tenant::OrganizationId;
use tokio::sync::RwLock;

/// One tenant's counters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TenantCounters {
    /// Gateway allows.
    pub allows: u64,
    /// Gateway denies, by the deny label.
    pub denies_by_label: HashMap<String, u64>,
    /// Executions started under an issued context.
    pub executions_started: u64,
    /// Executions finished, by outcome label.
    pub executions_by_outcome: HashMap<String, u64>,
    /// Fence failures observed (any source).
    pub fence_failures: u64,
}

impl TenantCounters {
    /// Total denies across labels.
    pub fn total_denies(&self) -> u64 {
        self.denies_by_label.values().sum()
    }
}

/// The per-tenant metrics registry.
#[derive(Default)]
pub struct TenantMetrics {
    counters: RwLock<HashMap<OrganizationId, TenantCounters>>,
}

impl TenantMetrics {
    /// An empty registry.
    pub fn new() -> Self {
        TenantMetrics::default()
    }

    /// Record a gateway decision.
    pub async fn record_decision(&self, organization_id: OrganizationId, label: &str) {
        let mut counters = self.counters.write().await;
        let entry = counters.entry(organization_id).or_default();
        if label == "allow" {
            entry.allows += 1;
        } else {
            *entry.denies_by_label.entry(label.to_string()).or_insert(0) += 1;
        }
    }

    /// Record an execution start.
    pub async fn record_execution_started(&self, organization_id: OrganizationId) {
        self.counters
            .write()
            .await
            .entry(organization_id)
            .or_default()
            .executions_started += 1;
    }

    /// Record an execution outcome.
    pub async fn record_execution_outcome(&self, organization_id: OrganizationId, outcome: &str) {
        let mut counters = self.counters.write().await;
        let entry = counters.entry(organization_id).or_default();
        *entry
            .executions_by_outcome
            .entry(outcome.to_string())
            .or_insert(0) += 1;
    }

    /// Record a fence failure.
    pub async fn record_fence_failure(&self, organization_id: OrganizationId) {
        self.counters
            .write()
            .await
            .entry(organization_id)
            .or_default()
            .fence_failures += 1;
    }

    /// One tenant's snapshot.
    pub async fn snapshot(&self, organization_id: OrganizationId) -> TenantCounters {
        self.counters
            .read()
            .await
            .get(&organization_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Every tenant's snapshot (the exporter polls this).
    pub async fn snapshot_all(&self) -> HashMap<OrganizationId, TenantCounters> {
        self.counters.read().await.clone()
    }
}

/// Share the registry.
pub fn shared() -> Arc<TenantMetrics> {
    Arc::new(TenantMetrics::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn decisions_count_by_label() {
        let metrics = TenantMetrics::new();
        let org = OrganizationId::new();
        metrics.record_decision(org, "allow").await;
        metrics.record_decision(org, "allow").await;
        metrics.record_decision(org, "wallet_not_bound").await;
        metrics.record_decision(org, "position_size_exceeds").await;

        let snapshot = metrics.snapshot(org).await;
        assert_eq!(snapshot.allows, 2);
        assert_eq!(snapshot.total_denies(), 2);
        assert_eq!(snapshot.denies_by_label["wallet_not_bound"], 1);
    }

    #[tokio::test]
    async fn tenants_are_isolated() {
        let metrics = TenantMetrics::new();
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        metrics.record_decision(a, "allow").await;
        assert_eq!(metrics.snapshot(b).await.allows, 0);
        assert_eq!(metrics.snapshot_all().await.len(), 1);
    }

    #[tokio::test]
    async fn executions_and_fences_track_their_own_counters() {
        let metrics = TenantMetrics::new();
        let org = OrganizationId::new();
        metrics.record_execution_started(org).await;
        metrics.record_execution_started(org).await;
        metrics.record_execution_outcome(org, "confirmed").await;
        metrics.record_fence_failure(org).await;

        let snapshot = metrics.snapshot(org).await;
        assert_eq!(snapshot.executions_started, 2);
        assert_eq!(snapshot.executions_by_outcome["confirmed"], 1);
        assert_eq!(snapshot.fence_failures, 1);
    }
}
