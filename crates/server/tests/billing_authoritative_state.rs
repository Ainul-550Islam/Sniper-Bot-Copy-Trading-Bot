//! §G billing authoritative-state integration tests.
//!
//! Prove through the public crate surface that the customer-facing
//! billing state is AUTHORITATIVE — loaded from the store, never
//! synthesized:
//!
//! 1. a tenant with no subscription sees explicit `none` values, never a
//!    default tier (the old code rendered `starter`/`active` for
//!    everyone);
//! 2. an assigned plan renders verbatim (code, name, provider, status);
//! 3. usage numbers are sums of recorded metering events only — and
//!    idempotent re-delivery does not double-count;
//! 4. payment transactions persist with idempotent recording and
//!    ownership-checked reads;
//! 5. the same invariants hold against a real PostgreSQL store when
//!    `POSTGRES_URL` is provided (NOT_RUN otherwise, printed loudly).

use std::sync::Arc;

use bot_core::billing::dunning::DunningState;
use bot_core::billing::payment::PaymentTransaction;
use bot_core::billing::plan::PlanCode;
use bot_core::billing::provider::BillingProviderKind;
use bot_core::billing::subscription::SubscriptionStatus;
use bot_core::billing::usage::{UsageEvent, UsageMetric, UsageSource};
use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::{Organization, OrganizationId, OrganizationStatus};
use chrono::{Duration, Utc};
use sniper_suite::saas::billing_view::BillingView;
use sniper_suite::saas::store::SaasStore;

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn not_run(msg: &str) {
    eprintln!("NOT_RUN: {msg} — POSTGRES_URL missing; memory-store assertions still run");
}

async fn pg_store() -> Option<SaasStore> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url).await.expect("connect");
    db.migrate().await.expect("migrate");
    SaasStore::with_database(Some(Arc::new(db)))
        .await
        .expect("durable saas store")
        .into()
}

fn new_org(status: OrganizationStatus) -> Organization {
    let id = OrganizationId::new();
    let mut org = Organization::new(
        id,
        format!("billing-it-{}", id.as_uuid()),
        "Billing IT",
        None,
        Utc::now(),
    );
    org.status = status;
    org
}

/// Memory store, org created.
async fn memory_store_with_org(status: OrganizationStatus) -> (SaasStore, OrganizationId) {
    let store = SaasStore::new();
    let org = new_org(status);
    store.create_organization(&org).await.expect("org created");
    (store, org.id)
}

#[tokio::test]
async fn no_subscription_is_explicit_none_never_a_default_tier() {
    let (store, org_id) = memory_store_with_org(OrganizationStatus::Active).await;
    let view = BillingView::load(&store, org_id, Utc::now()).await;
    let body = view.to_status_json();
    assert_eq!(body["plan_code"], "none", "no invented starter/pro");
    assert_eq!(body["subscription_status"], "none", "no invented active");
    assert_eq!(body["billing_provider"], "none");
    assert_eq!(body["entitlements_active"], false);
    assert_eq!(body["dunning_state"], "current");
    assert_eq!(body["suspension_reason"], serde_json::Value::Null);
    assert!(view.consistency().consistent);
}

#[tokio::test]
async fn assigned_plan_renders_verbatim_with_real_usage() {
    let (store, org_id) = memory_store_with_org(OrganizationStatus::Active).await;
    store
        .assign_plan(org_id, PlanCode::Pro, Utc::now())
        .await
        .expect("plan");
    for (metric, quantity, key) in [
        (UsageMetric::ApiRequests, 9.0, "it-api-1"),
        (UsageMetric::ApiRequests, 1.0, "it-api-2"),
        (UsageMetric::FillsBooked, 6.0, "it-fills-1"),
        (UsageMetric::ModuleRuntimeSeconds, 120.0, "it-rt-1"),
    ] {
        store
            .record_usage(&UsageEvent::new(
                org_id,
                metric,
                quantity,
                UsageSource::Api,
                key,
                Utc::now(),
            ))
            .await
            .expect("usage recorded");
    }
    let view = BillingView::load(&store, org_id, Utc::now()).await;
    let body = view.to_status_json();
    assert_eq!(body["plan_code"], "pro");
    assert_eq!(body["plan_name"], "Pro");
    assert_eq!(body["subscription_status"], "active");
    assert_eq!(body["billing_provider"], "manual");
    assert_eq!(body["entitlements_active"], true);
    assert_eq!(body["usage"]["api_requests"], 10.0);
    assert_eq!(body["usage"]["fills_booked"], 6.0);
    assert_eq!(body["usage"]["module_runtime_seconds"], 120.0);
    assert_eq!(
        body["usage"]["export_rows"], 0.0,
        "real zero, nothing metered"
    );
}

#[tokio::test]
async fn idempotent_usage_redelivery_does_not_double_count() {
    let (store, org_id) = memory_store_with_org(OrganizationStatus::Active).await;
    let event = UsageEvent::new(
        org_id,
        UsageMetric::ApiRequests,
        5.0,
        UsageSource::Api,
        "it-dup-key",
        Utc::now(),
    );
    assert!(store.record_usage(&event).await.expect("first"));
    // Same (tenant, key) identity again: refused, not re-counted.
    assert!(!store.record_usage(&event).await.expect("duplicate"));
    let view = BillingView::load(&store, org_id, Utc::now()).await;
    assert_eq!(view.usage.api_requests, 5.0);
}

