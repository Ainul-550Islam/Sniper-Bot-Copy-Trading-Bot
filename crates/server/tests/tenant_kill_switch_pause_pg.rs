//! Releasing the tenant kill switch must not erase an individual module pause
//! (gap #83), against REAL PostgreSQL.
//!
//! `set_trading_kill_switch(active=false)` used to `DELETE … WHERE module = ANY(trading modules)`,
//! i.e. every control row the tenant had — so a tenant that paused ONE module
//! for a real reason, hit the emergency stop, then released it, came back with
//! every module enabled and the pause silently erased. The unit witnesses cover
//! the memory-mode store; this suite proves the DURABLE path (tagged rows,
//! `starts_with` release, per-tenant scoping) on PostgreSQL.
//!
//! Gated on `POSTGRES_URL` (RLS off, migrations applied by the harness).

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use bot_core::models::BotModule;
use bot_core::tenant::OrganizationId;
use chrono::Utc;
use sniper_suite::trading_data_plane::module_control_store::ModuleControlStore;
use std::sync::Arc;

async fn setup() -> Option<Arc<Database>> {
    let url = std::env::var("POSTGRES_URL").ok()?;
    if url.trim().is_empty() {
        return None;
    }
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        ..Default::default()
    };
    let db = Database::connect(&cfg, &url)
        .await
        .expect("configured database must connect");
    db.migrate().await.expect("migrations must apply");
    Some(Arc::new(db))
}

async fn org(db: &Database, slug: &str) -> OrganizationId {
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, slug, name, status) VALUES ($1,$2,$3,'active')")
        .bind(id)
        .bind(slug)
        .bind(format!("kill switch {slug}"))
        .execute(db.pool())
        .await
        .expect("organization insert");
    OrganizationId::from(id)
}

#[tokio::test]
async fn a_durable_individual_pause_survives_the_kill_switch_release() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: tenant_kill_switch_pause_pg — POSTGRES_URL missing");
        return;
    };
    let run = format!(
        "{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let a = org(&db, &format!("ks-a-{run}")).await;
    let b = org(&db, &format!("ks-b-{run}")).await;
    let now = Utc::now();
    let store = ModuleControlStore::new(Some(db.clone()));
    assert!(store.is_durable());

    // A pauses sniper for a real reason; B pauses copy.
    store
        .apply_override(
            a,
            BotModule::Sniper,
            false,
            "manual: bad signals",
            "alice",
            "c1",
            now,
        )
        .await
        .expect("A's individual pause is durable");
    store
        .apply_override(
            b,
            BotModule::Copy,
            false,
            "manual: B pause",
            "bob",
            "c1",
            now,
        )
        .await
        .expect("B's individual pause is durable");

    // A hits the tenant emergency stop, then releases it.
    store
        .set_trading_kill_switch(a, true, "operator emergency", "amy", "c2", now)
        .await
        .expect("A's kill switch engages");
    for module in BotModule::TRADING {
        let (state, _) = store.effective_state(a, module).await.expect("read");
        assert_eq!(
            state,
            "disabled",
            "{} is disabled during the stop",
            module.as_str()
        );
    }
    store
        .set_trading_kill_switch(a, false, "operator resume", "amy", "c3", now)
        .await
        .expect("A's kill switch releases");

    // The rows really left in PostgreSQL, read without the cache.
    let rows: Vec<(String, bool, String, String)> = sqlx::query_as(
        "SELECT module, enabled, reason, updated_by FROM tenant_module_controls
          WHERE organization_id = $1 ORDER BY module",
    )
    .bind(a.as_uuid())
    .fetch_all(db.pool())
    .await
    .expect("durable read");
    assert_eq!(
        rows.len(),
        1,
        "only the operator's own pause remains: {rows:?}"
    );
    assert_eq!(rows[0].0, BotModule::Sniper.as_str());
    assert!(!rows[0].1);
    assert_eq!(rows[0].2, "manual: bad signals", "the reason is preserved");
    assert_eq!(rows[0].3, "alice", "the original actor is preserved");

    // Through the store: sniper stays paused, the other modules are released.
    assert_eq!(
        store.effective_state(a, BotModule::Sniper).await.unwrap().0,
        "disabled"
    );
    for module in [BotModule::Copy, BotModule::Polymarket] {
        assert_eq!(
            store.effective_state(a, module).await.unwrap().0,
            "enabled",
            "{} is released by the kill-switch release",
            module.as_str()
        );
    }
    // B is untouched by A's stop and resume.
    assert_eq!(
        store.effective_state(b, BotModule::Copy).await.unwrap().0,
        "disabled",
        "another tenant's pause is never affected"
    );
    for module in [BotModule::Sniper, BotModule::Polymarket] {
        assert_eq!(
            store.effective_state(b, module).await.unwrap().0,
            "enabled",
            "B was never stopped"
        );
    }

    // A durable release of the kill switch twice is idempotent.
    store
        .set_trading_kill_switch(a, false, "operator resume", "amy", "c4", now)
        .await
        .expect("second release is a no-op");
    assert_eq!(
        store.effective_state(a, BotModule::Sniper).await.unwrap().0,
        "disabled"
    );
}
