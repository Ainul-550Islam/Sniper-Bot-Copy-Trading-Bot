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

#[test]
fn replay_rejects_a_manifest_redirect_to_another_dataset() {
    let root = temp_root("manifest-redirect");
    let store = FsDatasetStore::new(&root);
    let mut manifest = import_fixtures(&fixtures_dir(), &root).unwrap();
    // Test-only recorded label: this checks the binding of the verifier's
    // input bytes to replay, not real-world provenance of fixture content.
    manifest.source = DatasetSource::Recorded;
    store.write_manifest(&manifest).unwrap();
    let policy = VerifierPolicy::default();
    assert!(
        verify(&store, &manifest.dataset_id, &policy)
            .unwrap()
            .authoritative
    );

    let redirected_id = "unverified-other-dataset";
    let cfg = RecorderConfig::new(&root, 1000, 4096, 1).unwrap();
    let other = DatasetManifest::new(
        redirected_id.into(),
        "synthetic destination".into(),
        DatasetSource::SyntheticFixture,
    );
    let mut writer = ChunkWriter::create(store.clone(), other, &cfg).unwrap();
    writer
        .append_with_slot(
            1,
            RecordedEvent::PoolPriceTick {
                slot: 1,
                mint: "test-only".into(),
                tick: Tick {
                    t_ms: 0,
                    price_sol: 1.0,
                    quote_reserve_lamports: 1,
                },
            },
        )
        .unwrap();
    writer.seal().unwrap();
    assert!(
        !verify(&store, redirected_id, &policy)
            .unwrap()
            .authoritative
    );

    let original_id = manifest.dataset_id.clone();
    manifest.dataset_id = redirected_id.into();
    // Deliberately tamper the original manifest file, not the destination.
    std::fs::write(
        store.manifest_path(&original_id),
        module_sniper::dataset::format::encode_manifest(&manifest).unwrap(),
    )
    .unwrap();
    let result = RecordedReplaySource::open(&root.join(&original_id), &policy);
    cleanup(&root);
    assert!(result.is_err(), "a manifest redirect must not replay another dataset under the first dataset's successful verification");
}

