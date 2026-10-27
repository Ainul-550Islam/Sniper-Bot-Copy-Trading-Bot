//! BATCH 02 — recorder: rotation boundaries, crash resume, measured drops,
//! graceful shutdown sealing.

use std::path::PathBuf;

use module_sniper::backtest::dataset::Tick;
use module_sniper::dataset::format::{
    decode_line, DatasetManifest, DatasetSource, DatasetStatus, RecordedEvent,
};
use module_sniper::dataset::hashing::GENESIS_PREV_SHA256;
use module_sniper::dataset::policy::RecorderConfig;
use module_sniper::dataset::recorder::{ChunkWriter, Recorder};
use module_sniper::dataset::store::{DatasetStore, FsDatasetStore};

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sniper-ds-rec-{}-{}-{}",
        label,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ))
}

fn cleanup(root: &PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

/// A simple, distinct event with an explicit capture `slot`.
fn tick(slot: u64) -> RecordedEvent {
    RecordedEvent::PoolPriceTick {
        slot,
        mint: "Mint1111111111111111111111111111111111111111".into(),
        tick: Tick {
            t_ms: slot,
            price_sol: 1.0 + slot as f64 * 1e-9,
            quote_reserve_lamports: 1_000_000 + slot,
        },
    }
}

#[test]
fn rotation_boundaries_are_exact() {
    let root = temp_root("rotation");
    let store = FsDatasetStore::new(&root);
    let cfg = RecorderConfig::new(&root, 5, 4 * 1024 * 1024, 16).unwrap();
    let m = DatasetManifest::new("ds-rot".into(), "rot".into(), DatasetSource::Recorded);
    let mut w = ChunkWriter::create(store.clone(), m, &cfg).unwrap();
    for i in 0..10u64 {
        w.append(tick(i + 1)).unwrap();
    }
    let sealed = w.seal().unwrap();

    assert_eq!(sealed.record_count, 10);
    assert_eq!(sealed.chunks.len(), 2, "exactly 2 full chunks");
    assert_eq!(sealed.chunks[0].record_count, 5);
    assert_eq!(sealed.chunks[1].record_count, 5);
    assert_eq!(sealed.chunks[0].chunk_index, 1);
    assert_eq!(sealed.chunks[1].chunk_index, 2);
    assert_eq!(sealed.chunks[0].prev_sha256, GENESIS_PREV_SHA256);
    assert_eq!(sealed.chunks[1].prev_sha256, sealed.chunks[0].sha256);

    let c1 = store.read_chunk("ds-rot", 1).unwrap();
    let c2 = store.read_chunk("ds-rot", 2).unwrap();
    assert_eq!(c1.iter().filter(|&&b| b == b'\n').count(), 5);
    assert_eq!(c2.iter().filter(|&&b| b == b'\n').count(), 5);
    cleanup(&root);
}

#[test]
fn rotation_by_bytes_is_honoured() {
    let root = temp_root("byte-rotation");
    let store = FsDatasetStore::new(&root);
    // Tiny byte budget forces several chunks despite a large record budget.
    let cfg = RecorderConfig::new(&root, 1000, 350, 16).unwrap();
    let m = DatasetManifest::new("ds-byte".into(), "byte".into(), DatasetSource::Recorded);
    let mut w = ChunkWriter::create(store, m, &cfg).unwrap();
    for i in 0..12u64 {
        w.append(tick(i + 1)).unwrap();
    }
    let sealed = w.seal().unwrap();
    assert_eq!(sealed.record_count, 12);
    assert!(
        sealed.chunks.len() >= 2,
        "byte cap must create multiple chunks"
    );
    assert_eq!(
        sealed.chunks.iter().map(|c| c.record_count).sum::<u64>(),
        12
    );
    cleanup(&root);
}

#[test]
fn restart_mid_chunk_resumes_without_loss_or_duplication() {
    let root = temp_root("resume");
    let store = FsDatasetStore::new(&root);
    let cfg = RecorderConfig::new(&root, 5, 4 * 1024 * 1024, 16).unwrap();
    let id = "ds-resume";

    // Phase 1: write 8 events -> chunk1 sealed at rotation, chunk2 in
    // progress with 3 lines. "Crash" = drop WITHOUT sealing.
    {
        let m = DatasetManifest::new(id.into(), "resume".into(), DatasetSource::Recorded);
        let mut w = ChunkWriter::create(store.clone(), m, &cfg).unwrap();
        for i in 0..8u64 {
            w.append(tick(i + 1)).unwrap();
        }
        drop(w);
    }
    let mid = store.load_manifest(id).unwrap();
    assert_eq!(mid.status, DatasetStatus::Recording);
    assert_eq!(mid.chunks.len(), 1, "only the rotated chunk is finalized");
    assert_eq!(mid.chunks[0].record_count, 5);

    // Simulate a torn trailing line on the in-progress chunk2.
    let c2path = store.chunk_path(id, 2);
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&c2path)
            .unwrap();
        f.write_all(b"{\"schema_version\":1,\"seq\":8,\"slot\":9,\"event\":{\"kin")
            .unwrap();
    }

    // Phase 2: resume (truncates torn line), append 7 more, seal.
    {
        let mut w = ChunkWriter::resume(&store, id, &cfg).unwrap();
        for i in 8..15u64 {
            w.append(tick(i + 1)).unwrap();
        }
        let sealed = w.seal().unwrap();
        assert_eq!(sealed.record_count, 15);
        assert_eq!(sealed.status, DatasetStatus::Sealed);
    }

    // Every seq 0..15 exactly once — no loss, no duplication.
    let chunks = store.open_chunks(id).unwrap();
    let mut seqs = Vec::new();
    for ch in &chunks {
        assert!(!ch.missing);
        let content = String::from_utf8(ch.bytes.clone()).unwrap();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            seqs.push(decode_line(line).unwrap().seq);
        }
    }
    seqs.sort();
    assert_eq!(seqs, (0..15).collect::<Vec<u64>>());
    cleanup(&root);
}

