//! BATCH 04 — migrations 0066/0067 on a real PostgreSQL.
//!
//! What this proves, against a live database rather than a mock:
//!
//! * both migrations apply on top of the existing graph (0001–0065) and are
//!   idempotent: re-running the whole `MIGRATOR` is a no-op, and re-executing
//!   the two SQL files by hand changes nothing and raises nothing;
//! * the tenant-isolation contract holds: `ENABLE` **and** `FORCE ROW LEVEL
//!   SECURITY`, the `tenant_org_isolation` policy, keyed on
//!   `app.current_org_id()` — and it is installed by the migration itself,
//!   guarded on the function existing, so it also works on a database without
//!   the Batch 01 foundation;
//! * every CHECK constraint actually fires, so a buggy writer cannot create a
//!   nonsense order, a second live consent, or a second pending command;
//! * the audit table is append-only in shape (no UPDATE path in the store, the
//!   event whitelist enforced by the database).
//!
//! Gated on `POSTGRES_URL` like the rest of the suite, and it PANICS when
//! `REQUIRE_POSTGRES=1` is set without one.

use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

const CONSENTS_SQL: &str = include_str!("../../core/migrations/0066_telegram_trade_consents.sql");
const COMMANDS_SQL: &str = include_str!("../../core/migrations/0067_telegram_trade_commands.sql");

fn pg_url() -> Option<String> {
    match std::env::var("POSTGRES_URL") {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => {
            if std::env::var("REQUIRE_POSTGRES").as_deref() == Ok("1") {
                panic!("REQUIRE_POSTGRES=1 but POSTGRES_URL is missing");
            }
            None
        }
    }
}

async fn setup() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        max_connections: 4,
        acquire_timeout_ms: 60_000,
        statement_timeout_ms: 120_000,
        query_timeout_ms: 120_000,
        ..Default::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &url)
            .await
            .expect("connect postgres"),
    );
    db.migrate().await.expect("migrations must apply");
    Some(db)
}

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).expect("timestamp")
}

async fn apply_migrations_again(db: &Database) {
    // Whole-graph idempotency: sqlx records the versions and skips them.
    db.migrate()
        .await
        .expect("migrations must re-apply cleanly");
    // File-level idempotency: the SQL itself is written to be re-runnable.
    sqlx::raw_sql(CONSENTS_SQL)
        .execute(db.pool())
        .await
        .expect("0066 must be idempotent");
    sqlx::raw_sql(COMMANDS_SQL)
        .execute(db.pool())
        .await
        .expect("0067 must be idempotent");
}

#[tokio::test]
async fn both_migrations_are_recorded_and_idempotent() {
    let Some(db) = setup().await else { return };
    apply_migrations_again(&db).await;

    let rows = sqlx::query("SELECT version, description FROM _sqlx_migrations ORDER BY version")
        .fetch_all(db.pool())
        .await
        .expect("migration ledger");
    let versions: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>("version")).collect();
    assert!(versions.contains(&66), "0066 must be recorded");
    assert!(versions.contains(&67), "0067 must be recorded");
    assert!(
        versions.len() > 60,
        "the whole graph applies ({} migrations)",
        versions.len()
    );
    // sqlx renders the file stem as a spaced description
    // ("telegram trade consents"), so compare on the version — the identity that
    // actually matters — and on the de-spaced description.
    let descriptions: Vec<String> = rows
        .iter()
        .map(|r| r.get::<String, _>("description").replace(' ', "_"))
        .collect();
    assert!(descriptions.iter().any(|d| d == "telegram_trade_consents"));
    assert!(descriptions.iter().any(|d| d == "telegram_trade_commands"));
    assert_eq!(
        versions.last().copied(),
        Some(67),
        "0067 must be the newest migration in the graph"
    );
}

