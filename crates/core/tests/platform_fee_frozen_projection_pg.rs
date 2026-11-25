//! Tests linkage between immutable intent policies and fee projections.
//! These projections are not ledger postings or proof of a live fill.
mod trading_isolation_common;

use bot_core::billing::platform_fee::{FeeSurface, PlatformFeeCharge, PlatformFeePolicy};
use bot_core::models::ExecutionMode;
use bot_core::trading_repository::fee_policy_snapshot::{
    IntentFeePolicy, TenantIntentFeePolicyStore,
};
use bot_core::trading_repository::platform_fee::{FeeProjectionOutcome, TenantFeeProjectionStore};
use bot_core::trading_repository::RepositoryError;
use trading_isolation_common::{org, read_scope, run_id, setup, write_scope};

#[tokio::test]
async fn projections_require_matching_frozen_policy_and_replay_identically() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: platform_fee_frozen_projection_pg requires POSTGRES_URL");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("fee-link-a-{run}")).await;
    let b = org(&db, &format!("fee-link-b-{run}")).await;
    let intent = format!("intent-{run}");
    let policy = PlatformFeePolicy::new(100, Some(10_000), 0).unwrap();
    let snapshot = IntentFeePolicy {
        organization_id: a,
        intent_id: intent.clone(),
        surface: FeeSurface::Copy,
        mode: ExecutionMode::Paper,
        asset: "SOL".into(),
        policy,
    };
    let charge = PlatformFeeCharge::assess(
        format!("fee-{run}"),
        a.to_string(),
        "jupiter",
        "SOL",
        1_000_000,
        policy,
        "tenant-wallet",
        "platform-wallet",
    )
    .unwrap()
    .with_surface(FeeSurface::Copy);
    let store = TenantFeeProjectionStore::new(db.clone());
    assert!(matches!(
        store
            .record_for_intent(&write_scope(a), &intent, &charge, ExecutionMode::Paper)
            .await,
        Err(RepositoryError::NotFound(_))
    ));
    TenantIntentFeePolicyStore::new(db.clone())
        .freeze(&write_scope(a), &snapshot)
        .await
        .unwrap();
    assert!(matches!(
        store
            .record_for_intent(&write_scope(a), &intent, &charge, ExecutionMode::Live)
            .await,
        Err(RepositoryError::Conflict(_))
    ));
    let mut changed = charge.clone();
    changed.surface = Some(FeeSurface::TelegramSwap);
    assert!(matches!(
        store
            .record_for_intent(&write_scope(a), &intent, &changed, ExecutionMode::Paper)
            .await,
        Err(RepositoryError::Conflict(_))
    ));
    changed = charge.clone();
    changed.policy = PlatformFeePolicy::new(50, None, 0).unwrap();
    changed.fee_micros = 5_000;
    assert!(matches!(
        store
            .record_for_intent(&write_scope(a), &intent, &changed, ExecutionMode::Paper)
            .await,
        Err(RepositoryError::Conflict(_))
    ));
    assert!(matches!(
        store
            .record_for_intent(&write_scope(b), &intent, &charge, ExecutionMode::Paper)
            .await,
        Err(RepositoryError::TenantMismatch)
    ));

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let db = db.clone();
        let charge = charge.clone();
        let intent = intent.clone();
        tasks.push(tokio::spawn(async move {
            TenantFeeProjectionStore::new(db)
                .record_for_intent(&write_scope(a), &intent, &charge, ExecutionMode::Paper)
                .await
                .unwrap()
        }));
    }
    let (mut inserted, mut replay) = (0, 0);
    for task in tasks {
        match task.await.unwrap() {
            FeeProjectionOutcome::Inserted => inserted += 1,
            FeeProjectionOutcome::IdenticalReplay => replay += 1,
        }
    }
    assert_eq!((inserted, replay), (1, 7));
    assert!(matches!(
        store
            .record(&write_scope(a), &charge, ExecutionMode::Paper)
            .await,
        Err(RepositoryError::Conflict(_))
    ));
    let (stored_intent, entry, platform): (String, serde_json::Value, serde_json::Value) =
        sqlx::query_as("SELECT intent_id, tenant_entry, platform_projection FROM public.platform_fee_projections WHERE organization_id=$1 AND charge_id=$2")
        .bind(a.as_uuid()).bind(&charge.charge_id).fetch_one(db.pool()).await.unwrap();
    assert_eq!(stored_intent, intent);
    assert!(!entry.to_string().contains("\"cash\""));
    assert!(!platform.to_string().contains("\"cash\""));
    assert_eq!(
        store
            .totals_by_surface(&read_scope(a), ExecutionMode::Paper, "SOL")
            .await
            .unwrap(),
        vec![(FeeSurface::Copy, 10_000)]
    );
    assert!(store
        .totals_by_surface(&read_scope(b), ExecutionMode::Paper, "SOL")
        .await
        .unwrap()
        .is_empty());

    if let Ok(url) = std::env::var("POSTGRES_APP_URL") {
        let config = bot_core::config::DatabaseConfig {
            enabled: true,
            auto_migrate: false,
            ..Default::default()
        };
        let app_db = std::sync::Arc::new(
            bot_core::db::Database::connect(&config, &url)
                .await
                .unwrap(),
        );
        let app_store = TenantFeeProjectionStore::new(app_db);
        assert_eq!(
            app_store
                .record_for_intent(&write_scope(a), &intent, &charge, ExecutionMode::Paper)
                .await
                .unwrap(),
            FeeProjectionOutcome::IdenticalReplay
        );
        let mut fresh = charge.clone();
        fresh.charge_id = format!("app-fee-{run}");
        assert_eq!(
            app_store
                .record_for_intent(&write_scope(a), &intent, &fresh, ExecutionMode::Paper)
                .await
                .unwrap(),
            FeeProjectionOutcome::Inserted
        );
        let mut wrong = charge.clone();
        wrong.organization_id = b.to_string();
        wrong.charge_id = format!("foreign-{run}");
        assert!(matches!(
            app_store
                .record_for_intent(&write_scope(b), &intent, &wrong, ExecutionMode::Paper)
                .await,
            Err(RepositoryError::NotFound(_))
        ));
    } else {
        eprintln!("NOT_RUN: authenticated app-role check requires POSTGRES_APP_URL");
    }
}