#[test]
fn dataset_create_rejects_a_parent_directory_escape_before_writing() {
    let base = temp_root("path-escape");
    let root = base.join("allowed");
    let escaped = base.join("escaped");
    let store = FsDatasetStore::new(&root);
    let manifest = DatasetManifest::new(
        "../escaped".into(),
        "escape probe".into(),
        DatasetSource::SyntheticFixture,
    );
    let result = store.create(&manifest);
    let wrote_outside = escaped.join("manifest.json").exists();
    cleanup(&base);
    assert!(
        result.is_err(),
        "dataset ids must not escape the configured root"
    );
    assert!(
        !wrote_outside,
        "rejection must occur before any outside-root write"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_verifier_policy_is_rejected_not_silently_defaulted() {
    use module_sniper::dataset::policy::ENV_MIN_RECORDS;
    use std::os::unix::ffi::OsStringExt;
    // No other case in this integration executable reads ENV_MIN_RECORDS.
    let previous = std::env::var_os(ENV_MIN_RECORDS);
    std::env::set_var(ENV_MIN_RECORDS, std::ffi::OsString::from_vec(vec![0xff]));
    let result = VerifierPolicy::from_env();
    match previous {
        Some(value) => std::env::set_var(ENV_MIN_RECORDS, value),
        None => std::env::remove_var(ENV_MIN_RECORDS),
    }
    assert!(
        result.is_err(),
        "non-UTF-8 policy input must not fall back to the permissive defaults"
    );
}

#[test]
fn all_dataset_store_io_rejects_path_ids_but_keeps_normal_ids_working() {
    use module_sniper::dataset::format::ChunkManifest;
    let base = temp_root("all-path-io");
    let store = FsDatasetStore::new(base.join("allowed"));
    let absolute = base.join("absolute-escape").to_string_lossy().to_string();
    let chunk = ChunkManifest {
        chunk_index: 1,
        first_slot: 0,
        last_slot: 0,
        record_count: 0,
        byte_len: 0,
        sha256: String::new(),
        prev_sha256: String::new(),
    };
    for id in [
        "",
        ".",
        "..",
        "../escape",
        "nested/child",
        "nested\\child",
        absolute.as_str(),
    ] {
        let manifest = DatasetManifest::new(
            id.into(),
            "invalid path".into(),
            DatasetSource::SyntheticFixture,
        );
        assert!(store.create(&manifest).is_err(), "create accepted {id:?}");
        assert!(
            store.write_manifest(&manifest).is_err(),
            "write accepted {id:?}"
        );
        assert!(store.seal(&manifest).is_err(), "seal accepted {id:?}");
        assert!(
            store.open_chunk_append(id, 1).is_err(),
            "append handle accepted {id:?}"
        );
        assert!(
            store.append_chunk(id, &chunk, b"").is_err(),
            "chunk write accepted {id:?}"
        );
        assert!(
            store.read_chunk(id, 1).is_err(),
            "chunk read accepted {id:?}"
        );
        assert!(
            store.load_manifest(id).is_err(),
            "manifest read accepted {id:?}"
        );
        assert!(store.open_chunks(id).is_err(), "chunk list accepted {id:?}");
        assert!(store.remove_chunk(id, 1).is_err(), "remove accepted {id:?}");
    }
    assert!(
        !base.exists(),
        "reject invalid paths before creating directories"
    );
    let normal = DatasetManifest::new(
        "normal-id_1".into(),
        "valid control".into(),
        DatasetSource::SyntheticFixture,
    );
    store.create(&normal).unwrap();
    assert_eq!(
        store.load_manifest("normal-id_1").unwrap().dataset_id,
        "normal-id_1"
    );
    cleanup(&base);
}

#[test]
fn audit_writer_rejects_parent_escape_before_deleting_existing_files() {
    let base = temp_root("writer-delete-escape");
    let root = base.join("allowed");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let sentinel = outside.join("must-survive.txt");
    std::fs::write(&sentinel, b"keep this external file").unwrap();
    let cfg = RecorderConfig::new(&root, 1, 4096, 1).unwrap();
    let manifest = DatasetManifest::new(
        "../outside".into(),
        "deletion probe".into(),
        DatasetSource::SyntheticFixture,
    );
    let result = ChunkWriter::create(FsDatasetStore::new(&root), manifest, &cfg);
    let retained = std::fs::read(&sentinel).ok();
    cleanup(&base);
    assert!(result.is_err(), "writer must reject path traversal");
    assert_eq!(
        retained.as_deref(),
        Some(b"keep this external file".as_slice()),
        "writer must validate BEFORE its clean-slate remove_dir_all"
    );
}

#[test]
fn audit_invalid_recorder_configuration_returns_errors_not_panics() {
    use module_sniper::dataset::recorder::Recorder;
    let root = temp_root("invalid-recorder-config");
    for capacity in [0, usize::MAX] {
        let mut cfg = RecorderConfig::new(&root, 1, 4096, 1).unwrap();
        cfg.channel_capacity = capacity;
        let manifest = DatasetManifest::new(
            "normal-id".into(),
            "config probe".into(),
            DatasetSource::SyntheticFixture,
        );
        let create =
            std::panic::catch_unwind(|| Recorder::new(FsDatasetStore::new(&root), manifest, &cfg));
        let resume = std::panic::catch_unwind(|| {
            Recorder::resume(FsDatasetStore::new(&root), "normal-id", &cfg)
        });
        cleanup(&root);
        assert!(
            create.is_ok(),
            "invalid public config must not panic in Recorder::new"
        );
        assert!(create.unwrap().is_err());
        assert!(
            resume.is_ok(),
            "invalid public config must not panic in Recorder::resume"
        );
        assert!(resume.unwrap().is_err());
    }
}
