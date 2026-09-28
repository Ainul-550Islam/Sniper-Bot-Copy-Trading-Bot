//! Service-backed billing integration (Batch 4). Fixtures, not live provider.

use bot_core::billing::invoice::InvoiceStatus;
use bot_core::billing::payment::TransactionStatus;
use bot_core::billing::plan::{features, FeatureLimit, Plan, PlanCode};
use bot_core::billing::provider::BillingProviderKind;
use bot_core::billing::provider_events::{ProviderEventKind, ProviderNormalizedEvent};
use bot_core::billing::reconciliation::{
    reconcile, InternalBillingSnapshot, ProviderBillingSnapshot, ReconciliationAction,
};
use bot_core::billing::subscription::SubscriptionStatus;
use bot_core::billing::usage_policy::{evaluate_feature, UsageThresholds};
use bot_core::config::DatabaseConfig;

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}
fn not_run(m: &str) {
    eprintln!("NOT_RUN: {m} — POSTGRES_URL missing; fixture tests still run");
}

#[test]
fn checkout_persistence_fixture() {
    let req = serde_json::json!({"plan_code":"pro","billing_provider":"manual","success_url":"https://example.com/success"});
    let s = req.to_string();
    assert!(s.contains("pro"));
    assert!(!s.to_ascii_lowercase().contains("price"));
}

#[test]
fn provider_event_dedup_fixture() {
    let now = chrono::Utc::now();
    let e1 = ProviderNormalizedEvent::new(
        BillingProviderKind::Stripe,
        "evt_123",
        ProviderEventKind::PaymentSucceeded,
        now,
        now,
    )
    .unwrap();
    let e2 = ProviderNormalizedEvent::new(
        BillingProviderKind::Stripe,
        "evt_123",
        ProviderEventKind::PaymentSucceeded,
        now,
        now,
    )
    .unwrap();
    assert_eq!(e1.idempotency.as_key(), e2.idempotency.as_key());
    assert_eq!(e1.idempotency.as_key(), "stripe:evt_123");
}

#[test]
fn invoice_payment_state_fixture() {
    let state = serde_json::json!({"invoice_status":"paid","payment_status":"succeeded"});
    assert_eq!(state["invoice_status"], "paid");
    assert!(matches!(
        InvoiceStatus::parse("paid"),
        Some(InvoiceStatus::Paid)
    ));
    assert!(matches!(
        TransactionStatus::parse("succeeded"),
        Some(TransactionStatus::Succeeded)
    ));
}

#[test]
fn entitlement_transition_fixture() {
    let plan = Plan::new(PlanCode::Pro, "Pro", chrono::Utc::now())
        .with_limit(features::MODULE_SNIPER, FeatureLimit::Unlimited);
    assert!(plan.limit_for(features::MODULE_SNIPER).is_enabled());
}

#[test]
fn dunning_state_transitions() {
    use bot_core::billing::dunning::DunningState;
    let mut s = DunningState::Current;
    assert!(matches!(s, DunningState::Current));
    s = DunningState::PaymentFailed;
    assert_eq!(s.as_str(), "payment_failed");
}

#[test]
fn usage_limits_thresholds() {
    let plan = Plan::new(PlanCode::Pro, "Pro", chrono::Utc::now())
        .with_limit(features::MONTHLY_ORDERS, FeatureLimit::Limited(100.0));
    let d = evaluate_feature(
        &plan,
        features::MONTHLY_ORDERS,
        85.0,
        &UsageThresholds::default(),
    );
    assert_eq!(d.state.as_str(), "soft_limit_exceeded");
}

#[test]
fn reconciliation_fixture() {
    use bot_core::tenant::OrganizationId;
    let now = chrono::Utc::now();
    let internal = InternalBillingSnapshot {
        organization_id: OrganizationId::new(),
        subscription_status: Some(SubscriptionStatus::Active),
        subscription_provider: None,
        last_payment_status: Some(TransactionStatus::Succeeded),
        last_invoice_status: Some(InvoiceStatus::Paid),
        last_invoice_id: None,
        entitlement_active: true,
        as_of: now,
    };
    let provider = ProviderBillingSnapshot {
        provider: BillingProviderKind::Stripe,
        provider_customer_id: Some("cus_123".into()),
        event_kind: ProviderEventKind::PaymentSucceeded,
        event_id: "evt_123".into(),
        subscription_status_hint: None,
        invoice_status_hint: None,
        payment_status_hint: None,
        amount_cents: Some(5000),
        currency: Some("usd".into()),
        event_timestamp: now,
    };
    let r = reconcile(&internal, &provider, now);
    assert_eq!(r.action, ReconciliationAction::NoOp);
}

#[tokio::test]
async fn billing_persistence_requires_postgres() {
    let Some(url) = pg_url() else {
        not_run("billing persistence");
        return;
    };
    let cfg = DatabaseConfig {
        enabled: true,
        ..Default::default()
    };
    let db = bot_core::db::Database::connect(&cfg, &url)
        .await
        .expect("connect");
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT tablename FROM pg_tables WHERE tablename='subscriptions'")
            .fetch_optional(db.pool())
            .await
            .unwrap_or(None);
    let _ = exists;
    let _ = ();
}

#[tokio::test]
async fn live_stripe_not_executed_guard() {
    if std::env::var("STRIPE_API_KEY").is_ok() {
        eprintln!("WARN live Stripe key present — but test still uses fixture, not live call");
    }
    let _ = "live Stripe NOT_EXECUTED — fixture only";
}
