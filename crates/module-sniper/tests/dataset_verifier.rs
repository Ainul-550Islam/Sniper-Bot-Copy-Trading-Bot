//! BATCH 02 — verifier: hash-chain integrity, coverage policy, and the
//! honesty rule that only the verifier decides `authoritative`.

use std::path::PathBuf;

use module_sniper::backtest::dataset::Tick;
use module_sniper::dataset::format::{DatasetManifest, DatasetSource, RecordedEvent};
use module_sniper::dataset::policy::{RecorderConfig, VerifierPolicy};
use module_sniper::dataset::recorder::ChunkWriter;
use module_sniper::dataset::store::{DatasetStore, FsDatasetStore};
use module_sniper::dataset::verifier::{verify, VerificationOutcome};

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sniper-ds-ver-{}-{}-{}",
        label,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

fn cleanup(root: &PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

fn tick(i: u64) -> RecordedEvent {
    RecordedEvent::PoolPriceTick {
        slot: i, // overridden by append_with_slot
        mint: "Mint1111111111111111111111111111111111111111".into(),
        tick: Tick {
            t_ms: i,
            price_sol: 1.0 + i as f64 * 1e-9,
            quote_reserve_lamports: 1_000_000 + i,
        },
    }
}

/// Build a sealed RECORDED dataset with explicit capture slots and a chosen
/// rotation size. Returns the store and the dataset id.
fn build_dataset(
    root: &PathBuf,
    slots: &[u64],
    chunk_max_records: usize,
) -> (FsDatasetStore, String) {
    let store = FsDatasetStore::new(root);
    let cfg = RecorderConfig::new(root, chunk_max_records, 4 * 1024 * 1024, 1).unwrap();
    let m = DatasetManifest::new("ds-v".into(), "verify".into(), DatasetSource::Recorded);
    let mut w = ChunkWriter::create(store.clone(), m, &cfg).unwrap();
    for (i, &slot) in slots.iter().enumerate() {
        w.append_with_slot(slot, tick(i as u64)).unwrap();
    }
    w.seal().unwrap();
    (store, "ds-v".into())
}

fn clean_policy() -> VerifierPolicy {
    VerifierPolicy::new(1, 0, 1000).unwrap()
}

#[test]
fn clean_recorded_dataset_passes_and_is_authoritative() {
    let root = temp_root("clean");
    let slots: Vec<u64> = (1..=20).collect();
    let (store, id) = build_dataset(&root, &slots, 6);
    // multiple chunks were produced
    assert_eq!(store.load_manifest(&id).unwrap().chunks.len(), 4);

    let report = verify(&store, &id, &clean_policy()).unwrap();
    assert!(report.hash_chain_ok);
    assert!(report.coverage_ok);
    assert_eq!(report.gaps_found, 0);
    assert_eq!(report.outcome, VerificationOutcome::Pass);
    assert!(report.authoritative, "recorded + verified => authoritative");
    assert_eq!(report.record_count, 20);
    cleanup(&root);
}

#[test]
fn flipping_one_byte_breaks_the_hash_chain() {
    let root = temp_root("flip");
    let slots: Vec<u64> = (1..=12).collect();
    let (store, id) = build_dataset(&root, &slots, 4);
    let path = store.chunk_path(&id, 1);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[10] ^= 0x01; // keep it ASCII/UTF-8; only the digest changes
    std::fs::write(&path, &bytes).unwrap();

    let report = verify(&store, &id, &clean_policy()).unwrap();
    assert!(!report.hash_chain_ok, "a flipped byte must fail integrity");
    assert_eq!(report.outcome, VerificationOutcome::Fail);
    assert!(!report.authoritative);
    cleanup(&root);
}

#[test]
fn deleting_a_chunk_fails_verification() {
    let root = temp_root("delete");
    let slots: Vec<u64> = (1..=12).collect();
    let (store, id) = build_dataset(&root, &slots, 4);
    std::fs::remove_file(store.chunk_path(&id, 2)).unwrap();

    let report = verify(&store, &id, &clean_policy()).unwrap();
    assert!(!report.hash_chain_ok, "a missing chunk must fail integrity");
    assert_eq!(report.outcome, VerificationOutcome::Fail);
    assert!(!report.authoritative);
    cleanup(&root);
}

#[test]
fn reordering_chunks_fails_verification() {
    let root = temp_root("reorder");
    let slots: Vec<u64> = (1..=12).collect();
    let (store, id) = build_dataset(&root, &slots, 4);

    // Reorder the manifest's chunk list (a tamper that keeps every hash but
    // breaks the chain order/index). The verifier must still refuse.
    let mut manifest = store.load_manifest(&id).unwrap();
    manifest.chunks.swap(0, 1);
    store.write_manifest(&manifest).unwrap();

    let report = verify(&store, &id, &clean_policy()).unwrap();
    assert!(
        !report.hash_chain_ok,
        "reordered chunks must fail integrity"
    );
    assert_eq!(report.outcome, VerificationOutcome::Fail);
    assert!(!report.authoritative);
    cleanup(&root);
}

#[test]
fn slot_gap_beyond_policy_drops_coverage_and_authority() {
    let root = temp_root("gap");
    // A large gap 5 -> 600 between consecutive known slots.
    let slots: Vec<u64> = vec![1, 2, 3, 4, 5, 600, 601, 602];
    let (store, id) = build_dataset(&root, &slots, 100);

    let policy = VerifierPolicy::new(1, 0, 10).unwrap(); // max gap 10
    let report = verify(&store, &id, &policy).unwrap();
    assert!(report.hash_chain_ok, "the chain itself is intact");
    assert!(!report.coverage_ok, "the slot gap must fail coverage");
    assert!(report.gaps_found >= 1);
    assert!(report.max_slot_gap > 10);
    assert_eq!(report.outcome, VerificationOutcome::Fail);
    assert!(
        !report.authoritative,
        "coverage shortfall forbids authority"
    );
    cleanup(&root);
}

#[test]
fn torn_trailing_line_is_detected_not_silently_accepted() {
    let root = temp_root("torn");
    let slots: Vec<u64> = (1..=6).collect();
    let (store, id) = build_dataset(&root, &slots, 100);
    // Append a partial, newline-less JSON fragment to the only chunk.
    let path = store.chunk_path(&id, 1);
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.write_all(b"{\"seq\":999,\"event\":{").unwrap();
    drop(f);

    // The verifier must produce a FAIL report (no panic, no silent pass).
    let report = verify(&store, &id, &clean_policy()).unwrap();
    assert!(!report.hash_chain_ok);
    assert_eq!(report.outcome, VerificationOutcome::Fail);
    assert!(!report.authoritative);
    cleanup(&root);
}
