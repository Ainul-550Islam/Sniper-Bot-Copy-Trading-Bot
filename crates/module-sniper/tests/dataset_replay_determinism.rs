//! BATCH 02 — deterministic replay + labelling honesty.

use std::path::{Path, PathBuf};

use module_sniper::backtest::dataset::Tick;
use module_sniper::dataset::fixture_import::import_fixtures;
use module_sniper::dataset::format::{
    DatasetManifest, DatasetSource, DatasetStatus, RecordedEvent,
};
use module_sniper::dataset::policy::{RecorderConfig, VerifierPolicy};
use module_sniper::dataset::recorded_source::{RecordedReplaySource, ReplayEventSource};
use module_sniper::dataset::recorder::ChunkWriter;
use module_sniper::dataset::store::{DatasetStore, FsDatasetStore};
use module_sniper::dataset::verifier::verify;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/replay")
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sniper-ds-rep-{}-{}-{}",
        label,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

fn cleanup(root: &PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn replaying_the_same_dataset_twice_is_byte_identical() {
    let root = temp_root("determinism");
    let manifest = import_fixtures(&fixtures_dir(), &root).unwrap();
    let dir = root.join(&manifest.dataset_id);
    let policy = VerifierPolicy::default();

    let a = RecordedReplaySource::open(&dir, &policy).unwrap();
    let b = RecordedReplaySource::open(&dir, &policy).unwrap();
    let ha = a.output().output_hash().unwrap();
    let hb = b.output().output_hash().unwrap();
    assert_eq!(ha, hb, "replaying the same dataset must be deterministic");
    assert_eq!(a.launch_events().len(), manifest.record_count as usize);
    cleanup(&root);
}

#[test]
fn synthetic_fixture_can_never_be_authoritative_even_if_it_claims_to_be() {
    let root = temp_root("honesty");
    let manifest = import_fixtures(&fixtures_dir(), &root).unwrap();
    let dir = root.join(&manifest.dataset_id);
    let store = FsDatasetStore::new(&root);

    // The honest, freshly imported dataset.
    let report = verify(&store, &manifest.dataset_id, &VerifierPolicy::default()).unwrap();
    assert_eq!(manifest.source, DatasetSource::SyntheticFixture);
    assert!(!manifest.authoritative);
    assert!(
        !report.authoritative,
        "a synthetic fixture must never be authoritative"
    );

    let source = RecordedReplaySource::open(&dir, &VerifierPolicy::default()).unwrap();
    let output = source.output();
    assert!(!output.authoritative);
    assert_eq!(
        output.data_source, "synthetic_fixture",
        "fixture replay must never be stamped as recorded market data"
    );
    assert_eq!(source.data_source(), "synthetic_fixture");
    assert!(
        source.authoritative_replay().is_err(),
        "authoritative replay must be refused"
    );

    // Now FORGE the self-declared flags: claim verified + authoritative.
    // The verifier must still refuse: it never trusts a self-declared flag.
    let mut forged = store.load_manifest(&manifest.dataset_id).unwrap();
    forged.status = DatasetStatus::Verified;
    forged.authoritative = true;
    store.write_manifest(&forged).unwrap();

    let report2 = verify(&store, &manifest.dataset_id, &VerifierPolicy::default()).unwrap();
    assert!(
        !report2.authoritative,
        "a self-declared authoritative=true must be ignored by the verifier"
    );
    let source2 = RecordedReplaySource::open(&dir, &VerifierPolicy::default()).unwrap();
    assert!(!source2.report().authoritative);
    assert!(source2.authoritative_replay().is_err());
    cleanup(&root);
}

#[test]
fn a_verified_recorded_dataset_can_be_replayed_authoritatively() {
    let root = temp_root("recorded");
    let store = FsDatasetStore::new(&root);
    let cfg = RecorderConfig::new(&root, 1000, 4 * 1024 * 1024, 1).unwrap();
    let id = "ds-recorded";
    let m = DatasetManifest::new(id.into(), "recorded".into(), DatasetSource::Recorded);
    let mut w = ChunkWriter::create(store.clone(), m, &cfg).unwrap();
    for i in 0..12u64 {
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
    w.seal().unwrap();

    let dir = root.join(id);
    let source =
        RecordedReplaySource::open(&dir, &VerifierPolicy::new(1, 0, 1000).unwrap()).unwrap();
    assert!(source.report().hash_chain_ok);
    assert!(source.report().coverage_ok);
    assert!(source.authoritative());
    let out = source.authoritative_replay().unwrap();
    assert!(out.authoritative);
    assert_eq!(out.data_source, "recorded");
    assert_eq!(out.dataset_id, id);
    cleanup(&root);
}
