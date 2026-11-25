//! The deployment-organization cache is PER DATABASE (gap: process-global
//! cache).
//!
//! `deployment_org_uuid` used to be one process-global `OnceCell`, so a second
//! `Database` handle — a second environment, a migration run beside a live pool,
//! another database in the same test process — received the FIRST database's
//! tenant id and wrote legacy rows under a tenant that does not exist in its own
//! database. This suite builds two REAL databases in one process and proves the
//! answers are independent, stable and identical to what each database's own
//! `public.deployment_organization_id()` returns.
//!
//! Gated like the other PostgreSQL suites: NOT_RUN without `POSTGRES_URL`.
//! The databases are created and dropped by the test (the role must be able to
//! `CREATE DATABASE`); run with `--test-threads=1`.

use bot_core::config::DatabaseConfig;
use bot_core::db::deployment_org::{
    cached_deployment_org_uuid, deployment_org_uuid, forget_deployment_org_uuid,
};
use bot_core::db::Database;
use uuid::Uuid;

fn admin_url() -> Option<String> {
    if let Ok(url) = std::env::var("POSTGRES_ADMIN_URL") {
        if !url.trim().is_empty() {
            return Some(url);
        }
    }
    let url = std::env::var("POSTGRES_URL").ok()?;
    let (prefix, _db) = url.rsplit_once('/')?;
    Some(format!("{prefix}/postgres"))
}

fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

async fn create_database(admin: &str, name: &str) {
    let pool = sqlx::PgPool::connect(admin)
        .await
        .expect("admin connection");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name}"))
        .execute(&pool)
        .await
        .expect("drop stale database");
    sqlx::query(&format!("CREATE DATABASE {name} OWNER sniper"))
        .execute(&pool)
        .await
        .expect("create database");
    pool.close().await;
}

async fn drop_database(admin: &str, name: &str) {
    if let Ok(pool) = sqlx::PgPool::connect(admin).await {
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
            .execute(&pool)
            .await;
        pool.close().await;
    }
}

async fn connect(url: &str) -> Database {
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    db
}

async fn own_answer(db: &Database) -> Uuid {
    let pool = db.system_pool().unwrap_or_else(|| db.pool());
    sqlx::query_scalar("SELECT public.deployment_organization_id()")
        .fetch_one(pool)
        .await
        .expect("the database answers for itself")
}

#[tokio::test]
async fn two_databases_in_one_process_never_share_a_deployment_organization() {
    let Some(admin) = admin_url() else {
        eprintln!("NOT_RUN: deployment_org_cache_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let (name_a, name_b) = (
        format!("deploy_cache_a_{}", run.replace('-', "_")),
        format!("deploy_cache_b_{}", run.replace('-', "_")),
    );
    // The database names must be simple identifiers; run ids are pid-nanos.
    let (name_a, name_b) = (
        name_a
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>(),
        name_b
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>(),
    );
    create_database(&admin, &name_a).await;
    create_database(&admin, &name_b).await;
    let base = admin.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap();
    let (url_a, url_b) = (format!("{base}/{name_a}"), format!("{base}/{name_b}"));

    let db_a = connect(&url_a).await;
    let db_b = connect(&url_b).await;
    assert_ne!(
        db_a.identity(),
        db_b.identity(),
        "two databases must not look identical to the cache: {} vs {}",
        db_a.identity(),
        db_b.identity()
    );

    // Resolve A first, then B. Before the fix, B silently received A's uuid.
    let cached_a = deployment_org_uuid(&db_a).await.expect("A resolves");
    let cached_b = deployment_org_uuid(&db_b).await.expect("B resolves");
    assert_eq!(
        cached_a,
        own_answer(&db_a).await,
        "the cached value is A's OWN deployment organization"
    );
    assert_eq!(
        cached_b,
        own_answer(&db_b).await,
        "the cached value is B's OWN deployment organization"
    );
    assert_ne!(
        cached_a, cached_b,
        "a process-global cell would have given B the deployment organization of A"
    );

    // Resolving B again (and A again) is stable and does not cross over.
    assert_eq!(deployment_org_uuid(&db_b).await.unwrap(), cached_b);
    assert_eq!(deployment_org_uuid(&db_a).await.unwrap(), cached_a);
    assert_eq!(cached_deployment_org_uuid(&db_a), Some(cached_a));
    assert_eq!(cached_deployment_org_uuid(&db_b), Some(cached_b));

    // A's rows land under A's tenant, B's under B's — the actual consequence.
    for (db, expected) in [(&db_a, cached_a), (&db_b, cached_b)] {
        let row: (Uuid,) = sqlx::query_as(
            "INSERT INTO copy_leader_events (address, event, replica_id)
             VALUES ('cache-witness', 'buy', $1)
             RETURNING organization_id",
        )
        .bind(format!("cache-witness-{run}"))
        .fetch_one(db.system_pool().unwrap_or_else(|| db.pool()))
        .await
        .expect("a legacy-style insert with the column DEFAULT");
        assert_eq!(
            row.0, expected,
            "the row is stamped with the deployment organization of ITS database"
        );
    }

    // Forgetting ONE database's entry leaves the other's intact.
    forget_deployment_org_uuid(&db_a);
    assert_eq!(cached_deployment_org_uuid(&db_a), None);
    assert_eq!(cached_deployment_org_uuid(&db_b), Some(cached_b));
    assert_eq!(
        deployment_org_uuid(&db_a).await.unwrap(),
        cached_a,
        "re-resolving A returns A's own (idempotent) organization again"
    );

    // Two concurrent first callers on the SAME database agree with the
    // database itself — the loser's answer is compared, not dropped blindly.
    forget_deployment_org_uuid(&db_b);
    let (first, second) = tokio::join!(deployment_org_uuid(&db_b), deployment_org_uuid(&db_b));
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first, second, "concurrent first callers converge");
    assert_eq!(first, own_answer(&db_b).await);

    drop_database(&admin, &name_a).await;
    drop_database(&admin, &name_b).await;
}
