use bot_core::tenant::rls_context::{SystemScope, TenantScope};
use sqlx::postgres::PgPoolOptions;
use sqlx::Connection;
use uuid::Uuid;

struct TenantRowCase {
    table: &'static str,
    id_a: String,
    id_b: String,
    attempted_id: String,
    insert_sql: &'static str,
    extra_uuid: Option<Uuid>,
}

fn postgres_url() -> String {
    std::env::var("POSTGRES_URL")
        .expect("POSTGRES_URL is required for the PostgreSQL RLS isolation test")
}

async fn insert_strategy(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
    id: Uuid,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO tenant_strategies (id, organization_id, name, module)
         VALUES ($1, $2, $3, 'sniper')",
    )
    .bind(id)
    .bind(organization_id)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}

async fn seed_audit_row(
    pool: &sqlx::PgPool,
    organization_id: Uuid,
    id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO audit_events (id, organization_id, actor, action, outcome, prev_hash, hash)
         VALUES ($1, $2, 'rls-test', 'test', 'success', 'previous', 'event')",
    )
    .bind(id)
    .bind(organization_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn assert_cross_tenant_insert_is_rejected(
    pool: &sqlx::PgPool,
    organization_a: Uuid,
    organization_b: Uuid,
    case: &TenantRowCase,
) {
    let mut connection = pool
        .acquire()
        .await
        .expect("acquire insert test connection");
    let transaction = connection
        .begin()
        .await
        .expect("begin cross-tenant insert check");
    let mut scope = TenantScope::from_transaction(transaction, organization_a)
        .await
        .expect("install tenant A context for insert check");
    let mut query = sqlx::query(case.insert_sql)
        .bind(&case.attempted_id)
        .bind(organization_b.to_string());
    if let Some(strategy_id) = case.extra_uuid {
        query = query.bind(strategy_id);
    }
    let result = query.execute(&mut **scope.transaction()).await;
    assert!(
        result.is_err(),
        "RLS must reject a tenant-B INSERT into {} while scoped to tenant A",
        case.table
    );
    scope
        .rollback()
        .await
        .expect("rollback rejected cross-tenant insert");
}

#[tokio::test]
async fn bot_app_tenant_scope_isolates_trading_control_and_audit_tables() {
    if std::env::var("REQUIRE_POSTGRES").ok().as_deref() == Some("1") {
        assert!(
            std::env::var_os("POSTGRES_URL").is_some(),
            "REQUIRE_POSTGRES=1 requires POSTGRES_URL"
        );
    }
    let pool = PgPoolOptions::new()
        .max_connections(6)
        .connect(&postgres_url())
        .await
        .expect("connect to PostgreSQL");
    bot_core::db::MIGRATOR
        .run(&pool)
        .await
        .expect("apply migrations");

    let org_a = Uuid::new_v4();
    let org_b = Uuid::new_v4();
    let strategy_a = Uuid::new_v4();
    let strategy_b = Uuid::new_v4();
    for (id, name) in [(org_a, "RLS test tenant A"), (org_b, "RLS test tenant B")] {
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("rls-{}", id.simple()))
            .bind(name)
            .execute(&pool)
            .await
            .expect("seed organization via system connection");
    }
    insert_strategy(&pool, org_a, strategy_a, "Tenant A strategy")
        .await
        .expect("seed A strategy via system connection");
    insert_strategy(&pool, org_b, strategy_b, "Tenant B strategy")
        .await
        .expect("seed B strategy via system connection");

    let order_a = format!("order-a-{}", Uuid::new_v4());
    let order_b = format!("order-b-{}", Uuid::new_v4());
    for (id, org) in [(&order_a, org_a), (&order_b, org_b)] {
        sqlx::query(
            "INSERT INTO orders (id, organization_id, module, side, symbol, venue, mode)
             VALUES ($1, $2, 'sniper', 'buy', 'RLS', 'test', 'paper')",
        )
        .bind(id)
        .bind(org)
        .execute(&pool)
        .await
        .expect("seed tenant order");
    }

    let position_a = format!("position-a-{}", Uuid::new_v4());
    let position_b = format!("position-b-{}", Uuid::new_v4());
    for (id, org) in [(&position_a, org_a), (&position_b, org_b)] {
        sqlx::query(
            "INSERT INTO positions (id, organization_id, source, venue, mode, symbol, opened_at)
             VALUES ($1, $2, 'manual', 'test', 'paper', 'RLS', now())",
        )
        .bind(id)
        .bind(org)
        .execute(&pool)
        .await
        .expect("seed tenant position");
    }

    let wallet_pool_a = Uuid::new_v4();
    let wallet_pool_b = Uuid::new_v4();
    for (id, org, name) in [
        (wallet_pool_a, org_a, "RLS pool A"),
        (wallet_pool_b, org_b, "RLS pool B"),
    ] {
        sqlx::query("INSERT INTO wallet_pools (id, organization_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(org)
            .bind(name)
            .execute(&pool)
            .await
            .expect("seed wallet pool");
    }

    let webhook_a = Uuid::new_v4();
    let webhook_b = Uuid::new_v4();
    for (id, org) in [(webhook_a, org_a), (webhook_b, org_b)] {
        sqlx::query(
            "INSERT INTO webhook_endpoints (id, organization_id, url, secret)
             VALUES ($1, $2, 'https://example.invalid/rls-test', 'non-secret-test-value')",
        )
        .bind(id)
        .bind(org)
        .execute(&pool)
        .await
        .expect("seed webhook endpoint");
    }

    let api_key_a = Uuid::new_v4();
    let api_key_b = Uuid::new_v4();
    for (id, org, suffix) in [(api_key_a, org_a, "a"), (api_key_b, org_b, "b")] {
        sqlx::query(
            "INSERT INTO saas_api_keys (id, organization_id, key_prefix, secret_hash, role)
             VALUES ($1, $2, $3, $4, 'viewer')",
        )
        .bind(id)
        .bind(org)
        .bind(format!("rls-key-{suffix}-{}", id.simple()))
        .bind(format!("hash-{suffix}-{}", id.simple()))
        .execute(&pool)
        .await
        .expect("seed tenant API key metadata");
    }

    let dca_a = format!("dca-a-{}", Uuid::new_v4());
    let dca_b = format!("dca-b-{}", Uuid::new_v4());
    for (id, org) in [(&dca_a, org_a), (&dca_b, org_b)] {
        sqlx::query(
            "INSERT INTO dca_schedules
                (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, next_run_at)
             VALUES ($1, $2, 'RLS', 0.1, 60, 1, now())",
        )
        .bind(id)
        .bind(org)
        .execute(&pool)
        .await
        .expect("seed DCA schedule");
    }

    let limit_order_a = format!("limit-a-{}", Uuid::new_v4());
    let limit_order_b = format!("limit-b-{}", Uuid::new_v4());
    for (id, org) in [(&limit_order_a, org_a), (&limit_order_b, org_b)] {
        sqlx::query(
            "INSERT INTO limit_orders
                (id, organization_id, mint, side, trigger_kind, price_sol, amount)
             VALUES ($1, $2, 'RLS', 'buy', 'at_or_below', 1, 1)",
        )
        .bind(id)
        .bind(org)
        .execute(&pool)
        .await
        .expect("seed limit order");
    }

    let backtest_a = Uuid::new_v4();
    let backtest_b = Uuid::new_v4();
    for (id, org, strategy) in [
        (backtest_a, org_a, strategy_a),
        (backtest_b, org_b, strategy_b),
    ] {
        sqlx::query(
            "INSERT INTO backtest_runs
                (id, organization_id, strategy_id, venue, period_start, period_end,
                 initial_balance_usd_cents, status, data_source, authoritative)
             VALUES ($1, $2, $3, 'test', now() - interval '1 day', now(), 10000,
                     'queued', 'synthetic', false)",
        )
        .bind(id)
        .bind(org)
        .bind(strategy)
        .execute(&pool)
        .await
        .expect("seed backtest run");
    }

    let audit_a = -((Uuid::new_v4().as_u128() % (i64::MAX as u128)) as i64);
    let audit_b = audit_a.saturating_sub(1);
    seed_audit_row(&pool, org_a, audit_a)
        .await
        .expect("seed tenant A audit event");
    seed_audit_row(&pool, org_b, audit_b)
        .await
        .expect("seed tenant B audit event");

    let cases = [
        TenantRowCase {
            table: "orders",
            id_a: order_a.clone(),
            id_b: order_b.clone(),
            attempted_id: format!("order-new-{}", Uuid::new_v4()),
            insert_sql: "INSERT INTO orders (id, organization_id, module, side, symbol, venue, mode) VALUES ($1::text, $2::uuid, 'sniper', 'buy', 'RLS', 'test', 'paper')",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "positions",
            id_a: position_a.clone(),
            id_b: position_b.clone(),
            attempted_id: format!("position-new-{}", Uuid::new_v4()),
            insert_sql: "INSERT INTO positions (id, organization_id, source, venue, mode, symbol, opened_at) VALUES ($1::text, $2::uuid, 'manual', 'test', 'paper', 'RLS', now())",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "wallet_pools",
            id_a: wallet_pool_a.to_string(),
            id_b: wallet_pool_b.to_string(),
            attempted_id: Uuid::new_v4().to_string(),
            insert_sql: "INSERT INTO wallet_pools (id, organization_id, name) VALUES ($1::uuid, $2::uuid, 'RLS inserted pool')",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "webhook_endpoints",
            id_a: webhook_a.to_string(),
            id_b: webhook_b.to_string(),
            attempted_id: Uuid::new_v4().to_string(),
            insert_sql: "INSERT INTO webhook_endpoints (id, organization_id, url, secret) VALUES ($1::uuid, $2::uuid, 'https://example.invalid/rls-new', 'test-only')",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "saas_api_keys",
            id_a: api_key_a.to_string(),
            id_b: api_key_b.to_string(),
            attempted_id: Uuid::new_v4().to_string(),
            insert_sql: "INSERT INTO saas_api_keys (id, organization_id, key_prefix, secret_hash, role) VALUES ($1::uuid, $2::uuid, 'rls-new-' || $1::text, 'rls-hash-' || $1::text, 'viewer')",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "tenant_strategies",
            id_a: strategy_a.to_string(),
            id_b: strategy_b.to_string(),
            attempted_id: Uuid::new_v4().to_string(),
            insert_sql: "INSERT INTO tenant_strategies (id, organization_id, name, module) VALUES ($1::uuid, $2::uuid, 'RLS inserted strategy', 'sniper')",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "dca_schedules",
            id_a: dca_a.clone(),
            id_b: dca_b.clone(),
            attempted_id: format!("dca-new-{}", Uuid::new_v4()),
            insert_sql: "INSERT INTO dca_schedules (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, next_run_at) VALUES ($1::text, $2::uuid, 'RLS', 0.1, 60, 1, now())",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "limit_orders",
            id_a: limit_order_a.clone(),
            id_b: limit_order_b.clone(),
            attempted_id: format!("limit-new-{}", Uuid::new_v4()),
            insert_sql: "INSERT INTO limit_orders (id, organization_id, mint, side, trigger_kind, price_sol, amount) VALUES ($1::text, $2::uuid, 'RLS', 'buy', 'at_or_below', 1, 1)",
            extra_uuid: None,
        },
        TenantRowCase {
            table: "backtest_runs",
            id_a: backtest_a.to_string(),
            id_b: backtest_b.to_string(),
            attempted_id: Uuid::new_v4().to_string(),
            insert_sql: "INSERT INTO backtest_runs (id, organization_id, strategy_id, venue, period_start, period_end, initial_balance_usd_cents, status, data_source, authoritative) VALUES ($1::uuid, $2::uuid, $3, 'test', now() - interval '1 day', now(), 10000, 'queued', 'synthetic', false)",
            extra_uuid: Some(strategy_b),
        },
        TenantRowCase {
            table: "audit_events",
            id_a: audit_a.to_string(),
            id_b: audit_b.to_string(),
            attempted_id: (audit_b.saturating_sub(1)).to_string(),
            insert_sql: "INSERT INTO audit_events (id, organization_id, actor, action, outcome, prev_hash, hash) VALUES ($1::bigint, $2::uuid, 'rls-test', 'insert', 'success', 'previous', 'event')",
            extra_uuid: None,
        },
    ];

    let mut connection = pool.acquire().await.expect("acquire RLS test connection");
    let transaction = connection.begin().await.expect("begin tenant transaction");
    let mut tenant_scope = TenantScope::from_transaction(transaction, org_a)
        .await
        .expect("install tenant A transaction context");
    let current_role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut **tenant_scope.transaction())
        .await
        .expect("read effective tenant role");
    assert_eq!(
        current_role, "bot_app",
        "tenant scope must assume the non-bypass app role"
    );
    for case in &cases {
        let visible: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {} WHERE id::text IN ($1, $2)",
            case.table
        ))
        .bind(&case.id_a)
        .bind(&case.id_b)
        .fetch_one(&mut **tenant_scope.transaction())
        .await
        .expect("read under tenant scope");
        assert_eq!(
            visible, 1,
            "tenant A should see only its own {} row",
            case.table
        );

        let updated = sqlx::query(&format!(
            "UPDATE {} SET organization_id = organization_id WHERE id::text = $1",
            case.table
        ))
        .bind(&case.id_b)
        .execute(&mut **tenant_scope.transaction())
        .await
        .expect("cross-tenant update must be filtered");
        assert_eq!(
            updated.rows_affected(),
            0,
            "tenant B {} row updated",
            case.table
        );

        let deleted = sqlx::query(&format!("DELETE FROM {} WHERE id::text = $1", case.table))
            .bind(&case.id_b)
            .execute(&mut **tenant_scope.transaction())
            .await
            .expect("cross-tenant delete must be filtered");
        assert_eq!(
            deleted.rows_affected(),
            0,
            "tenant B {} row deleted",
            case.table
        );
    }
    tenant_scope
        .commit()
        .await
        .expect("commit scoped read/update/delete checks");
    let role_after_commit: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut *connection)
        .await
        .expect("read role after scoped transaction");
    assert_eq!(
        role_after_commit, "sniper",
        "SET LOCAL ROLE must reset on commit"
    );
    let context_after_commit: Option<String> =
        sqlx::query_scalar("SELECT current_setting('app.org_id', true)")
            .fetch_one(&mut *connection)
            .await
            .expect("read tenant GUC after scoped transaction");
    assert!(
        context_after_commit
            .as_deref()
            .unwrap_or_default()
            .is_empty(),
        "tenant GUC must reset to empty rather than leak through pool reuse"
    );

    let mut unscoped_tx = pool
        .begin()
        .await
        .expect("begin unscoped tenant transaction");
    sqlx::query("SET LOCAL ROLE bot_app")
        .execute(&mut *unscoped_tx)
        .await
        .expect("switch to bot_app for unscoped check");
    for case in &cases {
        let unscoped: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {} WHERE id::text IN ($1, $2)",
            case.table
        ))
        .bind(&case.id_a)
        .bind(&case.id_b)
        .fetch_one(&mut *unscoped_tx)
        .await
        .expect("unscoped read must return no tenant rows");
        assert_eq!(unscoped, 0, "unscoped {} rows must be hidden", case.table);
    }
    unscoped_tx
        .rollback()
        .await
        .expect("rollback unscoped check");

    for case in &cases {
        assert_cross_tenant_insert_is_rejected(&pool, org_a, org_b, case).await;
    }

    let mut system_scope = SystemScope::begin(&pool)
        .await
        .expect("system scope must assume the separately privileged role");
    let system_role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut **system_scope.transaction())
        .await
        .expect("read effective system role");
    assert_eq!(system_role, "bot_system");
    let system_tenant_context: Option<String> =
        sqlx::query_scalar("SELECT NULLIF(current_setting('app.org_id', true), '')")
            .fetch_one(&mut **system_scope.transaction())
            .await
            .expect("read system transaction tenant context");
    assert_eq!(
        system_tenant_context, None,
        "system scope must clear tenant context"
    );
    let cross_tenant_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM orders WHERE id = ANY($1)")
            .bind(vec![order_a.clone(), order_b.clone()])
            .fetch_one(&mut **system_scope.transaction())
            .await
            .expect("system scope can perform reviewed cross-tenant reads");
    assert_eq!(cross_tenant_count, 2);
    system_scope.commit().await.expect("commit system scope");

    for case in &cases {
        sqlx::query(&format!(
            "DELETE FROM {} WHERE id::text IN ($1, $2)",
            case.table
        ))
        .bind(&case.id_a)
        .bind(&case.id_b)
        .execute(&pool)
        .await
        .expect("clean up RLS test rows");
    }
    sqlx::query("DELETE FROM organizations WHERE id IN ($1, $2)")
        .bind(org_a)
        .bind(org_b)
        .execute(&pool)
        .await
        .expect("clean up integration organizations");
}
