//! A transaction must never accept write attribution for another tenant.
//! Uses the disposable POSTGRES_URL database; REQUIRE_POSTGRES=1 makes absence fatal.

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::tenant::OrganizationId;
use bot_core::trading_repository::{
    RepositoryError, TenantWriteScope, TradingQueryScope, TradingTransaction, WriteOrigin,
};

async fn database() -> Option<Database> {
    let url = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let Some(url) = url else {
        assert!(
            std::env::var("REQUIRE_POSTGRES").as_deref() != Ok("1"),
            "POSTGRES_URL is required for the write-scope isolation proof"
        );
        eprintln!("NOT_RUN: POSTGRES_URL missing for write-scope isolation proof");
        return None;
    };
    let config = DatabaseConfig {
        enabled: true,
        auto_migrate: false,
        ..Default::default()
    };
    Some(
        Database::connect(&config, &url)
            .await
            .expect("test database connects"),
    )
}

#[tokio::test]
async fn foreign_write_scope_never_replaces_existing_attribution() {
    let Some(db) = database().await else { return };
    let acting = OrganizationId::new();
    let foreign = OrganizationId::new();
    let mut transaction = TradingTransaction::begin(&db, TradingQueryScope::new(acting))
        .await
        .expect("begin scoped transaction");
    let own = TenantWriteScope::new(acting, "user:acting", WriteOrigin::Http).unwrap();
    let other = TenantWriteScope::new(foreign, "user:foreign", WriteOrigin::Operator).unwrap();
    assert!(transaction.for_write(own).unwrap().is_none());
    assert!(matches!(
        transaction.for_write(other),
        Err(RepositoryError::TenantMismatch)
    ));
    assert_eq!(transaction.organization_id(), acting);
    assert_eq!(transaction.actor(), "user:acting");
    assert_eq!(transaction.write_scope().unwrap().organization_id(), acting);
    transaction.rollback().await.expect("rollback");
}

#[tokio::test]
async fn foreign_first_write_scope_never_creates_attribution() {
    let Some(db) = database().await else { return };
    let acting = OrganizationId::new();
    let foreign = OrganizationId::new();
    let mut transaction = TradingTransaction::begin(&db, TradingQueryScope::new(acting))
        .await
        .expect("begin scoped transaction");
    let other = TenantWriteScope::new(foreign, "user:foreign", WriteOrigin::Job).unwrap();
    assert!(matches!(
        transaction.for_write(other),
        Err(RepositoryError::TenantMismatch)
    ));
    assert!(transaction.write_scope().is_none());
    assert_eq!(transaction.actor(), "");
    transaction.rollback().await.expect("rollback");
}

#[tokio::test]
async fn same_tenant_scope_can_replace_attribution_and_returns_previous_scope() {
    let Some(db) = database().await else { return };
    let acting = OrganizationId::new();
    let mut transaction = TradingTransaction::begin(&db, TradingQueryScope::new(acting))
        .await
        .expect("begin scoped transaction");
    let first = TenantWriteScope::new(acting, "user:first", WriteOrigin::Http).unwrap();
    let second = TenantWriteScope::new(acting, "user:second", WriteOrigin::Recovery).unwrap();
    assert!(transaction.for_write(first.clone()).unwrap().is_none());
    assert_eq!(transaction.for_write(second.clone()).unwrap(), Some(first));
    assert_eq!(transaction.write_scope(), Some(&second));
    assert_eq!(transaction.organization_id(), acting);
    transaction
        .commit()
        .await
        .expect("commit same-tenant transaction");
}
