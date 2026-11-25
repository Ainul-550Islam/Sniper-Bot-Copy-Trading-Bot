//! Real Postgres tests for intent policy snapshots, not execution/settlement.
mod trading_isolation_common;

use bot_core::billing::platform_fee::{FeeSurface, PlatformFeePolicy};
use bot_core::models::ExecutionMode;
use bot_core::trading_repository::fee_policy_snapshot::{
    FreezeOutcome, IntentFeePolicy, TenantIntentFeePolicyStore,
};
use bot_core::trading_repository::RepositoryError;
use trading_isolation_common::{org, read_scope, run_id, setup, write_scope};

#[tokio::test]
async fn policy_is_immutable_idempotent_and_tenant_isolated() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: platform_fee_intent_policy_pg requires POSTGRES_URL");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("fee-policy-a-{run}")).await;
    let b = org(&db, &format!("fee-policy-b-{run}")).await;
    let store = TenantIntentFeePolicyStore::new(db.clone());
    let snapshot = IntentFeePolicy {
        organization_id: a,
        intent_id: format!("intent-{run}"),
        surface: FeeSurface::Copy,
        mode: ExecutionMode::Live,
        asset: "SOL".into(),
        policy: PlatformFeePolicy::new(30, Some(1000), 10).unwrap(),
    };
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let db = db.clone();
        let snapshot = snapshot.clone();
        tasks.push(tokio::spawn(async move {
            TenantIntentFeePolicyStore::new(db)
                .freeze(&write_scope(a), &snapshot)
                .await
                .unwrap()
        }));
    }
    let mut inserted = 0;
    let mut replayed = 0;
    for task in tasks {
        match task.await.unwrap() {
            FreezeOutcome::Inserted => inserted += 1,
            FreezeOutcome::IdenticalReplay => replayed += 1,
        }
    }
    assert_eq!((inserted, replayed), (1, 7));

    // Competing policy snapshots cannot both win the same intent. All
    // identical followers of the winner replay; the other policy conflicts.
    let race_id = format!("race-{run}");
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(8));
    let mut contenders = Vec::new();
    for index in 0..8 {
        let db = db.clone();
        let barrier = barrier.clone();
        let mut contender = snapshot.clone();
        contender.intent_id = race_id.clone();
        contender.policy.fee_bps = if index % 2 == 0 { 20 } else { 30 };
        contenders.push(tokio::spawn(async move {
            barrier.wait().await;
            TenantIntentFeePolicyStore::new(db)
                .freeze(&write_scope(a), &contender)
                .await
        }));
    }
    let (mut wins, mut identical, mut conflicts) = (0, 0, 0);
    for contender in contenders {
        match contender.await.unwrap() {
            Ok(FreezeOutcome::Inserted) => wins += 1,
            Ok(FreezeOutcome::IdenticalReplay) => identical += 1,
            Err(RepositoryError::Conflict(_)) => conflicts += 1,
            other => panic!("unexpected policy race result: {other:?}"),
        }
    }
    assert_eq!((wins, identical, conflicts), (1, 3, 4));
    assert_eq!(
        store
            .get(&read_scope(a), &snapshot.intent_id)
            .await
            .unwrap(),
        Some(snapshot.clone())
    );
    assert_eq!(
        store
            .get(&read_scope(b), &snapshot.intent_id)
            .await
            .unwrap(),
        None
    );
    assert!(matches!(
        store.freeze(&write_scope(b), &snapshot).await,
        Err(RepositoryError::TenantMismatch)
    ));
    for changed in [
        IntentFeePolicy {
            policy: PlatformFeePolicy::new(20, None, 0).unwrap(),
            ..snapshot.clone()
        },
        IntentFeePolicy {
            mode: ExecutionMode::Paper,
            ..snapshot.clone()
        },
        IntentFeePolicy {
            surface: FeeSurface::TelegramSwap,
            ..snapshot.clone()
        },
        IntentFeePolicy {
            asset: "USDC".into(),
            ..snapshot.clone()
        },
    ] {
        assert!(matches!(
            store.freeze(&write_scope(a), &changed).await,
            Err(RepositoryError::Conflict(_))
        ));
    }
    let invalid = IntentFeePolicy {
        policy: PlatformFeePolicy {
            fee_bps: 501,
            cap_micros: None,
            minimum_micros: 0,
        },
        ..snapshot.clone()
    };
    assert!(matches!(
        store.freeze(&write_scope(a), &invalid).await,
        Err(RepositoryError::Validation(_))
    ));
    let disabled = IntentFeePolicy {
        organization_id: b,
        policy: PlatformFeePolicy::new(0, None, 0).unwrap(),
        ..snapshot.clone()
    };
    assert_eq!(
        store.freeze(&write_scope(b), &disabled).await.unwrap(),
        FreezeOutcome::Inserted
    );
    assert_eq!(
        store
            .get(&read_scope(b), &disabled.intent_id)
            .await
            .unwrap(),
        Some(disabled)
    );

    // Exercise forced RLS and append-only permissions as the real app role.
    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("SET LOCAL ROLE bot_app")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(a.to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM public.platform_fee_intent_policy WHERE intent_id = $1",
    )
    .bind(&snapshot.intent_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let update = sqlx::query("UPDATE public.platform_fee_intent_policy SET fee_bps=0 WHERE organization_id=$1 AND intent_id=$2")
        .bind(a.as_uuid()).bind(&snapshot.intent_id).execute(&mut *tx).await;
    assert!(
        update.is_err(),
        "application role must not rewrite frozen policy"
    );
    tx.rollback().await.unwrap();

    // When configured, run the repository itself through a separately
    // authenticated application-role pool, rather than the owner pool.
    if let Ok(url) = std::env::var("POSTGRES_APP_URL") {
        let config = bot_core::config::DatabaseConfig {
            enabled: true,
            auto_migrate: false,
            ..Default::default()
        };
        let app_db = std::sync::Arc::new(
            bot_core::db::Database::connect(&config, &url)
                .await
                .expect("application role connection"),
        );
        let app_store = TenantIntentFeePolicyStore::new(app_db);
        assert_eq!(
            app_store
                .get(&read_scope(a), &snapshot.intent_id)
                .await
                .unwrap(),
            Some(snapshot.clone())
        );
        let app_new = IntentFeePolicy {
            intent_id: format!("app-{run}"),
            mode: ExecutionMode::Paper,
            ..snapshot.clone()
        };
        assert_eq!(
            app_store.freeze(&write_scope(a), &app_new).await.unwrap(),
            FreezeOutcome::Inserted
        );
        assert_eq!(
            app_store
                .get(&read_scope(b), &app_new.intent_id)
                .await
                .unwrap(),
            None
        );
    } else {
        eprintln!("NOT_RUN: authenticated app-role repository check requires POSTGRES_APP_URL");
    }
}
