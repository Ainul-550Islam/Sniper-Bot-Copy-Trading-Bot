//! Real-Postgres fee projection journal regression. No live fee settlement.
mod trading_isolation_common;
use bot_core::billing::platform_fee::{FeeSurface, PlatformFeeCharge, PlatformFeePolicy};
use bot_core::models::ExecutionMode;
use bot_core::trading_repository::platform_fee::{FeeProjectionOutcome, TenantFeeProjectionStore};
use bot_core::trading_repository::RepositoryError;
use trading_isolation_common::{org, run_id, setup, write_scope};

#[tokio::test]
async fn projection_is_tenant_scoped_atomic_and_replay_safe() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: platform_fee_projection_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("fee-a-{run}")).await;
    let b = org(&db, &format!("fee-b-{run}")).await;
    let store = TenantFeeProjectionStore::new(db.clone());
    let id = format!("intent-{run}");
    let charge = PlatformFeeCharge::assess(
        &id,
        a.to_string(),
        "jupiter",
        "SOL",
        1_000_000,
        PlatformFeePolicy::new(100, None, 0).unwrap(),
        "tenant-wallet",
        "platform-wallet",
    )
    .unwrap()
    .with_surface(FeeSurface::Copy);
    assert_eq!(
        store
            .record(&write_scope(a), &charge, ExecutionMode::Paper)
            .await
            .unwrap(),
        FeeProjectionOutcome::Inserted
    );
    assert_eq!(
        store
            .record(&write_scope(a), &charge, ExecutionMode::Paper)
            .await
            .unwrap(),
        FeeProjectionOutcome::IdenticalReplay
    );
    assert!(matches!(
        store
            .record(&write_scope(b), &charge, ExecutionMode::Paper)
            .await,
        Err(RepositoryError::TenantMismatch)
    ));
    assert!(matches!(
        store
            .record(&write_scope(a), &charge, ExecutionMode::Live)
            .await,
        Err(RepositoryError::Conflict(_))
    ));
    let mut changed = charge.clone();
    changed.notional_micros *= 2;
    changed.fee_micros *= 2;
    assert!(matches!(
        store
            .record(&write_scope(a), &changed, ExecutionMode::Paper)
            .await,
        Err(RepositoryError::Conflict(_))
    ));
    let mut other = charge.clone();
    other.organization_id = b.to_string();
    assert_eq!(
        store
            .record(&write_scope(b), &other, ExecutionMode::Paper)
            .await
            .unwrap(),
        FeeProjectionOutcome::Inserted
    );
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM public.platform_fee_projections WHERE charge_id = $1")
            .bind(&id)
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(count, 2);
    let (entry, revenue): (serde_json::Value, serde_json::Value) = sqlx::query_as("SELECT tenant_entry, platform_projection FROM public.platform_fee_projections WHERE organization_id = $1 AND charge_id = $2").bind(a.as_uuid()).bind(&id).fetch_one(db.pool()).await.unwrap();
    assert!(!entry.to_string().contains("\"cash\""));
    assert!(!revenue.to_string().contains("\"cash\""));
    // RLS: the application role can only see its transaction-local tenant.
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
    let (visible,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM public.platform_fee_projections WHERE charge_id = $1")
            .bind(&id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(visible, 1, "app role must not see the other tenant");
    tx.rollback().await.unwrap();

    // Eight simultaneous identical writes: one inserts and seven replay.
    let mut concurrent = charge.clone();
    concurrent.charge_id = format!("concurrent-{run}");
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let db = db.clone();
        let charge = concurrent.clone();
        tasks.push(tokio::spawn(async move {
            TenantFeeProjectionStore::new(db)
                .record(&write_scope(a), &charge, ExecutionMode::Live)
                .await
                .unwrap()
        }));
    }
    let mut inserted = 0;
    let mut replayed = 0;
    for task in tasks {
        match task.await.unwrap() {
            FeeProjectionOutcome::Inserted => inserted += 1,
            FeeProjectionOutcome::IdenticalReplay => replayed += 1,
        }
    }
    assert_eq!((inserted, replayed), (1, 7));
    // Projection totals are grouped by surface, mode, asset and tenant.
    for (index, surface) in [
        FeeSurface::Sniper,
        FeeSurface::TelegramSwap,
        FeeSurface::Polymarket,
    ]
    .into_iter()
    .enumerate()
    {
        let mut item = charge.clone();
        item.charge_id = format!("surface-{run}-{index}");
        item.surface = Some(surface);
        assert_eq!(
            store
                .record(&write_scope(a), &item, ExecutionMode::Paper)
                .await
                .unwrap(),
            FeeProjectionOutcome::Inserted
        );
    }
    let totals = store
        .totals_by_surface(
            &trading_isolation_common::read_scope(a),
            ExecutionMode::Paper,
            "SOL",
        )
        .await
        .unwrap();
    assert_eq!(totals.len(), 4);
    assert!(totals.iter().all(|(_, amount)| *amount == 10_000));
    let foreign = store
        .totals_by_surface(
            &trading_isolation_common::read_scope(b),
            ExecutionMode::Paper,
            "SOL",
        )
        .await
        .unwrap();
    assert_eq!(foreign, vec![(FeeSurface::Copy, 10_000)]);
    assert!(store
        .totals_by_surface(
            &trading_isolation_common::read_scope(a),
            ExecutionMode::Paper,
            "USDC"
        )
        .await
        .unwrap()
        .is_empty());
}
