//! Production `DbIntentSink` fail-closed write-ahead reservation.
//!
//! With `rls_enabled = false` the deployment-scoped journal has no system
//! pool, so a checked reservation must surface an error rather than degrade
//! to best-effort. The test proves the real server sink plugs into
//! `with_intent_disposition` and that the broadcast future is never polled.
//! It does not prove a successful durable write or atomic broadcast fence.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::repo::IntentRecord;
use bot_core::db::Database;
use bot_core::recovery::{with_intent_disposition, IntentDisposition, IntentSink};
use bot_core::BotResult;
use chrono::Utc;
use sniper_suite::recon::DbIntentSink;

fn pg_url() -> Option<String> {
    std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn rec(id: &str) -> IntentRecord {
    IntentRecord {
        intent_id: id.into(),
        module: "reservation-test".into(),
        symbol: "SYM".into(),
        wallet: "W".into(),
        side: "buy".into(),
        qty: "1".into(),
        status: "pending".into(),
        signature: None,
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn production_reservation_failure_aborts_before_broadcast_future() {
    let Some(url) = pg_url() else {
        assert!(
            std::env::var_os("REQUIRE_POSTGRES").is_none(),
            "POSTGRES_URL is required for the production intent reservation test"
        );
        eprintln!("NOT_RUN: production intent reservation — POSTGRES_URL missing");
        return;
    };
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: false,
        ..Default::default()
    };
    let db = Arc::new(
        Database::connect(&cfg, &url)
            .await
            .expect("configured database must connect"),
    );
    let sink: Arc<dyn IntentSink> = Arc::new(DbIntentSink::new(db));
    let sends = Arc::new(AtomicUsize::new(0));
    let sends_in_future = sends.clone();
    let out: BotResult<()> = with_intent_disposition(
        Some(&sink),
        rec("reservation-failure"),
        async move {
            sends_in_future.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
        |_| IntentDisposition::Abandon,
    )
    .await;
    let error = out.expect_err("unavailable durable reservation must fail closed");
    assert!(
        error.to_string().contains("RLS"),
        "expected deployment-scoped journal failure, got {error}"
    );
    assert_eq!(
        sends.load(Ordering::SeqCst),
        0,
        "the money-moving future must never be polled"
    );
}
