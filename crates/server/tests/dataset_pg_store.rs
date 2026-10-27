//! BATCH 02 — real-Postgres metadata store lifecycle.
//!
//! Gated: with `POSTGRES_URL` set it runs against a real database (all
//! migrations are applied). With `REQUIRE_POSTGRES=1` and NO url it PANICS
//! (fail-closed — a missing DB can never be reported as a green run).

use std::sync::Arc;

use bot_core::config::DatabaseConfig;
use bot_core::db::Database;
use module_sniper::backtest::dataset::Tick;
use module_sniper::dataset::format::{DatasetManifest, DatasetSource, RecordedEvent};
use module_sniper::dataset::policy::{RecorderConfig, VerifierPolicy};
use module_sniper::dataset::recorder::ChunkWriter;
use module_sniper::dataset::store::FsDatasetStore;
use sniper_suite::trading_data_plane::dataset_store_pg::PgDatasetStore;
use uuid::Uuid;

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

// A FRESH Database per test (never a shared static pool): each
// #[tokio::test] owns its runtime, and a sqlx PgPool must not outlive the
// runtime it was created on. sqlx's advisory lock serializes concurrent
// migrate() calls across tests.
async fn setup() -> Option<Arc<Database>> {
    let url = pg_url()?;
    let cfg = DatabaseConfig {
        enabled: true,
        auto_migrate: true,
        max_connections: 8,
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

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "sniper-ds-pg-{}-{}-{}",
        label,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

/// Build a clean RECORDED file dataset with a fresh id; return its store,
/// sealed manifest and temp root (caller cleans up the root).
fn build_recorded_dataset(root: &std::path::Path, id: &str) -> (FsDatasetStore, DatasetManifest) {
    let files = FsDatasetStore::new(root);
    let cfg = RecorderConfig::new(root, 1000, 4 * 1024 * 1024, 1).unwrap();
    let m = DatasetManifest::new(id.into(), "pg-life".into(), DatasetSource::Recorded);
    let mut w = ChunkWriter::create(files.clone(), m, &cfg).unwrap();
    for i in 0..8u64 {
        w.append_with_slot(
            i + 1,
            RecordedEvent::PoolPriceTick {
                slot: i + 1,
                mint: "Mint1111111111111111111111111111111111111111".into(),
                tick: Tick {
                    t_ms: i,
                    price_sol: 1.0 + i as f64 * 1e-9,
                    quote_reserve_lamports: 1_000_000 + i,
                },
            },
        )
        .unwrap();
    }
    let sealed = w.seal().unwrap();
    (files, sealed)
}

#[tokio::test]
async fn create_seal_verify_lifecycle() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: create/seal/verify — POSTGRES_URL missing");
        return;
    };
    let root = temp_root("lifecycle");
    let id = Uuid::new_v4().to_string();
    let (files, sealed) = build_recorded_dataset(&root, &id);
    let store = PgDatasetStore::new(db.pool().clone());

    // create -> recording, never authoritative (a self-declared flag in the
    // manifest must be ignored).
    let mut lying = sealed.clone();
    lying.authoritative = true;
    store.create(&lying).await.unwrap();
    let row = store.get(&id).await.unwrap();
    assert_eq!(row.status, "recording");
    assert!(!row.authoritative);

    // seal -> sealed + chunk metadata persisted.
    store.seal(&sealed).await.unwrap();
    let row = store.get(&id).await.unwrap();
    assert_eq!(row.status, "sealed");
    assert_eq!(row.record_count, 8);
    assert_eq!(row.sha256_root, sealed.sha256_root);
    assert_eq!(
        store.chunk_count(&id).await.unwrap(),
        sealed.chunks.len() as i64
    );

    // verify_and_record runs the sole verifier and flips authoritative ONLY
    // from its report -> verified + authoritative (recorded, clean chain).
    let report = store
        .verify_and_record(&files, &id, &VerifierPolicy::new(1, 0, 1000).unwrap())
        .await
        .unwrap();
    assert!(report.hash_chain_ok);
    assert!(report.coverage_ok);
    assert!(report.authoritative);
    let row = store.get(&id).await.unwrap();
    assert_eq!(row.status, "verified");
    assert!(row.authoritative);
    assert_eq!(store.verification_count(&id).await.unwrap(), 1);

    // The audit log is append-only: a second verification adds a row.
    store.record_verification(&report).await.unwrap();
    assert_eq!(store.verification_count(&id).await.unwrap(), 2);

    store.delete(&id).await.unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn cascade_delete_removes_chunks_and_verifications() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: cascade delete — POSTGRES_URL missing");
        return;
    };
    let root = temp_root("cascade");
    let id = Uuid::new_v4().to_string();
    let (files, sealed) = build_recorded_dataset(&root, &id);
    let store = PgDatasetStore::new(db.pool().clone());
    store.create(&sealed).await.unwrap();
    store.seal(&sealed).await.unwrap();
    store
        .verify_and_record(&files, &id, &VerifierPolicy::new(1, 0, 1000).unwrap())
        .await
        .unwrap();
    assert!(store.chunk_count(&id).await.unwrap() > 0);
    assert!(store.verification_count(&id).await.unwrap() > 0);

    store.delete(&id).await.unwrap();
    assert_eq!(store.chunk_count(&id).await.unwrap(), 0, "chunks cascade");
    assert_eq!(
        store.verification_count(&id).await.unwrap(),
        0,
        "verifications cascade"
    );
    assert!(store.get(&id).await.is_err(), "dataset row gone");
    let _ = std::fs::remove_dir_all(&root);
}

/// Direct raw INSERT probe for the 0063 CHECK constraints.
async fn try_insert(
    pool: &sqlx::PgPool,
    id: Uuid,
    source: &str,
    status: &str,
    authoritative: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO recorded_launch_datasets \
         (dataset_id, name, source, schema_version, status, authoritative) \
         VALUES ($1, 'probe', $2, 1, $3, $4)",
    )
    .bind(id)
    .bind(source)
    .bind(status)
    .bind(authoritative)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn check_constraints_reject_forged_authoritative_and_bad_labels() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: CHECK constraints — POSTGRES_URL missing");
        return;
    };
    let pool = db.pool();

    // authoritative=true for a synthetic_fixture row -> rejected.
    assert!(
        try_insert(pool, Uuid::new_v4(), "synthetic_fixture", "verified", true)
            .await
            .is_err()
    );
    // authoritative=true while status != verified -> rejected.
    assert!(try_insert(pool, Uuid::new_v4(), "recorded", "sealed", true)
        .await
        .is_err());
    // The one legal authoritative state: recorded + verified.
    assert!(
        try_insert(pool, Uuid::new_v4(), "recorded", "verified", true)
            .await
            .is_ok()
    );
    // Unknown source / status vocabulary is rejected by the CHECKs.
    assert!(try_insert(pool, Uuid::new_v4(), "bogus", "sealed", false)
        .await
        .is_err());
    assert!(try_insert(pool, Uuid::new_v4(), "recorded", "bogus", false)
        .await
        .is_err());
    // outcome CHECK on the verification table.
    let did = Uuid::new_v4();
    try_insert(pool, did, "recorded", "verified", false)
        .await
        .unwrap();
    let bad_outcome = sqlx::query(
        "INSERT INTO recorded_launch_dataset_verifications \
         (dataset_id, verifier_version, hash_chain_ok, coverage_ok, outcome) \
         VALUES ($1, 'v', true, true, 'maybe')",
    )
    .bind(did)
    .execute(pool)
    .await;
    assert!(bad_outcome.is_err(), "unknown outcome must be rejected");
}