#[tokio::test]
async fn drop_counters_are_measured_under_a_full_channel() {
    let root = temp_root("drops");
    let store = FsDatasetStore::new(&root);
    let cfg = RecorderConfig::new(&root, 1000, 4 * 1024 * 1024, 2).unwrap();
    let m = DatasetManifest::new("ds-drop".into(), "drop".into(), DatasetSource::Recorded);
    let (sink, recorder) = Recorder::new(store, m, &cfg).unwrap();

    // Worker is NOT running: the bounded channel (capacity 2) fills, the
    // remaining 8 records are dropped and MEASURED.
    let results: Vec<bool> = (0..10).map(|i| sink.record(tick(i + 1))).collect();
    assert_eq!(results.iter().filter(|&&r| r).count(), 2);
    assert_eq!(results.iter().filter(|&&r| !r).count(), 8);
    let stats = sink.stats();
    assert_eq!(stats.enqueued, 2);
    assert_eq!(stats.dropped, 8);
    assert_eq!(stats.written, 0);

    // Dropping the only sink closes the channel -> graceful seal of the 2
    // accepted records.
    drop(sink);
    let sealed = recorder.run().await.unwrap();
    assert_eq!(sealed.record_count, 2);
    cleanup(&root);
}

#[tokio::test]
async fn graceful_shutdown_seals() {
    let root = temp_root("seal");
    let store = FsDatasetStore::new(&root);
    let cfg = RecorderConfig::new(&root, 1000, 4 * 1024 * 1024, 32).unwrap();
    let m = DatasetManifest::new("ds-seal".into(), "seal".into(), DatasetSource::Recorded);
    let (sink, recorder) = Recorder::new(store.clone(), m, &cfg).unwrap();

    for i in 0..7u64 {
        assert!(sink.record(tick(i + 1)), "channel should accept all 7");
    }
    sink.seal().await.unwrap();
    let sealed = recorder.run().await.unwrap();

    assert_eq!(sealed.status, DatasetStatus::Sealed);
    assert_eq!(sealed.record_count, 7);
    assert!(sealed.sealed_at.is_some());
    assert!(
        !sealed.authoritative,
        "verifier, not recorder, decides authority"
    );
    let on_disk = store.load_manifest("ds-seal").unwrap();
    assert_eq!(on_disk.status, DatasetStatus::Sealed);
    assert_eq!(on_disk.record_count, 7);
    cleanup(&root);
}

#[tokio::test]
async fn explicit_seal_drains_accepted_records_and_measures_late_drops() {
    let root = temp_root("seal-drain");
    let store = FsDatasetStore::new(&root);
    let cfg = RecorderConfig::new(&root, 1000, 4 * 1024 * 1024, 16).unwrap();
    let m = DatasetManifest::new(
        "ds-seal-drain".into(),
        "seal-drain".into(),
        DatasetSource::Recorded,
    );
    let (sink, recorder) = Recorder::new(store, m, &cfg).unwrap();

    for i in 0..3u64 {
        assert!(sink.record(tick(i + 1)));
    }
    sink.seal().await.unwrap();
    // These sends are queued behind the seal marker because the worker has
    // not started yet. They were accepted, so a graceful seal must write them
    // rather than dropping the receiver's remaining queue.
    for i in 3..5u64 {
        assert!(sink.record(tick(i + 1)));
    }

    let sealed = recorder.run().await.unwrap();
    assert_eq!(sealed.record_count, 5);
    assert_eq!(sink.stats().written, 5);
    assert_eq!(sink.stats().enqueued, 5);
    assert_eq!(sink.stats().dropped, 0);

    // The worker closed the channel at the seal marker. A later producer
    // fails explicitly and the attempted loss is visible in the counters.
    assert!(!sink.record(tick(6)));
    let stats = sink.stats();
    assert_eq!(stats.enqueued, 5);
    assert_eq!(stats.written, 5);
    assert_eq!(stats.dropped, 1);
    cleanup(&root);
}
