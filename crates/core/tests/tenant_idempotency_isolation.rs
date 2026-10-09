//! Cross-tenant idempotency isolation tests (tenant-isolation file 65).
//!
//! GATED on `POSTGRES_URL` (same pattern as `db_integration`):
//! skipped unless a real database is provided.
//!
//! Proves the tenant-aware idempotency helpers implement the 0027
//! TENANT-LOCAL semantics (`idempotency_keys` PK
//! `(organization_id, scope, key)`):
//!
//! * tenant A's fresh key inserts (`true`);
//! * tenant A replaying its own key is refused (`false`);
//! * tenant B using the SAME `(scope, key)` text is an INDEPENDENT
//!   row (`true`) — tenant-local identity, no cross-tenant eviction;
//! * `held_by_tenant` answers `true` only for the OWNING tenant and
//!   `false` for every other tenant (existence of another tenant's
//!   key is not leaked).

use bot_core::config::DatabaseConfig;
use bot_core::db::tenant_idempotency::{held_by_tenant, insert_once, TenantIdempotencyKey};
use bot_core::db::Database;
use bot_core::tenant::OrganizationId;

fn url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
}

async fn setup() -> Option<Database> {
    let Some(url) = url() else {
        eprintln!("POSTGRES_URL not set — skipping tenant idempotency isolation tests");
        return None;
    };
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    Some(db)
}

/// Seed the organizations row the tenant FKs point at (tenant tables
/// reference `organizations` — the row must exist first).
async fn seed_org(db: &Database, organization_id: OrganizationId) {
    let now = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name, status, created_at, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6) ON CONFLICT DO NOTHING",
    )
    .bind(organization_id.as_uuid())
    .bind(format!("org-{organization_id}"))
    .bind(format!("Org {organization_id}"))
    .bind("active")
    .bind(now)
    .bind(now)
    .execute(db.pool())
    .await
    .expect("seed organization");
}

/// Unique namespace per test run so repeated runs never collide.
fn run_id() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    )
}

#[tokio::test]
async fn key_validation_is_fail_closed() {
    let org = OrganizationId::new();
    assert!(TenantIdempotencyKey::new(org, "api", "k-1").is_ok());
    // Blank or oversized scopes/keys are refused.
    assert!(TenantIdempotencyKey::new(org, "", "k-1").is_err());
    assert!(TenantIdempotencyKey::new(org, "api", "").is_err());
    assert!(TenantIdempotencyKey::new(org, "x".repeat(65), "k-1").is_err());
    assert!(TenantIdempotencyKey::new(org, "api", "k".repeat(257)).is_err());
}

#[tokio::test]
async fn a_tenant_cannot_reuse_another_tenants_idempotency_key() {
    let Some(db) = setup().await else { return };
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let scope = format!("iso-{}", run_id());
    let key = "shared-key";

    seed_org(&db, a).await;
    seed_org(&db, b).await;
    let key_a = TenantIdempotencyKey::new(a, &scope, key).unwrap();
    let key_b = TenantIdempotencyKey::new(b, &scope, key).unwrap();

    // Tenant A inserts fresh: true.
    let mut conn = db.pool().acquire().await.unwrap();
    assert!(insert_once(&mut conn, &key_a).await.unwrap());

    // Tenant A replays its own key: refused.
    assert!(!insert_once(&mut conn, &key_a).await.unwrap());

    // Tenant B using the SAME (scope, key) text: an INDEPENDENT
    // tenant-local row (0027) — inserted fresh for B, and A's row is
    // untouched (no cross-tenant eviction or overwrite).
    assert!(insert_once(&mut conn, &key_b).await.unwrap());
    // B's replay of its own key is refused too.
    assert!(!insert_once(&mut conn, &key_b).await.unwrap());

    // Ownership stays attributed per tenant.
    assert!(held_by_tenant(&mut conn, &key_a).await.unwrap());
    assert!(held_by_tenant(&mut conn, &key_b).await.unwrap());
    // A key text only A used is invisible to B.
    let solo_key = format!("solo-{}", run_id());
    let key_solo = TenantIdempotencyKey::new(a, &scope, &solo_key).unwrap();
    assert!(insert_once(&mut conn, &key_solo).await.unwrap());
    let key_solo_b = TenantIdempotencyKey::new(b, &scope, &solo_key).unwrap();
    assert!(!held_by_tenant(&mut conn, &key_solo_b).await.unwrap());
}

#[tokio::test]
async fn distinct_tenants_keep_independent_keys() {
    let Some(db) = setup().await else { return };
    let a = OrganizationId::new();
    let b = OrganizationId::new();
    let run = run_id();

    seed_org(&db, a).await;
    seed_org(&db, b).await;
    let key_a = TenantIdempotencyKey::new(a, format!("iso-a-{run}"), "k").unwrap();
    let key_b = TenantIdempotencyKey::new(b, format!("iso-b-{run}"), "k").unwrap();

    let mut conn = db.pool().acquire().await.unwrap();
    // Each tenant's own key is fresh for them.
    assert!(insert_once(&mut conn, &key_a).await.unwrap());
    assert!(insert_once(&mut conn, &key_b).await.unwrap());
    // And each holds their own.
    assert!(held_by_tenant(&mut conn, &key_a).await.unwrap());
    assert!(held_by_tenant(&mut conn, &key_b).await.unwrap());
    // A never holds B's key and vice versa.
    let b_through_a = TenantIdempotencyKey::new(a, format!("iso-b-{run}"), "k").unwrap();
    let a_through_b = TenantIdempotencyKey::new(b, format!("iso-a-{run}"), "k").unwrap();
    assert!(!held_by_tenant(&mut conn, &b_through_a).await.unwrap());
    assert!(!held_by_tenant(&mut conn, &a_through_b).await.unwrap());
}
