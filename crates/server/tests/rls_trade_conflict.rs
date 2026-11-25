//! Real app-role RLS coverage for tenant trade insert/retry/conflict.
//! CI supplies the app, system and DDL URLs and requires PostgreSQL.

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::{
    rls_context::{bind_request_tenant, scope_request_context},
    OrganizationId,
};
use bot_core::trading_repository::positions::{model::TenantTrade, TenantTradeRepo};
use bot_core::trading_repository::write_scope::{TenantWriteScope, WriteOrigin};
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn app_role_trade_retry_and_foreign_id_collision() {
    if std::env::var("REQUIRE_POSTGRES").as_deref() != Ok("1") {
        eprintln!("NOT_RUN: app_role_trade_retry_and_foreign_id_collision — REQUIRE_POSTGRES != 1");
        return;
    }
    let app_url = std::env::var("POSTGRES_APP_URL").expect("POSTGRES_APP_URL required");
    let ddl_url = std::env::var("POSTGRES_DDL_URL").expect("POSTGRES_DDL_URL required");
    let cfg = DatabaseConfig {
        enabled: true,
        url_env: "POSTGRES_APP_URL".into(),
        rls_enabled: true,
        auto_migrate: true,
        ..DatabaseConfig::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &app_url)
            .await
            .expect("connect app/system pools"),
    );
    let ddl = sqlx::PgPool::connect(&ddl_url).await.unwrap();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    for id in [a, b] {
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'trade rls test')")
            .bind(id)
            .bind(format!("trade-rls-{id}"))
            .execute(&ddl)
            .await
            .unwrap();
    }
    let trade = TenantTrade {
        organization_id: OrganizationId::from(a),
        id: format!("trade-rls-{}", Uuid::new_v4()),
        ts: chrono::Utc::now(),
        source: "sniper".into(),
        venue: "paper".into(),
        mode: "paper".into(),
        side: "long".into(),
        symbol: "RLS-A".into(),
        symbol_display: "RLS-A".into(),
        amount_in: 1.0,
        amount_out: 1.0,
        quote_symbol: "SOL".into(),
        price: 1.0,
        fee: 0.0,
        slippage_bps: 0,
        signature: None,
        position_id: None,
        note: None,
        latency_ms: None,
    };
    let repo = TenantTradeRepo::new(db.clone());
    scope_request_context(async {
        bind_request_tenant(a).unwrap();
        let role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(role, "bot_app");
        let scope =
            TenantWriteScope::new(OrganizationId::from(a), "user:test", WriteOrigin::Http).unwrap();
        repo.append(&scope, &trade)
            .await
            .expect("app role first insert");
        repo.append(&scope, &trade)
            .await
            .expect("app role exact retry");
        let mut altered = trade.clone();
        altered.symbol = "RLS-B".into();
        assert!(
            repo.append(&scope, &altered).await.is_err(),
            "distinct payload must not be accepted"
        );
    })
    .await;
    scope_request_context(async {
        bind_request_tenant(b).unwrap();
        let mut foreign = trade.clone();
        foreign.organization_id = OrganizationId::from(b);
        let scope =
            TenantWriteScope::new(OrganizationId::from(b), "user:test", WriteOrigin::Http).unwrap();
        assert!(
            repo.append(&scope, &foreign).await.is_err(),
            "foreign id collision must not return success"
        );
        let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM trades WHERE id=$1")
            .bind(&trade.id)
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(visible, 0, "foreign row not readable");
    })
    .await;
    let stored: (Uuid, String) =
        sqlx::query_as("SELECT organization_id,symbol FROM trades WHERE id=$1")
            .bind(&trade.id)
            .fetch_one(&ddl)
            .await
            .unwrap();
    assert_eq!(stored, (a, "RLS-A".into()));
    sqlx::query("DELETE FROM trades WHERE id=$1")
        .bind(&trade.id)
        .execute(&ddl)
        .await
        .unwrap();
    sqlx::query("DELETE FROM organizations WHERE id IN ($1,$2)")
        .bind(a)
        .bind(b)
        .execute(&ddl)
        .await
        .unwrap();
    ddl.close().await;
    db.close().await;
}

