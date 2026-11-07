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

// A trigger SELECT under tenant RLS cannot distinguish a missing parent from
// an invisible foreign parent. Exercise actual inserts/updates as bot_app, not
// just an admin query or an assertion that the constraint's name exists.
fn assert_custody_profile_fk(error: sqlx::Error) {
    let database_error = error
        .as_database_error()
        .expect("custody tenant mismatch must produce a database constraint error");
    assert_eq!(database_error.code().as_deref(), Some("23503"));
    assert_eq!(
        database_error.constraint(),
        Some("custody_signers_profile_org_fk"),
        "a uniqueness error, generic trigger failure or RLS rejection is not the FK regression"
    );
}

async fn custody_profile_tenant_fk_under_rls(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    organization_a: uuid::Uuid,
    organization_b: uuid::Uuid,
) {
    let profile_a = uuid::Uuid::new_v4();
    let profile_b = uuid::Uuid::new_v4();
    let signer_a = uuid::Uuid::new_v4();
    for (profile, organization, name) in [
        (profile_a, organization_a, "own-profile"),
        (profile_b, organization_b, "foreign-profile"),
    ] {
        sqlx::query(
            "INSERT INTO custody_profiles (id, organization_id, name, provider_type) VALUES ($1, $2, $3, 'local')",
        )
        .bind(profile)
        .bind(organization)
        .bind(name)
        .execute(&mut **transaction)
        .await
        .expect("privileged custody profile fixtures must insert");
    }
    sqlx::query("SET LOCAL ROLE bot_app")
        .execute(&mut **transaction)
        .await
        .expect("disposable migration test admin must be allowed to SET ROLE bot_app");
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(organization_a.to_string())
        .execute(&mut **transaction)
        .await
        .expect("tenant A RLS context must be set");
    let tested_role: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut **transaction)
        .await
        .expect("tested database role must be readable");
    assert_eq!(tested_role, "bot_app");
    let foreign_visible: i64 =
        sqlx::query_scalar("SELECT count(*) FROM custody_profiles WHERE id = $1")
            .bind(profile_b)
            .fetch_one(&mut **transaction)
            .await
            .expect("RLS visibility must be checked");
    assert_eq!(foreign_visible, 0, "foreign profile must be hidden by RLS");

    sqlx::query(
        "INSERT INTO custody_signers (id, organization_id, custody_profile_id, logical_identity, provider_type, public_address) VALUES ($1, $2, $3, 'same-org-control', 'local', 'same-org-fixture')",
    )
    .bind(signer_a)
    .bind(organization_a)
    .bind(profile_a)
    .execute(&mut **transaction)
    .await
    .expect("a same-organization signer must still insert under bot_app");

    sqlx::query("SAVEPOINT foreign_custody_insert")
        .execute(&mut **transaction)
        .await
        .expect("foreign-profile insert savepoint must start");
    let rejected_insert = sqlx::query(
        "INSERT INTO custody_signers (id, organization_id, custody_profile_id, logical_identity, provider_type, public_address) VALUES ($1, $2, $3, 'foreign-org-insert', 'local', 'foreign-org-fixture')",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(organization_a)
    .bind(profile_b)
    .execute(&mut **transaction)
    .await
    .expect_err("a signer must not attach to an RLS-hidden foreign profile");
    assert_custody_profile_fk(rejected_insert);
    sqlx::query("ROLLBACK TO SAVEPOINT foreign_custody_insert")
        .execute(&mut **transaction)
        .await
        .expect("failed foreign-profile insert must roll back to its savepoint");
    sqlx::query("RELEASE SAVEPOINT foreign_custody_insert")
        .execute(&mut **transaction)
        .await
        .expect("foreign-profile insert savepoint must release");

    sqlx::query("SAVEPOINT foreign_custody_update")
        .execute(&mut **transaction)
        .await
        .expect("foreign-profile update savepoint must start");
    let rejected_update =
        sqlx::query("UPDATE custody_signers SET custody_profile_id = $1 WHERE id = $2")
            .bind(profile_b)
            .bind(signer_a)
            .execute(&mut **transaction)
            .await
            .expect_err("an existing signer must not move to an RLS-hidden foreign profile");
    assert_custody_profile_fk(rejected_update);
    sqlx::query("ROLLBACK TO SAVEPOINT foreign_custody_update")
        .execute(&mut **transaction)
        .await
        .expect("failed signer update must roll back to its savepoint");
    sqlx::query("RELEASE SAVEPOINT foreign_custody_update")
        .execute(&mut **transaction)
        .await
        .expect("foreign-profile update savepoint must release");

    sqlx::query("RESET ROLE")
        .execute(&mut **transaction)
        .await
        .expect("restore migration admin for the parent-row mutation check");
    sqlx::query("SAVEPOINT foreign_custody_parent_update")
        .execute(&mut **transaction)
        .await
        .expect("parent organization update savepoint must start");
    let rejected_parent_update =
        sqlx::query("UPDATE custody_profiles SET organization_id = $1 WHERE id = $2")
            .bind(organization_b)
            .bind(profile_a)
            .execute(&mut **transaction)
            .await
            .expect_err("an admin parent edit must not orphan the signer's tenant binding");
    assert_custody_profile_fk(rejected_parent_update);
    sqlx::query("ROLLBACK TO SAVEPOINT foreign_custody_parent_update")
        .execute(&mut **transaction)
        .await
        .expect("failed parent organization update must roll back to its savepoint");
    sqlx::query("RELEASE SAVEPOINT foreign_custody_parent_update")
        .execute(&mut **transaction)
        .await
        .expect("parent organization update savepoint must release");
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

    let realized_pnl_nullable: Option<String> = sqlx::query_scalar(
        "SELECT is_nullable::text FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'positions'
           AND column_name = 'realized_pnl_quote'",
    )
    .fetch_optional(&pool)
    .await
    .expect("realized PnL column metadata must be queryable");
    assert_eq!(
        realized_pnl_nullable.as_deref(),
        Some("YES"),
        "legacy realized PnL must stay explicitly unknown rather than being backfilled as a guessed zero"
    );
    let realized_at_nullable: Option<String> = sqlx::query_scalar(
        "SELECT is_nullable::text FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'positions'
           AND column_name = 'realized_at'",
    )
    .fetch_optional(&pool)
    .await
    .expect("realized exit timestamp metadata must be queryable");
    assert_eq!(
        realized_at_nullable.as_deref(),
        Some("YES"),
        "legacy exit timestamps must remain unknown rather than borrowing updated_at"
    );

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
    for (id, slug) in [
        (organization_a, "migration-a"),
        (organization_b, "migration-b"),
    ] {
        sqlx::query("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(slug)
            .bind(slug)
            .execute(&mut *transaction)
            .await
            .expect("test organization must insert");
    }

    // Regression for the 0033/0070 ledger identity swap: event ids are not
    // organization-prefixed, so two tenants may have the same event id and
    // sequence number. Both tenant-local events must retain their posting;
    // a leftover UNIQUE (event_id, seq) silently drops the second line in
    // the writer's ON CONFLICT path.
    sqlx::query("SET LOCAL ROLE bot_system")
        .execute(&mut *transaction)
        .await
        .expect("migration test role must be able to exercise tenant tables");
    let shared_reference = "shared-reference";
    let shared_event_id = bot_core::accounting::event_id_for(
        bot_core::accounting::EventKind::Deposit,
        bot_core::models::BotModule::Sniper,
        bot_core::models::Venue::PumpFun,
        "shared-wallet",
        shared_reference,
    );
    for organization_id in [organization_a, organization_b] {
        sqlx::query(
            "INSERT INTO ledger_events (organization_id, event_id, kind, module, venue, wallet, asset, quote_asset, mode, reference_id, ts) VALUES ($1, $2, 'deposit', 'sniper', 'pump.fun', 'shared-wallet', 'USD', 'USD', 'paper', $3, $4)",
        )
        .bind(organization_id)
        .bind(&shared_event_id)
        .bind(shared_reference)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .expect("same event id must be independent across organizations");
        sqlx::query(
            "INSERT INTO ledger_postings (organization_id, event_id, seq, account, wallet, asset, side, amount) VALUES ($1, $2, 0, 'cash', 'shared-wallet', 'USD', 'debit', 1.0) ON CONFLICT (organization_id, event_id, seq) DO NOTHING",
        )
        .bind(organization_id)
        .bind(&shared_event_id)
        .execute(&mut *transaction)
        .await
        .expect("same event id and sequence must allow one posting per organization");
    }
    let shared_postings: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ledger_postings WHERE event_id = $1 AND seq = 0")
            .bind(&shared_event_id)
            .fetch_one(&mut *transaction)
            .await
            .expect("tenant-local postings must be countable");
    assert_eq!(
        shared_postings, 2,
        "both tenants' posting lines must persist"
    );

    // Runtime child rows must not be able to name another tenant's parent,
    // even when the child row itself passes its organization RLS policy.
    let schedule_a = format!("migration-dca-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO dca_schedules (id, organization_id, mint, amount_per_run_sol, interval_secs, budget_sol, next_run_at) VALUES ($1, $2, 'test-mint', 0.1, 60, 1.0, $3)",
    )
    .bind(&schedule_a)
    .bind(organization_a)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .expect("tenant A DCA schedule must insert");
    sqlx::query(
        "INSERT INTO dca_runs (organization_id, schedule_id, slot_at, amount_sol) VALUES ($1, $2, $3, 0.1)",
    )
    .bind(organization_a)
    .bind(&schedule_a)
    .bind(now)
    .execute(&mut *transaction)
    .await
    .expect("same-tenant DCA run must insert");
    sqlx::query("SAVEPOINT cross_tenant_dca_run")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant DCA savepoint must start");
    let dca_cross_tenant = sqlx::query(
        "INSERT INTO dca_runs (organization_id, schedule_id, slot_at, amount_sol) VALUES ($1, $2, $3, 0.1)",
    )
    .bind(organization_b)
    .bind(&schedule_a)
    .bind(now + chrono::Duration::seconds(1))
    .execute(&mut *transaction)
    .await
    .expect_err("tenant B must not attach a DCA run to tenant A's schedule");
    let dca_error = dca_cross_tenant
        .as_database_error()
        .expect("cross-tenant DCA reference must fail at a database constraint");
    assert_eq!(dca_error.code().as_deref(), Some("23503"));
    assert_eq!(
        dca_error.constraint(),
        Some("dca_runs_schedule_tenant_fkey")
    );
    sqlx::query("ROLLBACK TO SAVEPOINT cross_tenant_dca_run")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant DCA insert must roll back to savepoint");
    sqlx::query("RELEASE SAVEPOINT cross_tenant_dca_run")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant DCA savepoint must release");

    let config_a = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO tenant_sso_configs (id, organization_id, provider_type, issuer_url, client_id) VALUES ($1, $2, 'oidc', 'https://idp.example', 'migration-test')",
    )
    .bind(config_a)
    .bind(organization_a)
    .execute(&mut *transaction)
    .await
    .expect("tenant A SSO config must insert");
    let pending_state_a = format!("migration-state-a-{}", uuid::Uuid::new_v4());
    sqlx::query(
        "INSERT INTO sso_pending_auth (state, organization_id, config_id, code_verifier, redirect_uri, created_at, expires_at) VALUES ($1, $2, $3, $4, 'https://app.example/callback', $5, $6)",
    )
    .bind(&pending_state_a)
    .bind(organization_a)
    .bind(config_a)
    .bind("v".repeat(43))
    .bind(now)
    .bind(now + chrono::Duration::minutes(10))
    .execute(&mut *transaction)
    .await
    .expect("same-tenant SSO pending state must insert");
    sqlx::query("SAVEPOINT cross_tenant_sso_state")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant SSO savepoint must start");
    let pending_state_b = format!("migration-state-b-{}", uuid::Uuid::new_v4());
    let sso_cross_tenant = sqlx::query(
        "INSERT INTO sso_pending_auth (state, organization_id, config_id, code_verifier, redirect_uri, created_at, expires_at) VALUES ($1, $2, $3, $4, 'https://app.example/callback', $5, $6)",
    )
    .bind(&pending_state_b)
    .bind(organization_b)
    .bind(config_a)
    .bind("w".repeat(43))
    .bind(now)
    .bind(now + chrono::Duration::minutes(10))
    .execute(&mut *transaction)
    .await
    .expect_err("tenant B must not bind a pending SSO flow to tenant A's config");
    let sso_error = sso_cross_tenant
        .as_database_error()
        .expect("cross-tenant SSO reference must fail at a database constraint");
    assert_eq!(sso_error.code().as_deref(), Some("23503"));
    assert_eq!(
        sso_error.constraint(),
        Some("sso_pending_auth_config_tenant_fkey")
    );
    sqlx::query("ROLLBACK TO SAVEPOINT cross_tenant_sso_state")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant SSO insert must roll back to savepoint");
    sqlx::query("RELEASE SAVEPOINT cross_tenant_sso_state")
        .execute(&mut *transaction)
        .await
        .expect("cross-tenant SSO savepoint must release");

    sqlx::query("RESET ROLE")
        .execute(&mut *transaction)
        .await
        .expect("restore the migration role for subsequent contract checks");

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
    assert_eq!(
        projected_session.as_deref(),
        Some(backfill_token_hash.as_str())
    );

    custody_profile_tenant_fk_under_rls(&mut transaction, organization_a, organization_b).await;

    transaction
        .rollback()
        .await
        .expect("migration behavior test must roll back fixtures");
}
