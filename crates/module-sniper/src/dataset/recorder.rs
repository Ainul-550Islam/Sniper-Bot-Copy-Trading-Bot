//! Bounded, crash-safe recorder (BATCH 02).
//!
//! Producers hold a cloneable [`RecorderSink`] and enqueue [`RecordedEvent`]s
//! through a bounded `tokio::sync::mpsc`. The channel is NON-blocking:
//! [`RecorderSink::record`] uses `try_send` and, on a full/closed channel,
//! increments a **measured** `dropped` counter and returns `false` — it never
//! silently loses an event and never stalls the (money-path) producer.
//!
//! [`Recorder::run`] owns the single [`ChunkWriter`]: it appends lines to the
//! current chunk (fsync per line), rotates by record/byte thresholds, and on
//! graceful shutdown (all sinks dropped, or an explicit `seal`) finalizes the
//! hash chain and atomically writes the sealed manifest.
//!
//! [`ChunkWriter`] is public and can be used directly for recovery tests: a
//! process crash is modelled by dropping it WITHOUT sealing, leaving a
//! partially-written current chunk. [`ChunkWriter::resume`] truncates the torn
//! trailing line and continues with no loss and no duplication.

use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::mpsc;

use bot_core::error::{BotError, BotResult};

use super::format::{
    decode_line, encode_line, ChunkManifest, DatasetManifest, DatasetRecord, DatasetStatus,
    RecordedEvent,
};
use super::hashing::{hash_chunk, prev_link_for, sha256_root, GENESIS_PREV_SHA256};
use super::policy::RecorderConfig;
use super::store::{read_chunk_text, truncate_partial_line, DatasetStore, FsDatasetStore};

/// Measured counters shared between all sinks and the worker.
#[derive(Default)]
struct RecorderCounters {
    enqueued: AtomicU64,
    written: AtomicU64,
    dropped: AtomicU64,
}

/// Public, immutable snapshot of the recorder's measured counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecorderStats {
    pub enqueued: u64,
    pub written: u64,
    pub dropped: u64,
}

