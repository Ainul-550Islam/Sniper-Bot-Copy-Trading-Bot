//! BATCH 02 — replay-fixture import: all existing fixtures import and are
//! honestly labelled synthetic/non-authoritative.

use std::path::{Path, PathBuf};

use module_sniper::dataset::fixture_import::import_fixtures;
use module_sniper::dataset::format::DatasetSource;
use module_sniper::dataset::policy::VerifierPolicy;
use module_sniper::dataset::store::{DatasetStore, FsDatasetStore};
use module_sniper::dataset::verifier::verify;
use module_sniper::replay::ReplayFixture;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/replay")
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sniper-ds-imp-{}-{}-{}",
        label,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

fn cleanup(root: &PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn all_19_replay_fixtures_import_as_synthetic_and_non_authoritative() {
    let fixtures = fixtures_dir();
    // The repository ships exactly 19 replay fixtures; this pins that they
    // are all consumed (no fixture silently dropped).
    let fixture_files: Vec<_> = std::fs::read_dir(&fixtures)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .collect();
    assert_eq!(fixture_files.len(), 19, "expected 19 replay fixtures");

    let expected_steps: u64 = ReplayFixture::load_dir(&fixtures)
        .unwrap()
        .iter()
        .map(|(_, f)| f.steps.len() as u64)
        .sum();

    let root = temp_root("import");
    let manifest = import_fixtures(&fixtures, &root).unwrap();
    assert_eq!(manifest.source, DatasetSource::SyntheticFixture);
    assert!(
        !manifest.authoritative,
        "synthetic fixtures are never authoritative"
    );
    assert_eq!(
        manifest.record_count, expected_steps,
        "every fixture step imported"
    );

    // Chunk files are present and cover the whole dataset.
    let store = FsDatasetStore::new(&root);
    let chunks = store.open_chunks(&manifest.dataset_id).unwrap();
    assert!(!chunks.is_empty());
    assert!(chunks.iter().all(|c| !c.missing));
    assert_eq!(
        chunks.iter().map(|c| c.meta.record_count).sum::<u64>(),
        expected_steps
    );

    // The verifier confirms integrity but (correctly) refuses authority.
    let report = verify(&store, &manifest.dataset_id, &VerifierPolicy::default()).unwrap();
    assert!(report.hash_chain_ok);
    assert!(!report.authoritative);
    cleanup(&root);
}

#[test]
fn fixture_import_is_idempotent_and_deterministic() {
    let fixtures = fixtures_dir();
    let root = temp_root("idempotent");
    let first = import_fixtures(&fixtures, &root).unwrap();
    let second = import_fixtures(&fixtures, &root).unwrap();
    // Content-derived id: the same fixture set always maps to one dataset.
    assert_eq!(first.dataset_id, second.dataset_id);
    assert_eq!(first.record_count, second.record_count);
    assert_eq!(
        first.sha256_root, second.sha256_root,
        "byte-identical chunks -> same root"
    );

    // Re-importing the same id must not duplicate or append records.
    let store = FsDatasetStore::new(&root);
    let report = verify(&store, &first.dataset_id, &VerifierPolicy::default()).unwrap();
    assert!(report.hash_chain_ok);
    assert_eq!(report.record_count, first.record_count);
    cleanup(&root);
}
