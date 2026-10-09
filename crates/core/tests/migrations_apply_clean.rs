//! Fresh-database migration contract.
//!
//! This test is deliberately gated: it must receive a disposable PostgreSQL
//! URL and refuses to run against a database that already has applied
//! migrations. That prevents a buyer's real database from being mutated by a
//! verification test while still catching ordering, duplicate-table and
//! foreign-key mistakes on the clean path.

use std::collections::BTreeSet;

use bot_core::db::MIGRATOR;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

fn migration_url() -> Option<String> {
    match std::env::var("POSTGRES_MIGRATION_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        Ok(_) => panic!("POSTGRES_MIGRATION_URL must name a dedicated disposable database"),
        Err(_) if std::env::var("POSTGRES_URL").is_ok() => {
            panic!("POSTGRES_URL is not safe for the clean migration test; configure a separate POSTGRES_MIGRATION_URL")
        }
        Err(_) => None,
    }
}

async fn columns(pool: &PgPool, table: &str) -> BTreeSet<String> {
    sqlx::query_scalar(
        "SELECT column_name::text
           FROM information_schema.columns
          WHERE table_schema = 'public' AND table_name = $1
          ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("information_schema must be queryable")
    .into_iter()
    .collect()
}

#[tokio::test]
async fn fresh_migrations_apply_and_keep_operator_and_tenant_tables_separate() {
    let Some(url) = migration_url() else {
        eprintln!("NOT_RUN: migrations_apply_clean — POSTGRES_MIGRATION_URL missing (no shared-database fallback)");
        return;
    };

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("migration database must be reachable");

    let preexisting_public_objects: bool = sqlx::query_scalar(
        r#"SELECT
             EXISTS (
                 SELECT 1
                   FROM pg_catalog.pg_class c
                   JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
                  WHERE n.nspname = 'public'
                    AND c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')
             )
             OR EXISTS (
                 SELECT 1
                   FROM pg_catalog.pg_type t
                   JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace
                  WHERE n.nspname = 'public'
                    AND t.typtype IN ('e', 'd')
             )
             OR EXISTS (
                 SELECT 1
                   FROM pg_catalog.pg_proc p
                   JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
                  WHERE n.nspname = 'public'
             )"#,
    )
    .fetch_one(&pool)
    .await
    .expect("PostgreSQL public-schema state must be queryable");
    assert!(
        !preexisting_public_objects,
        "POSTGRES_MIGRATION_URL must point to a dedicated empty database; preexisting tables, views, sequences, functions, enums, or domains are never accepted"
    );

    MIGRATOR
        .run(&pool)
        .await
        .expect("every embedded migration must apply to an empty database");

    let operator_strategies = columns(&pool, "strategies").await;
    assert!(operator_strategies.contains("module"));
    assert!(operator_strategies.contains("name"));
    assert!(operator_strategies.contains("enabled"));
    assert!(
        !operator_strategies.contains("organization_id"),
        "migration 0037 must not mutate the deployment-level strategies table"
    );

    let tenant_strategies = columns(&pool, "tenant_strategies").await;
    for required in [
        "id",
        "organization_id",
        "name",
        "description",
        "module",
        "mode",
        "status",
        "version",
        "config_json",
    ] {
        assert!(
            tenant_strategies.contains(required),
            "tenant_strategies is missing required column {required}"
        );
    }

    for table in [
        "backtest_runs",
        "webhook_endpoints",
        "webhook_deliveries",
        "user_mfa_devices",
        "tenant_security_policies",
        "saas_runtime_records",
        "sessions",
        "tenant_sso_configs",
        "portfolio_snapshots_hourly",
        "tenant_daily_accounting",
    ] {
        assert!(
            !columns(&pool, table).await.is_empty(),
            "missing table {table}"
        );
    }

    let pending_totp_index_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname = 'public' AND indexname = 'user_mfa_one_pending_totp_per_tenant')",
    )
    .fetch_one(&pool)
    .await
    .expect("pending TOTP enrollment index must be queryable");
    assert!(
        pending_totp_index_exists,
        "migrations must enforce at most one pending TOTP device per user and tenant"
    );

    let backtest_foreign_keys: Vec<String> = sqlx::query_scalar(
        "SELECT pg_get_constraintdef(oid)::text
           FROM pg_constraint
          WHERE conrelid = 'backtest_runs'::regclass AND contype = 'f'",
    )
    .fetch_all(&pool)
    .await
    .expect("backtest foreign keys must be queryable");
    assert!(
        backtest_foreign_keys.iter().any(|definition| {
            let definition = definition.to_ascii_lowercase();
            definition.contains("foreign key (organization_id, strategy_id)")
                && definition.contains("references tenant_strategies(organization_id, id)")
        }),
        "backtest_runs must bind its strategy to the same organization: {backtest_foreign_keys:?}"
    );

    // Exercise the database rule, not just its printed definition: a valid
    // same-tenant run inserts, while a cross-tenant strategy reference fails.
    let organization_a = uuid::Uuid::new_v4();
    let organization_b = uuid::Uuid::new_v4();
    let strategy_a = uuid::Uuid::new_v4();
    let now = chrono::Utc::now();
    let mut transaction = pool.begin().await.expect("test transaction must start");
    for (id, slug) in [(organization_a, "migration-a"), (organization_b, "migration-b")] {
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(slug)
            .bind(slug)
            .execute(&mut *transaction)
            .await
            .expect("test organization must insert");
    }
    let user_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash) VALUES ($1, $2, 'migration-test-hash')",
    )
    .bind(user_id)
    .bind(format!("{}@migration.invalid", user_id))
    .execute(&mut *transaction)
    .await
    .expect("test user must insert");

    sqlx::query(
        "INSERT INTO tenant_strategies (id, organization_id, name, module) VALUES ($1, $2, 'test strategy', 'sniper')",
    )
    .bind(strategy_a)
    .bind(organization_a)
    .execute(&mut *transaction)
    .await
    .expect("test strategy must insert");

    sqlx::query(
        "INSERT INTO backtest_runs (id, organization_id, strategy_id, venue, period_start, period_end, initial_balance_usd_cents) VALUES ($1, $2, $3, 'solana', $4, $5, 10000)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(organization_a)
    .bind(strategy_a)
    .bind(now)
    .bind(now + chrono::Duration::seconds(1))
    .execute(&mut *transaction)
    .await
    .expect("same-tenant backtest strategy must be accepted");
    sqlx::query("SAVEPOINT cross_tenant_strategy_fk")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant FK savepoint must start");
    let cross_tenant = sqlx::query(
        "INSERT INTO backtest_runs (id, organization_id, strategy_id, venue, period_start, period_end, initial_balance_usd_cents) VALUES ($1, $2, $3, 'solana', $4, $5, 10000)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(organization_b)
    .bind(strategy_a)
    .bind(now)
    .bind(now + chrono::Duration::seconds(1))
    .execute(&mut *transaction)
    .await;
    assert!(
        cross_tenant.is_err(),
        "database must reject a backtest whose strategy belongs to another organization"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT cross_tenant_strategy_fk")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant FK failure must roll back to its savepoint");
    sqlx::query("RELEASE SAVEPOINT cross_tenant_strategy_fk")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant FK savepoint must release");

    let pending_device_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_mfa_devices (id, user_id, organization_id, device_type, name, secret_encrypted, verified) VALUES ($1, $2, $3, 'totp', 'pending', 'encrypted-test-secret', false)",
    )
    .bind(pending_device_id)
    .bind(user_id)
    .bind(organization_a)
    .execute(&mut *transaction)
    .await
    .expect("the first pending TOTP enrollment must insert");

    sqlx::query("SAVEPOINT duplicate_pending_totp")
        .execute(&mut *transaction)
        .await
        .expect("duplicate-device savepoint must start");
    let duplicate_pending = sqlx::query(
        "INSERT INTO user_mfa_devices (id, user_id, organization_id, device_type, name, secret_encrypted, verified) VALUES ($1, $2, $3, 'totp', 'duplicate pending', 'encrypted-test-secret', false)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user_id)
    .bind(organization_a)
    .execute(&mut *transaction)
    .await;
    assert!(
        duplicate_pending.is_err(),
        "database must reject concurrent duplicate pending TOTP enrollments"
    );
    sqlx::query("ROLLBACK TO SAVEPOINT duplicate_pending_totp")
        .execute(&mut *transaction)
        .await
        .expect("duplicate-device constraint check must be rolled back to its savepoint");
    sqlx::query("RELEASE SAVEPOINT duplicate_pending_totp")
        .execute(&mut *transaction)
        .await
        .expect("duplicate-device savepoint must release");

    sqlx::query("UPDATE user_mfa_devices SET verified = true WHERE id = $1")
        .bind(pending_device_id)
        .execute(&mut *transaction)
        .await
        .expect("a successfully verified TOTP device must leave the pending set");
    sqlx::query(
        "INSERT INTO user_mfa_devices (id, user_id, organization_id, device_type, name, secret_encrypted, verified) VALUES ($1, $2, $3, 'totp', 'next pending', 'encrypted-test-secret', false)",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(user_id)
    .bind(organization_a)
    .execute(&mut *transaction)
    .await
    .expect("one new pending TOTP enrollment must be allowed after verification");

    // Exercise migration 0044's real SQL against legacy runtime records that
    // predate their normalized relational projections. The schema migrator has
    // already run on this empty database; inserting a fresh synthetic legacy
    // set here lets the backfill itself be executed repeatably in this single
    // disposable-DB test transaction without editing SQLx's migration ledger.
    let backfill_user = uuid::Uuid::new_v4();
    let backfill_org = uuid::Uuid::new_v4();
    let backfill_membership = uuid::Uuid::new_v4();
    let backfill_session = uuid::Uuid::new_v4();
    let backfill_token_hash = format!("migration-fixture-{}", uuid::Uuid::new_v4());
    let backfill_email = format!("{}@runtime-backfill.invalid", backfill_user);
    let backfill_slug = format!("runtime-backfill-{}", backfill_org.simple());
    let created_at = now.to_rfc3339();
    let expires_at = (now + chrono::Duration::hours(1)).to_rfc3339();
    let runtime_identity_fixtures = vec![
        (
            "user",
            backfill_user.to_string(),
            json!({
                "id": backfill_user,
                "email": backfill_email,
                "email_verified": true,
                "display_name": "Migration backfill fixture",
                "password_hash": "pbkdf2-sha256$fixture",
                "status": "active",
                "platform_admin": false,
                "created_at": created_at,
                "updated_at": created_at,
                "last_login_at": null
            }),
        ),
        (
            "organization",
            backfill_org.to_string(),
            json!({
                "id": backfill_org,
                "slug": backfill_slug,
                "name": "Migration backfill fixture",
                "status": "active",
                "created_by": backfill_user,
                "created_at": created_at,
                "updated_at": created_at,
                "suspended_at": null,
                "suspend_reason": ""
            }),
        ),
        (
            "membership",
            backfill_membership.to_string(),
            json!({
                "id": backfill_membership,
                "organization_id": backfill_org,
                "user_id": backfill_user,
                "role": "org_owner",
                "status": "active",
                "invited_by": backfill_user,
                "created_at": created_at,
                "updated_at": created_at
            }),
        ),
        (
            "session",
            backfill_session.to_string(),
            json!({
                "id": backfill_session,
                "user_id": backfill_user,
                "organization_id": backfill_org,
                "token_hash": backfill_token_hash,
                "token_prefix": "ses_migration_fixture",
                "user_agent": "migration-test",
                "ip": "127.0.0.1",
                "created_at": created_at,
                "last_seen_at": created_at,
                "expires_at": expires_at,
                "revoked_at": null,
                "revoke_reason": "",
                "mfa_policy_updated_at": null,
                "mfa_enrollment_only": false
            }),
        ),
    ];
    for (kind, id, record) in &runtime_identity_fixtures {
        sqlx::query(
            "INSERT INTO saas_runtime_records (kind, id, organization_id, user_id, record) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(*kind)
        .bind(id.as_str())
        .bind(backfill_org)
        .bind(backfill_user)
        .bind(record)
        .execute(&mut *transaction)
        .await
        .expect("pre-backfill SaaS runtime fixture must insert");
    }

    let preexisting_projection: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1) OR EXISTS (SELECT 1 FROM organizations WHERE id = $2) OR EXISTS (SELECT 1 FROM organization_members WHERE id = $3) OR EXISTS (SELECT 1 FROM sessions WHERE id = $4)",
    )
    .bind(backfill_user)
    .bind(backfill_org)
    .bind(backfill_membership)
    .bind(backfill_session)
    .fetch_one(&mut *transaction)
    .await
    .expect("pre-backfill projection state must be queryable");
    assert!(
        !preexisting_projection,
        "legacy runtime records must begin without relational projections"
    );

    let backfill_sql = include_str!("../migrations/0044_project_runtime_identity_rows.sql");
    for (statement_index, statement) in backfill_sql.split(";\n").enumerate() {
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        sqlx::query(statement)
            .execute(&mut *transaction)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "migration 0044 backfill statement {} must execute: {error}",
                    statement_index + 1
                )
            });
    }

    let projected_email: Option<String> =
        sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(backfill_user)
            .fetch_optional(&mut *transaction)
            .await
            .expect("backfilled user projection must be queryable");
    assert_eq!(projected_email.as_deref(), Some(backfill_email.as_str()));
    let projected_slug: Option<String> =
        sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
            .bind(backfill_org)
            .fetch_optional(&mut *transaction)
            .await
            .expect("backfilled organization projection must be queryable");
    assert_eq!(projected_slug.as_deref(), Some(backfill_slug.as_str()));
    let projected_membership: Option<String> = sqlx::query_scalar(
        "SELECT role || ':' || status FROM organization_members WHERE id = $1 AND organization_id = $2 AND user_id = $3",
    )
    .bind(backfill_membership)
    .bind(backfill_org)
    .bind(backfill_user)
    .fetch_optional(&mut *transaction)
    .await
    .expect("backfilled membership projection must be queryable");
    assert_eq!(projected_membership.as_deref(), Some("org_owner:active"));
    let projected_session: Option<String> =
        sqlx::query_scalar("SELECT token_hash FROM sessions WHERE id = $1 AND user_id = $2")
            .bind(backfill_session)
            .bind(backfill_user)
            .fetch_optional(&mut *transaction)
            .await
            .expect("backfilled session projection must be queryable");
    assert_eq!(projected_session.as_deref(), Some(backfill_token_hash.as_str()));

    transaction
        .rollback()
        .await
        .expect("migration behavior test must roll back fixtures");
}