impl RecorderCounters {
    fn snapshot(&self) -> RecorderStats {
        RecorderStats {
            enqueued: self.enqueued.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

enum RecorderMsg {
    Record(RecordedEvent),
    Seal,
}

/// Cloneable producer side of the recorder. Bounded, non-blocking, measured.
#[derive(Clone)]
pub struct RecorderSink {
    tx: mpsc::Sender<RecorderMsg>,
    counters: Arc<RecorderCounters>,
}

impl RecorderSink {
    /// Try to enqueue `event`. Returns `true` when accepted. A full or closed
    /// channel drops the event but increments the `dropped` counter — the
    /// loss is always visible to the operator (never silent).
    pub fn record(&self, event: RecordedEvent) -> bool {
        match self.tx.try_send(RecorderMsg::Record(event)) {
            Ok(()) => {
                self.counters.enqueued.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Explicitly request a graceful seal (async send so it is reliable even
    /// when the channel is momentarily full). Dropping all sinks is an
    /// equally valid way to trigger a seal (channel close).
    pub async fn seal(&self) -> BotResult<()> {
        self.tx
            .send(RecorderMsg::Seal)
            .await
            .map_err(|_| BotError::Encoding("recorder worker stopped before seal".into()))
    }

    pub fn stats(&self) -> RecorderStats {
        self.counters.snapshot()
    }
}

/// The single-consumer recorder worker.
pub struct Recorder {
    rx: mpsc::Receiver<RecorderMsg>,
    counters: Arc<RecorderCounters>,
    writer: ChunkWriter,
}

impl Recorder {
    /// Create a NEW dataset. Returns the producer sink and the worker.
    pub fn new(
        store: FsDatasetStore,
        manifest: DatasetManifest,
        cfg: &RecorderConfig,
    ) -> BotResult<(RecorderSink, Recorder)> {
        let (tx, rx) = mpsc::channel(cfg.channel_capacity);
        let counters = Arc::new(RecorderCounters::default());
        let sink = RecorderSink {
            tx,
            counters: counters.clone(),
        };
        let writer = ChunkWriter::create(store, manifest, cfg)?;
        Ok((
            sink,
            Recorder {
                rx,
                counters,
                writer,
            },
        ))
    }

    /// RESUME an existing, still-`recording` dataset after a crash.
    pub fn resume(
        store: FsDatasetStore,
        dataset_id: &str,
        cfg: &RecorderConfig,
    ) -> BotResult<(RecorderSink, Recorder)> {
        let (tx, rx) = mpsc::channel(cfg.channel_capacity);
        let counters = Arc::new(RecorderCounters::default());
        let sink = RecorderSink {
            tx,
            counters: counters.clone(),
        };
        let writer = ChunkWriter::resume(&store, dataset_id, cfg)?;
        Ok((
            sink,
            Recorder {
                rx,
                counters,
                writer,
            },
        ))
    }

    /// Drive the worker. Seals on an explicit `Seal` message OR when every
    /// sink has been dropped (channel close) — both are graceful shutdowns.
    pub async fn run(mut self) -> BotResult<DatasetManifest> {
        while let Some(msg) = self.rx.recv().await {
            match msg {
                RecorderMsg::Record(event) => {
                    self.writer.append(event)?;
                    self.counters.written.fetch_add(1, Ordering::Relaxed);
                }
                RecorderMsg::Seal => break,
            }
        }
        self.writer.seal()
    }

    /// Spawn `run` on the tokio runtime. `RecorderTask::finish` yields the
    /// sealed manifest. The caller keeps its [`RecorderSink`] clone(s).
    pub fn spawn(self) -> RecorderTask {
        RecorderTask {
            handle: tokio::spawn(self.run()),
        }
    }
}

/// Handle to a spawned recorder worker.
pub struct RecorderTask {
    handle: tokio::task::JoinHandle<BotResult<DatasetManifest>>,
}

impl RecorderTask {
    pub async fn finish(self) -> BotResult<DatasetManifest> {
        self.handle
            .await
            .map_err(|e| BotError::Encoding(format!("recorder task join: {e}")))?
    }
}

/// In-progress chunk being appended on disk.
struct CurrentChunk {
    index: u64,
    file: File,
    first_slot: u64,
    last_slot: u64,
    record_count: u64,
    byte_len: u64,
    prev_sha256: String,
}

impl CurrentChunk {
    fn new(index: u64, file: File, prev_sha256: String) -> Self {
        CurrentChunk {
            index,
            file,
            first_slot: 0,
            last_slot: 0,
            record_count: 0,
            byte_len: 0,
            prev_sha256,
        }
    }
}

/// Synchronous, crash-safe chunk writer. Owns the append-only file for the
/// current chunk, rotates by records/bytes, computes the hash chain, and
/// atomically writes the manifest on seal. Public for recovery tests.
pub struct ChunkWriter {
    store: FsDatasetStore,
    manifest: DatasetManifest,
    current: Option<CurrentChunk>,
    next_seq: u64,
    cfg: RecorderConfig,
}

impl ChunkWriter {
    /// Start a FRESH dataset (clean slate for this dataset id).
    pub fn create(
        store: FsDatasetStore,
        mut manifest: DatasetManifest,
        cfg: &RecorderConfig,
    ) -> BotResult<Self> {
        let dir = store.dataset_dir(&manifest.dataset_id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        manifest.status = DatasetStatus::Recording;
        manifest.chunks.clear();
        manifest.record_count = 0;
        manifest.sha256_root.clear();
        manifest.sealed_at = None;
        manifest.authoritative = false;
        store.create(&manifest)?;
        let file = store.open_chunk_append(&manifest.dataset_id, 1)?;
        Ok(ChunkWriter {
            store,
            manifest,
            current: Some(CurrentChunk::new(1, file, GENESIS_PREV_SHA256.to_string())),
            next_seq: 0,
            cfg: cfg.clone(),
        })
    }

    /// Reopen a `recording` dataset after a crash: truncate the torn trailing
    /// line of the current chunk and rebuild its state from the complete
    /// lines. No record is lost and none is duplicated.
    pub fn resume(
        store: &FsDatasetStore,
        dataset_id: &str,
        cfg: &RecorderConfig,
    ) -> BotResult<Self> {
        let manifest = store.load_manifest(dataset_id)?;
        if manifest.status != DatasetStatus::Recording {
            return Err(BotError::InvalidArgument(format!(
                "cannot resume dataset {dataset_id} with status {:?}",
                manifest.status
            )));
        }
        let finalized: u64 = manifest.chunks.iter().map(|c| c.record_count).sum();
        let last_hash = manifest.chunks.last().map(|c| c.sha256.clone());
        let index = manifest.chunks.len() as u64 + 1;
        let path = store.chunk_path(dataset_id, index);

        let (current, next_seq) = if path.exists() {
            truncate_partial_line(&path)?;
            let content = read_chunk_text(&path)?;
            let mut cur = CurrentChunk::new(index, store.open_chunk_append(dataset_id, index)?, {
                prev_link_for(index, last_hash.as_deref())
            });
            let mut max_seq: Option<u64> = None;
            for line in content.lines() {
                if line.is_empty() {
                    continue;
                }
                let rec = decode_line(line)?;
                if cur.record_count == 0 {
                    cur.first_slot = rec.slot();
                }
                cur.last_slot = rec.slot();
                cur.record_count += 1;
                cur.byte_len += line.len() as u64 + 1; // + trailing '\n'
                max_seq = Some(rec.seq);
            }
            (cur, max_seq.map(|s| s + 1).unwrap_or(finalized))
        } else {
            (
                CurrentChunk::new(
                    index,
                    store.open_chunk_append(dataset_id, index)?,
                    prev_link_for(index, last_hash.as_deref()),
                ),
                finalized,
            )
        };

        Ok(ChunkWriter {
            store: store.clone(),
            manifest,
            current: Some(current),
            next_seq,
            cfg: cfg.clone(),
        })
    }

    /// Append one event as a single NDJSON line (fsynced), rotating first if
    /// the current chunk hit a records/bytes threshold. The capture slot is
    /// taken from the event itself.
    pub fn append(&mut self, event: RecordedEvent) -> BotResult<()> {
        let slot = event.slot();
        self.append_with_slot(slot, event)
    }

    /// Append one event with an explicit capture slot (used by the fixture
    /// importer and by tests to lay out a clean, deterministic slot order).
    pub fn append_with_slot(&mut self, slot: u64, event: RecordedEvent) -> BotResult<()> {
        let record = DatasetRecord::with_slot(self.next_seq, slot, event);
        let line = encode_line(&record)?;
        let bytes = line.as_bytes();
        let line_len = bytes.len() as u64;

        let need_rotate = match &self.current {
            Some(cur) => {
                cur.record_count >= self.cfg.chunk_max_records as u64
                    || (cur.record_count > 0
                        && cur.byte_len + line_len > self.cfg.chunk_max_bytes as u64)
            }
            None => true,
        };
        if need_rotate {
            self.rotate()?;
        }

        let cur = self
            .current
            .as_mut()
            .ok_or_else(|| BotError::Encoding("recorder has no current chunk".into()))?;
        if cur.record_count == 0 {
            cur.first_slot = record.slot();
        }
        cur.last_slot = record.slot();
        cur.file.write_all(bytes)?;
        cur.file.sync_all()?;
        cur.record_count += 1;
        cur.byte_len += line_len;
        self.next_seq += 1;
        Ok(())
    }

    /// Finalize the current chunk, persist progress, open the next chunk.
    fn rotate(&mut self) -> BotResult<()> {
        let last_hash = if self.current.is_some() {
            let h = self.finalize_current()?;
            self.store.write_manifest(&self.manifest)?;
            Some(h)
        } else {
            self.manifest.chunks.last().map(|c| c.sha256.clone())
        };
        let index = self.manifest.chunks.len() as u64 + 1;
        let file = self
            .store
            .open_chunk_append(&self.manifest.dataset_id, index)?;
        self.current = Some(CurrentChunk::new(
            index,
            file,
            prev_link_for(index, last_hash.as_deref()),
        ));
        Ok(())
    }

    /// Compute the current chunk's hash from its exact bytes and push its
    /// [`ChunkManifest`] onto the manifest. Returns the hash.
    fn finalize_current(&mut self) -> BotResult<String> {
        let cur = self
            .current
            .take()
            .ok_or_else(|| BotError::Encoding("finalize with no current chunk".into()))?;
        cur.file.sync_all()?;
        drop(cur.file);
        let path = self.store.chunk_path(&self.manifest.dataset_id, cur.index);
        let content = read_chunk_text(&path)?;
        let hash = hash_chunk(&content);
        let chunk = ChunkManifest {
            chunk_index: cur.index,
            first_slot: cur.first_slot,
            last_slot: cur.last_slot,
            record_count: cur.record_count,
            byte_len: content.len() as u64,
            sha256: hash.clone(),
            prev_sha256: cur.prev_sha256,
        };
        self.manifest.chunks.push(chunk);
        Ok(hash)
    }

    /// Finalize the last chunk, compute `sha256_root`, set status `sealed`,
    /// and atomically write the manifest. `authoritative` stays false — only
    /// the verifier may decide it.
    pub fn seal(mut self) -> BotResult<DatasetManifest> {
        match &self.current {
            Some(cur) if cur.record_count == 0 => {
                let idx = cur.index;
                self.current = None;
                self.store.remove_chunk(&self.manifest.dataset_id, idx)?;
            }
            Some(_) => {
                self.finalize_current()?;
            }
            None => {}
        }

        let hashes: Vec<String> = self
            .manifest
            .chunks
            .iter()
            .map(|c| c.sha256.clone())
            .collect();
        self.manifest.sha256_root = sha256_root(&hashes);
        self.manifest.record_count = self.manifest.chunks.iter().map(|c| c.record_count).sum();
        if let (Some(first), Some(last)) =
            (self.manifest.chunks.first(), self.manifest.chunks.last())
        {
            self.manifest.slot_start = first.first_slot;
            self.manifest.slot_end = last.last_slot;
        }
        self.manifest.status = DatasetStatus::Sealed;
        self.manifest.sealed_at = Some(Utc::now());
        self.manifest.authoritative = false;
        self.store.seal(&self.manifest)?;
        Ok(self.manifest)
    }
}