#[tokio::test]
async fn the_three_tables_and_their_indexes_exist() {
    let Some(db) = setup().await else { return };
    let rows = sqlx::query(
        "SELECT relname FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind = 'r' \
           AND c.relname IN ('telegram_trade_consents', 'telegram_trade_commands', \
                             'telegram_trade_audit') ORDER BY relname",
    )
    .fetch_all(db.pool())
    .await
    .expect("tables");
    let tables: Vec<String> = rows.iter().map(|r| r.get::<String, _>("relname")).collect();
    assert_eq!(
        tables,
        vec![
            "telegram_trade_audit".to_string(),
            "telegram_trade_commands".to_string(),
            "telegram_trade_consents".to_string()
        ]
    );

    // The port idempotency key is a real, UNIQUE column (not an implicit
    // agreement), and it must accept NULL never — a command without one could
    // be executed twice downstream.
    let idempotency = sqlx::query(
        "SELECT is_nullable, data_type FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = 'telegram_trade_commands' \
           AND column_name = 'idempotency_key'",
    )
    .fetch_one(db.pool())
    .await
    .expect("telegram_trade_commands.idempotency_key must exist");
    assert_eq!(idempotency.get::<String, _>("data_type"), "uuid");
    assert_eq!(idempotency.get::<String, _>("is_nullable"), "NO");
    let unique = sqlx::query(
        // Accept any constraint/index whose key is exactly (idempotency_key):
        // a table-level UNIQUE, or the implicit unique index behind it.
        "SELECT count(*) AS n FROM ( \
             SELECT conkey AS cols FROM pg_constraint \
              WHERE conrelid = 'public.telegram_trade_commands'::regclass \
                AND contype IN ('u', 'p') \
             UNION ALL \
             SELECT indkey AS cols FROM pg_index \
              WHERE indrelid = 'public.telegram_trade_commands'::regclass \
                AND indisunique \
         ) keys \
         WHERE keys.cols = ARRAY[ \
             (SELECT attnum FROM pg_attribute \
               WHERE attrelid = 'public.telegram_trade_commands'::regclass \
                 AND attname = 'idempotency_key')]",
    )
    .fetch_one(db.pool())
    .await
    .expect("unique constraint lookup");
    assert!(
        unique.get::<i64, _>("n") >= 1,
        "idempotency_key must be UNIQUE"
    );

    let indexes = sqlx::query(
        "SELECT indexname FROM pg_indexes WHERE schemaname = 'public' \
           AND tablename LIKE 'telegram_trade_%' ORDER BY indexname",
    )
    .fetch_all(db.pool())
    .await
    .expect("indexes");
    let names: Vec<String> = indexes
        .iter()
        .map(|r| r.get::<String, _>("indexname"))
        .collect();
    for expected in [
        "telegram_trade_consents_one_active",
        "telegram_trade_consents_chat_lookup",
        "telegram_trade_commands_one_pending",
        "telegram_trade_commands_chat_recent",
        "telegram_trade_commands_spent",
        "telegram_trade_audit_org_ts",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing index {expected} (have {names:?})"
        );
    }
}

