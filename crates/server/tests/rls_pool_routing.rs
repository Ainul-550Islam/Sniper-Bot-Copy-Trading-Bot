use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::rls_context::{bind_request_tenant, scope_request_context};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn required_env(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("{name} is required for the RLS pool-routing integration test"))
}

#[tokio::test]
async fn application_and_system_pools_enforce_separate_rls_roles() {
    assert_eq!(
        std::env::var("REQUIRE_POSTGRES").as_deref(),
        Ok("1"),
        "the CI gate must require the real PostgreSQL service"
    );
    let app_url = required_env("POSTGRES_APP_URL");
    let system_url = required_env("POSTGRES_SYSTEM_URL");
    let ddl_url = required_env("POSTGRES_DDL_URL");
    let cfg = DatabaseConfig {
        enabled: true,
        url_env: "POSTGRES_APP_URL".into(),
        rls_enabled: true,
        auto_migrate: true,
        ..DatabaseConfig::default()
    };
    let db = Database::connect(&cfg, &app_url)
        .await
        .expect("connect distinct app/system/migration pools");
    let system_pool = db
        .system_pool()
        .expect("separate system pool is configured");
    assert_ne!(
        app_url, system_url,
        "app and system credentials/URLs must be distinct"
    );

    let ddl_pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&ddl_url)
        .await
        .expect("connect DDL setup pool");
    let organization_a = Uuid::new_v4();
    let organization_b = Uuid::new_v4();
    let order_a = format!("rls-pool-a-{}", Uuid::new_v4());
    let order_b = format!("rls-pool-b-{}", Uuid::new_v4());
    for (organization_id, order_id) in [(organization_a, &order_a), (organization_b, &order_b)] {
        sqlx::query(
            "INSERT INTO organizations (id, slug, name) VALUES ($1, $2, 'pool route test')",
        )
        .bind(organization_id)
        .bind(format!("rls-pool-{}", organization_id.simple()))
        .execute(&ddl_pool)
        .await
        .expect("seed tenant organization through DDL pool");
        sqlx::query(
            "INSERT INTO orders (id, organization_id, module, side, symbol, venue, mode)
             VALUES ($1, $2, 'sniper', 'buy', 'RLS', 'test', 'paper')",
        )
        .bind(order_id)
        .bind(organization_id)
        .execute(&ddl_pool)
        .await
        .expect("seed tenant order through DDL pool");
    }

    scope_request_context(async {
        assert_eq!(
            bind_request_tenant(organization_a),
            Ok(()),
            "bind authenticated tenant before tenant queries"
        );
        let role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(db.pool())
            .await
            .expect("checkout non-bypass app pool connection");
        assert_eq!(role, "bot_app");
        let visible: Vec<String> =
            sqlx::query_scalar("SELECT id FROM orders WHERE id = ANY($1) ORDER BY id")
                .bind(vec![order_a.clone(), order_b.clone()])
                .fetch_all(db.pool())
                .await
                .expect("tenant-scoped app read");
        assert_eq!(visible, vec![order_a.clone()]);

        let system_role: String = sqlx::query_scalar("SELECT current_user::text")
            .fetch_one(system_pool)
            .await
            .expect("checkout separate system pool connection");
        assert_eq!(system_role, "bot_system");
        let system_visible: i64 =
            sqlx::query_scalar("SELECT count(*) FROM orders WHERE id = ANY($1)")
                .bind(vec![order_a.clone(), order_b.clone()])
                .fetch_one(system_pool)
                .await
                .expect("system pool can perform trusted cross-tenant reads");
        assert_eq!(system_visible, 2);
    })
    .await;

    scope_request_context(async {
        let no_tenant_rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM orders WHERE id = ANY($1)")
                .bind(vec![order_a.clone(), order_b.clone()])
                .fetch_one(db.pool())
                .await
                .expect("an unbound app request must remain fail-closed");
        assert_eq!(no_tenant_rows, 0);
    })
    .await;

    sqlx::query("DELETE FROM orders WHERE id = ANY($1)")
        .bind(vec![order_a, order_b])
        .execute(&ddl_pool)
        .await
        .expect("remove isolated pool-routing orders");
    sqlx::query("DELETE FROM organizations WHERE id IN ($1, $2)")
        .bind(organization_a)
        .bind(organization_b)
        .execute(&ddl_pool)
        .await
        .expect("remove isolated pool-routing organizations");
    ddl_pool.close().await;
    db.close().await;
}