#[tokio::test]
async fn app_role_order_transition_and_foreign_collision() {
    if std::env::var("REQUIRE_POSTGRES").as_deref() != Ok("1") {
        eprintln!(
            "NOT_RUN: app_role_order_transition_and_foreign_collision — REQUIRE_POSTGRES != 1"
        );
        return;
    }
    use bot_core::trading_repository::orders::{TenantOrderRead, TenantOrderWrite};
    use bot_core::trading_repository::query_scope::TradingQueryScope;
    let app_url = std::env::var("POSTGRES_APP_URL").expect("POSTGRES_APP_URL required");
    let ddl_url = std::env::var("POSTGRES_DDL_URL").expect("POSTGRES_DDL_URL required");
    let cfg = DatabaseConfig {
        enabled: true,
        url_env: "POSTGRES_APP_URL".into(),
        rls_enabled: true,
        auto_migrate: true,
        ..DatabaseConfig::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &app_url)
            .await
            .expect("connect app/system pools"),
    );
    let ddl = sqlx::PgPool::connect(&ddl_url).await.unwrap();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    for id in [a, b] {
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1,$2,'order RLS test')")
            .bind(id)
            .bind(format!("order-rls-{id}"))
            .execute(&ddl)
            .await
            .unwrap();
    }
    let id = format!("order-rls-{}", Uuid::new_v4());
    sqlx::query("INSERT INTO orders (organization_id,id,module,side,symbol,venue,mode,status,qty) VALUES ($1,$2,'sniper','buy','SOL/USDC','paper','paper','created',1)")
        .bind(a).bind(&id).execute(&ddl).await.unwrap();
    let reader = TenantOrderRead::new(db.clone());
    let writer = TenantOrderWrite::new(db.clone());
    let candidate = scope_request_context(async {
        bind_request_tenant(a).unwrap();
        let role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(role, "bot_app");
        let write =
            TenantWriteScope::new(OrganizationId::from(a), "user:test", WriteOrigin::Http).unwrap();
        writer
            .set_status(
                &write,
                &id,
                "created",
                "submitted",
                None,
                None,
                None,
                None,
                chrono::Utc::now(),
            )
            .await
            .expect("tenant status transition");
        let visible: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM order_status_history WHERE order_id=$1 AND organization_id=$2",
        )
        .bind(&id)
        .bind(a)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(visible, 1);
        reader
            .get(&TradingQueryScope::new(OrganizationId::from(a)), &id)
            .await
            .unwrap()
    })
    .await;
    scope_request_context(async {
        bind_request_tenant(b).unwrap();
        let mut foreign = candidate.clone();
        foreign.organization_id = OrganizationId::from(b);
        foreign.status = "failed".into();
        let write =
            TenantWriteScope::new(OrganizationId::from(b), "user:test", WriteOrigin::Http).unwrap();
        assert!(
            writer.upsert(&write, &foreign).await.is_err(),
            "foreign ID must not report success"
        );
        let visible: i64 =
            sqlx::query_scalar("SELECT count(*) FROM order_status_history WHERE order_id=$1")
                .bind(&id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(visible, 0);
    })
    .await;
    let stored: (Uuid, String) = sqlx::query_as(
        "SELECT organization_id,to_status FROM order_status_history WHERE order_id=$1",
    )
    .bind(&id)
    .fetch_one(&ddl)
    .await
    .unwrap();
    assert_eq!(stored, (a, "submitted".into()));
    sqlx::query("DELETE FROM orders WHERE id=$1")
        .bind(&id)
        .execute(&ddl)
        .await
        .unwrap();
    sqlx::query("DELETE FROM organizations WHERE id IN ($1,$2)")
        .bind(a)
        .bind(b)
        .execute(&ddl)
        .await
        .unwrap();
    ddl.close().await;
    db.close().await;
}