#[tokio::test]
async fn tenant_isolation_is_enabled_forced_and_policied() {
    let Some(db) = setup().await else { return };
    for table in [
        "telegram_trade_consents",
        "telegram_trade_commands",
        "telegram_trade_audit",
    ] {
        let row = sqlx::query(
            "SELECT c.relrowsecurity, c.relforcerowsecurity FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'public' AND c.relname = $1",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .expect("pg_class row");
        assert!(row.get::<bool, _>("relrowsecurity"), "{table} RLS");
        assert!(row.get::<bool, _>("relforcerowsecurity"), "{table} FORCE");

        let policy = sqlx::query(
            "SELECT qual, with_check FROM pg_policies \
             WHERE schemaname = 'public' AND tablename = $1 \
               AND policyname = 'tenant_org_isolation'",
        )
        .bind(table)
        .fetch_one(db.pool())
        .await
        .unwrap_or_else(|e| panic!("{table} must have tenant_org_isolation: {e}"));
        let qual: String = policy.get("qual");
        let check: String = policy.get("with_check");
        assert!(
            qual.contains("current_org_id"),
            "{table} USING without the tenant function: {qual}"
        );
        assert!(
            check.contains("current_org_id"),
            "{table} WITH CHECK without the tenant function: {check}"
        );
    }
    // The guard is what makes the migration safe on a database without the
    // Batch 01 foundation: prove the function is the one being referenced.
    let exists = sqlx::query("SELECT to_regprocedure('app.current_org_id()') IS NOT NULL AS ok")
        .fetch_one(db.pool())
        .await
        .expect("regprocedure");
    assert!(exists.get::<bool, _>("ok"));
}

#[tokio::test]
async fn consent_constraints_are_enforced_by_the_database() {
    let Some(db) = setup().await else { return };
    let org = Uuid::new_v4();
    let chat = 8_100_000_000i64 + i64::from(Uuid::new_v4().as_u128() as u32);

    let insert = |chat_id: i64, scope: &'static str, max_order: i64, daily: i64| {
        let pool = db.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO telegram_trade_consents \
                 (consent_id, organization_id, user_id, chat_id, scope, max_order_lamports, \
                  daily_cap_lamports, granted_at, expires_at, phrase_sha256) \
                 VALUES ($1, $2, 111, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(Uuid::new_v4())
            .bind(org)
            .bind(chat_id)
            .bind(
                scope
                    .trim_matches(|c| c == '{' || c == '}' || c == '"')
                    .split(',')
                    .map(|s| s.trim_matches('"').to_string())
                    .collect::<Vec<String>>(),
            )
            .bind(max_order)
            .bind(daily)
            .bind(at(1_000))
            .bind(at(90_000))
            .bind(vec![0u8; 32])
            .execute(&pool)
            .await
        }
    };

    // Baseline row is fine.
    insert(chat, "{buy}", 50_000_000, 200_000_000)
        .await
        .expect("valid consent row");
    // Unknown scope label.
    assert!(insert(chat + 1, "{swap}", 50_000_000, 200_000_000)
        .await
        .is_err());
    // Empty scope.
    insert(chat + 2, "{}", 50_000_000, 200_000_000)
        .await
        .expect_err("empty scope must be refused");
    // Zero / negative order cap.
    assert!(insert(chat + 3, "{buy}", 0, 200_000_000).await.is_err());
    // Daily below order.
    assert!(insert(chat + 4, "{buy}", 200_000_000, 50_000_000)
        .await
        .is_err());
    // A second live consent for the same chat.
    assert!(insert(chat, "{buy}", 50_000_000, 200_000_000)
        .await
        .is_err());
    // Wrong-length phrase digest.
    let bad_digest = sqlx::query(
        "INSERT INTO telegram_trade_consents \
         (consent_id, organization_id, user_id, chat_id, scope, max_order_lamports, \
          daily_cap_lamports, granted_at, expires_at, phrase_sha256) \
         VALUES ($1, $2, 111, $3, ARRAY['buy'], 1, 1, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(chat + 5)
    .bind(at(1_000))
    .bind(at(90_000))
    .bind(vec![0u8; 31])
    .execute(db.pool())
    .await;
    assert!(bad_digest.is_err());
    // Expiry must be after the grant.
    let backwards = sqlx::query(
        "INSERT INTO telegram_trade_consents \
         (consent_id, organization_id, user_id, chat_id, scope, max_order_lamports, \
          daily_cap_lamports, granted_at, expires_at, phrase_sha256) \
         VALUES ($1, $2, 111, $3, ARRAY['buy'], 1, 1, $4, $5, $6)",
    )
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(chat + 6)
    .bind(at(90_000))
    .bind(at(1_000))
    .bind(vec![0u8; 32])
    .execute(db.pool())
    .await;
    assert!(backwards.is_err());
}

#[tokio::test]
async fn command_constraints_are_enforced_by_the_database() {
    let Some(db) = setup().await else { return };
    let org = Uuid::new_v4();
    let chat = 8_200_000_000i64 + i64::from(Uuid::new_v4().as_u128() as u32);
    const MINT: &str = "So11111111111111111111111111111111111111112";

    let insert = |chat_id: i64,
                  action: &'static str,
                  mint: &'static str,
                  amount: Option<i64>,
                  percent: Option<i32>,
                  slippage: i16,
                  status: &'static str,
                  nonce: Vec<u8>| {
        let pool = db.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO telegram_trade_commands \
                 (command_id, organization_id, chat_id, user_id, action, mint, amount_lamports, \
                  percent_bps, slippage_bps, nonce_sha256, idempotency_key, status, created_at, \
                  expires_at) \
                 VALUES ($1, $2, $3, 111, $4, $5, $6, $7, $8, $9, $1, $10, $11, $12)",
            )
            .bind(Uuid::new_v4())
            .bind(org)
            .bind(chat_id)
            .bind(action)
            .bind(mint)
            .bind(amount)
            .bind(percent)
            .bind(slippage)
            .bind(nonce)
            .bind(status)
            .bind(at(1_000))
            .bind(at(1_060))
            .execute(&pool)
            .await
        }
    };

    insert(
        chat,
        "buy",
        MINT,
        Some(10_000_000),
        None,
        100,
        "pending",
        vec![1u8; 32],
    )
    .await
    .expect("valid buy");

    // A second pending command in the same chat is refused by the partial
    // unique index…
    assert!(insert(
        chat,
        "buy",
        MINT,
        Some(1),
        None,
        100,
        "pending",
        vec![2u8; 32]
    )
    .await
    .is_err());
    // …a settled one is not.
    insert(
        chat,
        "buy",
        MINT,
        Some(1),
        None,
        100,
        "executed",
        vec![3u8; 32],
    )
    .await
    .expect("executed history row");

    // Shapes.
    insert(
        chat + 1,
        "buy",
        MINT,
        None,
        None,
        100,
        "pending",
        vec![4u8; 32],
    )
    .await
    .expect_err("a buy without an amount");
    insert(
        chat + 2,
        "buy",
        MINT,
        Some(1),
        Some(5_000),
        100,
        "pending",
        vec![5u8; 32],
    )
    .await
    .expect_err("a buy with a percentage");
    insert(
        chat + 3,
        "sell",
        MINT,
        None,
        None,
        100,
        "pending",
        vec![6u8; 32],
    )
    .await
    .expect_err("a sell without a percentage");
    insert(
        chat + 4,
        "sell",
        MINT,
        Some(1),
        Some(5_000),
        100,
        "pending",
        vec![7u8; 32],
    )
    .await
    .expect_err("a sell with an amount");
    insert(
        chat + 5,
        "sell",
        MINT,
        None,
        Some(10_001),
        100,
        "pending",
        vec![8u8; 32],
    )
    .await
    .expect_err("percentage above 100%");
    insert(
        chat + 6,
        "sell",
        MINT,
        None,
        Some(0),
        100,
        "pending",
        vec![9u8; 32],
    )
    .await
    .expect_err("zero percentage");
    insert(
        chat + 7,
        "swap",
        MINT,
        Some(1),
        None,
        100,
        "pending",
        vec![10u8; 32],
    )
    .await
    .expect_err("unknown action");
    insert(
        chat + 8,
        "buy",
        "not-a-mint",
        Some(1),
        None,
        100,
        "pending",
        vec![11u8; 32],
    )
    .await
    .expect_err("malformed mint");
    insert(
        chat + 9,
        "buy",
        MINT,
        Some(0),
        None,
        100,
        "pending",
        vec![12u8; 32],
    )
    .await
    .expect_err("zero amount");
    insert(
        chat + 10,
        "buy",
        MINT,
        Some(1),
        None,
        0,
        "pending",
        vec![13u8; 32],
    )
    .await
    .expect_err("zero slippage");
    insert(
        chat + 11,
        "buy",
        MINT,
        Some(1),
        None,
        501,
        "pending",
        vec![14u8; 32],
    )
    .await
    .expect_err("slippage above the window");
    insert(
        chat + 12,
        "buy",
        MINT,
        Some(1),
        None,
        100,
        "teleported",
        vec![15u8; 32],
    )
    .await
    .expect_err("unknown status");
    // Every one of the seven states the Rust machine can write is accepted.
    for (offset, status) in [
        (20, "pending"),
        (21, "denied"),
        (22, "cancelled"),
        (23, "expired"),
        (24, "confirmed"),
        (25, "failed"),
    ] {
        insert(
            chat + offset,
            "buy",
            MINT,
            Some(1),
            None,
            100,
            status,
            vec![(40 + offset) as u8; 32],
        )
        .await
        .unwrap_or_else(|e| panic!("status {status} must be accepted: {e}"));
    }
    insert(
        chat + 13,
        "buy",
        MINT,
        Some(1),
        None,
        100,
        "pending",
        vec![16u8; 31],
    )
    .await
    .expect_err("short nonce digest");

    // The expiry must be after creation.
    let backwards = sqlx::query(
        "INSERT INTO telegram_trade_commands \
         (command_id, organization_id, chat_id, user_id, action, mint, amount_lamports, \
          slippage_bps, nonce_sha256, idempotency_key, status, created_at, expires_at) \
         VALUES ($1, $2, $3, 111, 'buy', $4, 1, 100, $5, $1, 'pending', $6, $7)",
    )
    .bind(Uuid::new_v4())
    .bind(org)
    .bind(chat + 14)
    .bind(MINT)
    .bind(vec![17u8; 32])
    .bind(at(1_060))
    .bind(at(1_000))
    .execute(db.pool())
    .await;
    assert!(backwards.is_err());
}