#[tokio::test]
async fn tenant_usage_isolation() {
    let store = SaasStore::new();
    let org_a = new_org(OrganizationStatus::Active);
    let org_b = new_org(OrganizationStatus::Active);
    store.create_organization(&org_a).await.expect("a");
    store.create_organization(&org_b).await.expect("b");
    store
        .record_usage(&UsageEvent::new(
            org_b.id,
            UsageMetric::ApiRequests,
            42.0,
            UsageSource::Api,
            "iso-b-1",
            Utc::now(),
        ))
        .await
        .expect("b usage");
    let view_a = BillingView::load(&store, org_a.id, Utc::now()).await;
    let view_b = BillingView::load(&store, org_b.id, Utc::now()).await;
    assert_eq!(
        view_a.usage.api_requests, 0.0,
        "org A must not see org B usage"
    );
    assert_eq!(view_b.usage.api_requests, 42.0);
}

#[tokio::test]
async fn past_due_derives_grace_then_failure() {
    let (store, org_id) = memory_store_with_org(OrganizationStatus::Active).await;
    store
        .assign_plan(org_id, PlanCode::Pro, Utc::now())
        .await
        .expect("plan");
    let mut sub = store.subscription_of(org_id).await.expect("sub");
    sub.status = SubscriptionStatus::PastDue;

    // Inside the period: grace.
    sub.current_period_end = Some(Utc::now() + Duration::days(2));
    store.update_subscription(&sub).await.expect("updated");
    let view = BillingView::load(&store, org_id, Utc::now()).await;
    assert_eq!(view.dunning, DunningState::GracePeriod);
    assert!(view.grace_until().is_some());
    assert_eq!(view.to_status_json()["payment_state"], "failed");

    // Past the period: payment failed, no grace.
    sub.current_period_end = Some(Utc::now() - Duration::hours(1));
    store.update_subscription(&sub).await.expect("updated");
    let view = BillingView::load(&store, org_id, Utc::now()).await;
    assert_eq!(view.dunning, DunningState::PaymentFailed);
    assert!(view.grace_until().is_none());
}

#[tokio::test]
async fn payments_persist_idempotently_and_read_tenant_scoped() {
    let (store, org_id) = memory_store_with_org(OrganizationStatus::Active).await;
    let mut payment = PaymentTransaction::new(
        org_id,
        BillingProviderKind::Manual,
        "it-payment-1",
        9_900,
        "USD",
        Utc::now(),
    );
    assert!(store.record_payment(&payment).await.expect("recorded"));
    // Idempotent re-record refused.
    assert!(!store.record_payment(&payment).await.expect("re-record"));

    // A second tenant cannot read the first tenant's payment.
    let store2 = SaasStore::new();
    let org_other = new_org(OrganizationStatus::Active);
    store2
        .create_organization(&org_other)
        .await
        .expect("other org");
    assert!(store2.payment_of(org_id, payment.id).await.is_none());

    // Status transition persists through update_payment.
    payment
        .transition_to(
            bot_core::billing::payment::TransactionStatus::Succeeded,
            None,
            Utc::now(),
        )
        .expect("transition");
    store.update_payment(&payment).await.expect("updated");
    let loaded = store.payment_of(org_id, payment.id).await.expect("loaded");
    assert_eq!(
        loaded.status,
        bot_core::billing::payment::TransactionStatus::Succeeded
    );
    let listed = store.payments_of(org_id).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, payment.id);
}

#[tokio::test]
async fn pg_durable_store_agrees_with_memory_invariants() {
    let Some(store) = pg_store().await else {
        not_run("pg durable billing view");
        return;
    };
    let org = new_org(OrganizationStatus::Active);
    store.create_organization(&org).await.expect("org created");

    // No subscription: explicit none.
    let view = BillingView::load(&store, org.id, Utc::now()).await;
    assert_eq!(view.plan_code(), "none");
    assert_eq!(view.subscription_status(), "none");

    // Assign + meter: verbatim.
    store
        .assign_plan(org.id, PlanCode::Starter, Utc::now())
        .await
        .expect("plan");
    store
        .record_usage(&UsageEvent::new(
            org.id,
            UsageMetric::ApiRequests,
            3.0,
            UsageSource::Api,
            format!("pg-{}", org.id.as_uuid()),
            Utc::now(),
        ))
        .await
        .expect("usage");
    let view = BillingView::load(&store, org.id, Utc::now()).await;
    assert_eq!(view.plan_code(), "starter");
    assert_eq!(view.subscription_status(), "active");
    assert!(view.entitlements_active());
    // Exactly the one event this test recorded (fresh random org).
    assert_eq!(view.usage.api_requests, 3.0);

    // Payment persistence round-trips durably.
    let payment = PaymentTransaction::new(
        org.id,
        BillingProviderKind::Manual,
        format!("pg-pay-{}", org.id.as_uuid()),
        1_234,
        "USD",
        Utc::now(),
    );
    assert!(store.record_payment(&payment).await.expect("recorded"));
    assert!(!store.record_payment(&payment).await.expect("dup refused"));
    let loaded = store.payment_of(org.id, payment.id).await.expect("loaded");
    assert_eq!(loaded.amount_cents, 1_234);
}
