//! STEP 3 integration: the tenant observability layer — metrics, the
//! decision log and the health roll-up, consistent with each other.

use std::sync::Arc;

use bot_core::tenant::OrganizationId;
use chrono::Utc;
use sniper_suite::runtime_registry::{MemoryRuntimeStore, RuntimeRegistryService};
use sniper_suite::tenant_observability::decision_log::{
    DecisionLogEntry, DecisionLogSink, MemoryDecisionLogSink,
};
use sniper_suite::tenant_observability::health::{HealthStatus, TenantHealthRollup};
use sniper_suite::tenant_observability::metrics::TenantMetrics;

#[tokio::test]
async fn metrics_and_health_agree() {
    let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )));
    let org = OrganizationId::new();
    runtime
        .ensure_active(org, "worker", Utc::now())
        .await
        .unwrap();

    let metrics = Arc::new(TenantMetrics::new());
    metrics.record_decision(org, "allow").await;
    metrics.record_decision(org, "allow").await;
    metrics.record_decision(org, "wallet_not_bound").await;
    metrics.record_execution_started(org).await;
    metrics.record_execution_outcome(org, "confirmed").await;

    let rollup = TenantHealthRollup::new(runtime, metrics);
    let health = rollup.tenant(org).await.unwrap();
    assert_eq!(health.status, HealthStatus::Healthy);
    assert_eq!(health.allows, 2);
    assert_eq!(health.denies, 1);
    assert_eq!(health.runtime_generation, Some(1));
}

#[tokio::test]
async fn the_decision_log_and_counters_tell_the_same_story() {
    let org = OrganizationId::new();
    let sink = MemoryDecisionLogSink::new();
    let metrics = TenantMetrics::new();

    for (label, detail) in [
        ("allow", "all guards passed".to_string()),
        (
            "position_size_exceeds",
            "requested size 50000 USD exceeds the effective cap 10000 USD".to_string(),
        ),
    ] {
        sink.append(DecisionLogEntry::from_static(
            org,
            label,
            detail,
            "copy",
            "paper",
            "req",
            "user-1",
            Utc::now(),
        ))
        .await
        .unwrap();
        metrics.record_decision(org, label).await;
    }

    let recent = sink.recent(org, 10).await.unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].decision, "position_size_exceeds", "newest first");

    let snapshot = metrics.snapshot(org).await;
    assert_eq!(snapshot.allows, 1);
    assert_eq!(snapshot.total_denies(), 1);
}

#[tokio::test]
async fn a_tenant_without_a_runtime_reports_no_runtime() {
    let runtime = Arc::new(RuntimeRegistryService::new(Arc::new(
        MemoryRuntimeStore::new(),
    )));
    let metrics = Arc::new(TenantMetrics::new());
    let rollup = TenantHealthRollup::new(runtime, metrics);
    let health = rollup.tenant(OrganizationId::new()).await.unwrap();
    assert_eq!(health.status, HealthStatus::NoRuntime);
    assert_eq!(health.runtime_worker, None);
}