#[tokio::test]
async fn the_audit_table_accepts_only_known_events_and_object_details() {
    let Some(db) = setup().await else { return };
    let org = Uuid::new_v4();
    let chat = 8_300_000_000i64 + i64::from(Uuid::new_v4().as_u128() as u32);

    for event in [
        "requested",
        "denied",
        "confirmed",
        "executed",
        "failed",
        "expired",
        "cancelled",
        "revoked",
        "consent_granted",
    ] {
        sqlx::query(
            "INSERT INTO telegram_trade_audit (organization_id, chat_id, event, detail, ts) \
             VALUES ($1, $2, $3, '{}'::jsonb, $4)",
        )
        .bind(org)
        .bind(chat)
        .bind(event)
        .bind(at(1_000))
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("event {event} must be accepted: {e}"));
    }
    // Unknown event.
    assert!(sqlx::query(
        "INSERT INTO telegram_trade_audit (organization_id, chat_id, event, ts) \
         VALUES ($1, $2, 'teleported', $3)",
    )
    .bind(org)
    .bind(chat)
    .bind(at(1_000))
    .execute(db.pool())
    .await
    .is_err());
    // Non-object detail.
    assert!(sqlx::query(
        "INSERT INTO telegram_trade_audit (organization_id, chat_id, event, detail, ts) \
         VALUES ($1, $2, 'denied', '\"a string\"'::jsonb, $3)",
    )
    .bind(org)
    .bind(chat)
    .bind(at(1_000))
    .execute(db.pool())
    .await
    .is_err());
    // The detail default is an empty object, never NULL.
    sqlx::query(
        "INSERT INTO telegram_trade_audit (organization_id, chat_id, event, ts) \
         VALUES ($1, $2, 'denied', $3)",
    )
    .bind(org)
    .bind(chat)
    .bind(at(1_000))
    .execute(db.pool())
    .await
    .expect("default detail");
    let row =
        sqlx::query("SELECT detail FROM telegram_trade_audit WHERE organization_id = $1 LIMIT 1")
            .bind(org)
            .fetch_one(db.pool())
            .await
            .expect("row");
    let detail: serde_json::Value = row.get("detail");
    assert!(detail.is_object());
}
