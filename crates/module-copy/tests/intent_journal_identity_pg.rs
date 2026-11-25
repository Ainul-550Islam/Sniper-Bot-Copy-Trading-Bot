//! Distinct attempt identities across copy replicas against real PostgreSQL.

use bot_core::config::{AppConfig, DatabaseConfig};
use bot_core::db::{repo::IntentRepo, Database};
use bot_core::state::AppState;
use solana_kit::tokens::Wallet;
use std::sync::Arc;

#[tokio::test]
async fn separate_replica_attempts_do_not_alias_in_intent_journal() {
    let Some(url) = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    else {
        eprintln!("SKIP: POSTGRES_URL is not set — intent journal isolation needs PostgreSQL");
        return;
    };
    let db = Arc::new(
        Database::connect(
            &DatabaseConfig {
                enabled: true,
                auto_migrate: true,
                ..Default::default()
            },
            &url,
        )
        .await
        .expect("connect"),
    );
    db.migrate().await.expect("migrate");
    let repo = IntentRepo::new(db);
    let a = AppState::new(AppConfig::from_defaults());
    let b = AppState::new(AppConfig::from_defaults());
    let wallet = Arc::new(Wallet::generate());
    let first = module_copy::intent::journal_record(&a, &wallet, "MINT-A", "buy", "1");
    let second = module_copy::intent::journal_record(&b, &wallet, "MINT-B", "buy", "2");
    assert_ne!(
        first.intent_id, second.intent_id,
        "independent state counters must not alias journal attempts"
    );
    repo.record(&first).await.expect("first insert");
    repo.record(&second).await.expect("second insert");
    repo.link(&first.intent_id, "test-signature-a")
        .await
        .expect("link first");
    let row_a = repo.get(&first.intent_id).await.unwrap().unwrap();
    let row_b = repo.get(&second.intent_id).await.unwrap().unwrap();
    assert_eq!(row_a.symbol, "MINT-A");
    assert_eq!(row_a.status, "submitted");
    assert_eq!(row_a.signature.as_deref(), Some("test-signature-a"));
    assert_eq!(row_b.symbol, "MINT-B");
    assert_eq!(row_b.status, "pending");
    assert!(row_b.signature.is_none());
}

/// This witnesses the old aliasing failure rather than desirable behavior.
/// The two deliberately equal ids are unique to this test run.
#[tokio::test]
async fn colliding_legacy_attempt_can_link_the_wrong_journal_row() {
    let Some(url) = std::env::var("POSTGRES_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    else {
        eprintln!("SKIP: POSTGRES_URL is not set — intent journal alias witness needs PostgreSQL");
        return;
    };
    let db = Arc::new(
        Database::connect(
            &DatabaseConfig {
                enabled: true,
                auto_migrate: true,
                ..Default::default()
            },
            &url,
        )
        .await
        .expect("connect"),
    );
    db.migrate().await.expect("migrate");
    let repo = IntentRepo::new(db);
    let state = AppState::new(AppConfig::from_defaults());
    let wallet = Arc::new(Wallet::generate());
    let first = module_copy::intent::journal_record(&state, &wallet, "MINT-A", "buy", "1");
    let mut second = module_copy::intent::journal_record(&state, &wallet, "MINT-B", "buy", "2");
    second.intent_id = first.intent_id.clone();
    repo.record(&first).await.unwrap();
    repo.record(&second).await.unwrap();
    repo.link(&second.intent_id, "signature-for-attempt-b")
        .await
        .unwrap();
    let row = repo.get(&first.intent_id).await.unwrap().unwrap();
    assert_eq!(row.symbol, "MINT-A");
    assert_eq!(row.signature.as_deref(), Some("signature-for-attempt-b"));
}
