//! STEP 3 integration: the tenant binding registry (cross-tenant
//! isolation) over memory and (when POSTGRES_URL is set) PostgreSQL.

use std::sync::Arc;

use bot_core::db::Database;
use bot_core::tenant::{OrganizationId, SignerProvider};
use chrono::Utc;
use sniper_suite::tenant::registry::{
    MemoryTenantBindingRegistry, PgTenantBindingRegistry, SignerBindingRow, TenantBindingRegistry,
    WalletBinding,
};

/// Insert an organizations row (the FK target every tenant table
/// references) and return its id.
async fn seed_org(db: &Arc<Database>) -> OrganizationId {
    let org = OrganizationId::new();
    sqlx::query(
        "INSERT INTO organizations (id, slug, name) \
         VALUES ($1, $2, $3)",
    )
    .bind(org.as_uuid())
    .bind(format!("it-{org:?}"))
    .bind("Integration Tenant")
    .execute(db.pool())
    .await
    .unwrap();
    org
}

#[tokio::test]
async fn memory_bindings_are_tenant_scoped() {
    let registry = MemoryTenantBindingRegistry::new();
    let a = OrganizationId::new();
    let b = OrganizationId::new();

    for org in [a, b] {
        registry
            .put_wallet(WalletBinding {
                organization_id: org,
                label: "main".into(),
                address: format!("addr-{org:?}"),
                active: true,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
    }

    // The same label resolves per tenant — never across.
    let for_a = registry.wallet(a, "main").await.unwrap().unwrap();
    let for_b = registry.wallet(b, "main").await.unwrap().unwrap();
    assert_eq!(for_a.organization_id, a);
    assert_eq!(for_b.organization_id, b);
    assert_ne!(for_a.address, for_b.address);

    // A signer under the same key ref is provider-scoped.
    registry
        .put_signer(SignerBindingRow {
            organization_id: a,
            provider: SignerProvider::Custody,
            key_ref: "shared-key".into(),
            active: true,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    assert!(registry
        .signer(a, SignerProvider::Custody, "shared-key")
        .await
        .unwrap()
        .is_some());
    assert!(registry
        .signer(b, SignerProvider::Custody, "shared-key")
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .signer(a, SignerProvider::External, "shared-key")
        .await
        .unwrap()
        .is_none());
}

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

#[tokio::test]
async fn pg_bindings_when_available() {
    let Some(url) = pg_url() else {
        eprintln!("NOT_RUN: pg bindings — POSTGRES_URL missing");
        return;
    };
    let cfg = bot_core::config::DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Arc::new(Database::connect(&cfg, &url).await.unwrap());
    db.migrate().await.unwrap();

    let registry = PgTenantBindingRegistry::new(db.clone());
    let org = seed_org(&db).await;
    registry
        .put_wallet(WalletBinding {
            organization_id: org,
            label: "it-main".into(),
            address: "ItWallet1111111111111111111111111111111111111".into(),
            active: true,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
    registry
        .put_signer(SignerBindingRow {
            organization_id: org,
            provider: SignerProvider::Custody,
            key_ref: "it-key".into(),
            active: true,
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let wallet = registry.wallet(org, "it-main").await.unwrap().unwrap();
    assert!(wallet.active);
    assert!(registry
        .signer(org, SignerProvider::Custody, "it-key")
        .await
        .unwrap()
        .is_some());

    // Another tenant sees nothing of this one's bindings.
    let outsider = OrganizationId::new();
    assert!(registry
        .wallet(outsider, "it-main")
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .signer(outsider, SignerProvider::Custody, "it-key")
        .await
        .unwrap()
        .is_none());
}
