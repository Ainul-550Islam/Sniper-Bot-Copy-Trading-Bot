//! Real PostgreSQL immutable fill replay and tenant isolation regression.
mod trading_isolation_common;
use bot_core::trading_repository::polymarket::{
    model::TenantPolyFill, TenantPolyRead, TenantPolyWrite,
};
use bot_core::trading_repository::RepositoryError;
use chrono::Utc;
use std::sync::Arc;
use trading_isolation_common::{org, read_scope, run_id, setup, write_scope};

#[tokio::test]
async fn fill_replays_are_checked_and_conflicting_economics_rejected() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let a = org(&db, &format!("poly-fill-a-{run}")).await;
    let b = org(&db, &format!("poly-fill-b-{run}")).await;
    let writes = Arc::new(TenantPolyWrite::new(db.clone()));
    let reads = TenantPolyRead::new(db.clone());
    let fill = TenantPolyFill {
        organization_id: a,
        fill_id: format!("fill-{run}"),
        venue_order_id: "venue-order".into(),
        order_id: "oms-order".into(),
        token_id: "outcome-token".into(),
        side: "buy".into(),
        price: 0.5,
        size_tokens: 3.0,
        quote_usd: 1.5,
        source: "recon".into(),
        position_id: None,
        ts: Utc::now(),
    };
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let w = writes.clone();
        let item = fill.clone();
        tasks.push(tokio::spawn(async move {
            w.record_fill(&write_scope(a), &item).await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let mut replay = fill.clone();
    replay.source = "user_ws".into();
    replay.ts = Utc::now();
    writes.record_fill(&write_scope(a), &replay).await.unwrap();
    let mut altered = fill.clone();
    altered.size_tokens = 4.0;
    altered.quote_usd = 2.0;
    assert!(matches!(
        writes.record_fill(&write_scope(a), &altered).await,
        Err(RepositoryError::Conflict("poly_fill"))
    ));
    let mut altered = fill.clone();
    altered.token_id = "other-token".into();
    assert!(matches!(
        writes.record_fill(&write_scope(a), &altered).await,
        Err(RepositoryError::Conflict("poly_fill"))
    ));
    let mut invalid = fill.clone();
    invalid.fill_id = format!("invalid-{run}");
    invalid.price = f64::NAN;
    assert!(matches!(
        writes.record_fill(&write_scope(a), &invalid).await,
        Err(RepositoryError::Validation(_))
    ));
    let mut other = fill.clone();
    other.organization_id = b;
    other.size_tokens = 4.0;
    other.quote_usd = 2.0;
    writes.record_fill(&write_scope(b), &other).await.unwrap();
    assert_eq!(
        reads
            .fill(&read_scope(a), &fill.fill_id)
            .await
            .unwrap()
            .unwrap()
            .size_tokens,
        3.0
    );
    assert_eq!(
        reads
            .fill(&read_scope(b), &fill.fill_id)
            .await
            .unwrap()
            .unwrap()
            .size_tokens,
        4.0
    );
}

#[tokio::test]
async fn app_role_retry_and_conflict_remain_tenant_local() {
    use bot_core::config::DatabaseConfig;
    use bot_core::db::Database;
    use bot_core::tenant::rls_context::{bind_request_tenant, scope_request_context};
    if std::env::var("REQUIRE_POSTGRES").as_deref() != Ok("1") {
        eprintln!("NOT_RUN: app role test requires strict PostgreSQL credentials");
        return;
    }
    let app_url = std::env::var("POSTGRES_APP_URL").expect("POSTGRES_APP_URL required");
    let ddl_url = std::env::var("POSTGRES_DDL_URL").expect("POSTGRES_DDL_URL required");
    let cfg = DatabaseConfig {
        enabled: true,
        url_env: "POSTGRES_APP_URL".into(),
        rls_enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Arc::new(Database::connect(&cfg, &app_url).await.unwrap());
    let ddl = sqlx::PgPool::connect(&ddl_url).await.unwrap();
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    for id in [a, b] {
        sqlx::query("INSERT INTO organizations (id,slug,name) VALUES ($1,$2,'fill rls')")
            .bind(id)
            .bind(format!("fill-rls-{id}"))
            .execute(&ddl)
            .await
            .unwrap();
    }
    let a = bot_core::tenant::OrganizationId::from(a);
    let b = bot_core::tenant::OrganizationId::from(b);
    let fill = TenantPolyFill {
        organization_id: a,
        fill_id: format!("fill-rls-{}", uuid::Uuid::new_v4()),
        venue_order_id: "venue".into(),
        order_id: "order".into(),
        token_id: "token".into(),
        side: "buy".into(),
        price: 0.5,
        size_tokens: 2.0,
        quote_usd: 1.0,
        source: "recon".into(),
        position_id: None,
        ts: Utc::now(),
    };
    let repo = TenantPolyWrite::new(db.clone());
    scope_request_context(async {
        bind_request_tenant(a.as_uuid()).unwrap();
        let role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(role, "bot_app");
        repo.record_fill(&write_scope(a), &fill).await.unwrap();
        repo.record_fill(&write_scope(a), &fill).await.unwrap();
        let mut changed = fill.clone();
        changed.size_tokens = 3.0;
        changed.quote_usd = 1.5;
        assert!(matches!(
            repo.record_fill(&write_scope(a), &changed).await,
            Err(RepositoryError::Conflict("poly_fill"))
        ));
    })
    .await;
    scope_request_context(async {
        bind_request_tenant(b.as_uuid()).unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM poly_fills WHERE fill_id=$1")
            .bind(&fill.fill_id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
        let mut other = fill.clone();
        other.organization_id = b;
        repo.record_fill(&write_scope(b), &other).await.unwrap();
    })
    .await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM poly_fills WHERE fill_id=$1")
        .bind(&fill.fill_id)
        .fetch_one(&ddl)
        .await
        .unwrap();
    assert_eq!(n, 2);
    sqlx::query("DELETE FROM poly_fills WHERE fill_id=$1")
        .bind(&fill.fill_id)
        .execute(&ddl)
        .await
        .unwrap();
    sqlx::query("DELETE FROM organizations WHERE id IN ($1,$2)")
        .bind(a.as_uuid())
        .bind(b.as_uuid())
        .execute(&ddl)
        .await
        .unwrap();
    ddl.close().await;
    db.close().await;
}
