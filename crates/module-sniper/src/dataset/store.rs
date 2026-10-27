//! SOLE owner of on-disk dataset persistence (BATCH 02).
//!
//! [`FsDatasetStore`] is the one place that knows the directory layout and
//! how bytes reach disk (atomic temp-file + rename, fsync). The
//! [`DatasetStore`] trait is a thin abstraction so the verifier / replay
//! source can read metadata without being tied to the filesystem.
//!
//! Layout:
//! ```text
//! <root>/<dataset_id>/manifest.json
//! <root>/<dataset_id>/chunk-000001.ndjson
//! <root>/<dataset_id>/chunk-000002.ndjson
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use bot_core::error::{BotError, BotResult};

use super::format::{decode_manifest, encode_manifest, ChunkManifest, DatasetManifest};

/// One chunk's metadata + bytes as read from disk. `missing` is set when the
/// file named by the manifest could not be read (deleted/corrupt path) so the
/// verifier can report a clean failure instead of erroring out.
#[derive(Debug, Clone)]
pub struct ChunkData {
    pub meta: ChunkManifest,
    pub bytes: Vec<u8>,
    pub missing: bool,
}

/// The dataset persistence contract (file-backed here; the server has a
/// separate Postgres METADATA store built on top of these same files).
pub trait DatasetStore: Send + Sync {
    fn root(&self) -> &Path;
    fn dataset_dir(&self, dataset_id: &str) -> PathBuf;
    fn chunk_path(&self, dataset_id: &str, chunk_index: u64) -> PathBuf;

    /// Create a fresh dataset directory and write its initial manifest.
    fn create(&self, manifest: &DatasetManifest) -> BotResult<()>;
    /// Atomic write of the (possibly mid-recording) manifest.
    fn write_manifest(&self, manifest: &DatasetManifest) -> BotResult<()>;
    /// Atomic FINAL write of a sealed/verified manifest.
    fn seal(&self, manifest: &DatasetManifest) -> BotResult<()>;
    /// Load + version-check the manifest.
    fn load_manifest(&self, dataset_id: &str) -> BotResult<DatasetManifest>;
    /// Atomically write one COMPLETE chunk file.
    fn append_chunk(
        &self,
        dataset_id: &str,
        chunk: &ChunkManifest,
        content: &[u8],
    ) -> BotResult<()>;
    /// Read one chunk's raw bytes.
    fn read_chunk(&self, dataset_id: &str, chunk_index: u64) -> BotResult<Vec<u8>>;
    /// Read all chunks in manifest order (marking any missing ones).
    fn open_chunks(&self, dataset_id: &str) -> BotResult<Vec<ChunkData>>;
    /// All dataset ids (directories holding a manifest), sorted.
    fn list(&self) -> BotResult<Vec<String>>;
}

/// Filesystem-backed dataset store.
#[derive(Debug, Clone)]
pub struct FsDatasetStore {
    root: PathBuf,
}

impl FsDatasetStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        FsDatasetStore { root: root.into() }
    }

    pub fn manifest_path(&self, dataset_id: &str) -> PathBuf {
        self.dataset_dir(dataset_id).join("manifest.json")
    }

    /// Open (creating if needed) a chunk file positioned for append. Returns
    /// the handle; the recorder fsyncs after each line. Used for the
    /// streaming append path, not part of the read-only trait surface.
    pub fn open_chunk_append(&self, dataset_id: &str, chunk_index: u64) -> BotResult<File> {
        let path = self.chunk_path(dataset_id, chunk_index);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        Ok(file)
    }

    /// Remove a chunk file (used to drop an empty trailing chunk on seal).
    pub fn remove_chunk(&self, dataset_id: &str, chunk_index: u64) -> BotResult<()> {
        let path = self.chunk_path(dataset_id, chunk_index);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// Truncate `path` back to its last complete newline-terminated line.
/// Returns `(new_len, complete_line_count)`. A crash mid-write leaves at most
/// one partial trailing line; dropping it makes the chunk resume-safe with no
/// duplication and no half-JSON line.
pub fn truncate_partial_line(path: &Path) -> BotResult<(u64, u64)> {
    let bytes = fs::read(path)?;
    let keep = match bytes.iter().rposition(|&b| b == b'\n') {
        Some(pos) => pos + 1,
        None => 0,
    };
    let kept = &bytes[..keep];
    let lines = kept.iter().filter(|&&b| b == b'\n').count() as u64;
    if keep != bytes.len() {
        let file = OpenOptions::new().write(true).open(path)?;
        file.set_len(keep as u64)?;
        file.sync_all()?;
    }
    Ok((keep as u64, lines))
}

/// Read a chunk file as UTF-8 text (chunk content is always valid UTF-8 when
/// produced by this crate). A non-UTF-8 file is caller-visible corruption.
pub fn read_chunk_text(path: &Path) -> BotResult<String> {
    let bytes = fs::read(path)?;
    String::from_utf8(bytes).map_err(|e| BotError::Encoding(format!("chunk is not UTF-8: {e}")))
}

/// Atomic, fsynced file write: write to a sibling temp file, sync, rename.
fn atomic_write(path: &Path, data: &[u8]) -> BotResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.to_path_buf();
    let mut name = tmp
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("chunk")
        .to_string();
    name.push_str(".tmp");
    tmp.set_file_name(name);
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    // Best-effort directory fsync so the rename itself is durable.
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

impl DatasetStore for FsDatasetStore {
    fn root(&self) -> &Path {
        &self.root
    }

    fn dataset_dir(&self, dataset_id: &str) -> PathBuf {
        self.root.join(dataset_id)
    }

    fn chunk_path(&self, dataset_id: &str, chunk_index: u64) -> PathBuf {
        self.dataset_dir(dataset_id)
            .join(format!("chunk-{chunk_index:06}.ndjson"))
    }

    fn create(&self, manifest: &DatasetManifest) -> BotResult<()> {
        fs::create_dir_all(self.dataset_dir(&manifest.dataset_id))?;
        self.write_manifest(manifest)
    }

    fn write_manifest(&self, manifest: &DatasetManifest) -> BotResult<()> {
        let text = encode_manifest(manifest)?;
        atomic_write(&self.manifest_path(&manifest.dataset_id), text.as_bytes())
    }

    fn seal(&self, manifest: &DatasetManifest) -> BotResult<()> {
        // The final manifest write is atomic; the recorder has already set
        // status/sealed_at/sha256_root on `manifest` before calling seal.
        self.write_manifest(manifest)
    }

    fn load_manifest(&self, dataset_id: &str) -> BotResult<DatasetManifest> {
        let path = self.manifest_path(dataset_id);
        let text = fs::read_to_string(&path)
            .map_err(|e| BotError::NotFound(format!("dataset manifest {}: {e}", path.display())))?;
        decode_manifest(&text)
    }

    fn append_chunk(
        &self,
        dataset_id: &str,
        chunk: &ChunkManifest,
        content: &[u8],
    ) -> BotResult<()> {
        if content.len() as u64 != chunk.byte_len {
            return Err(BotError::Encoding(format!(
                "chunk {} byte_len mismatch: manifest {} vs content {}",
                chunk.chunk_index,
                chunk.byte_len,
                content.len()
            )));
        }
        let path = self.chunk_path(dataset_id, chunk.chunk_index);
        atomic_write(&path, content)
    }

    fn read_chunk(&self, dataset_id: &str, chunk_index: u64) -> BotResult<Vec<u8>> {
        let path = self.chunk_path(dataset_id, chunk_index);
        fs::read(&path).map_err(|e| {
            BotError::NotFound(format!("chunk {} ({}): {e}", chunk_index, path.display()))
        })
    }

    fn open_chunks(&self, dataset_id: &str) -> BotResult<Vec<ChunkData>> {
        let manifest = self.load_manifest(dataset_id)?;
        let mut out = Vec::with_capacity(manifest.chunks.len());
        for meta in &manifest.chunks {
            match self.read_chunk(dataset_id, meta.chunk_index) {
                Ok(bytes) => out.push(ChunkData {
                    meta: meta.clone(),
                    bytes,
                    missing: false,
                }),
                Err(_) => out.push(ChunkData {
                    meta: meta.clone(),
                    bytes: Vec::new(),
                    missing: true,
                }),
            }
        }
        Ok(out)
    }

    fn list(&self) -> BotResult<Vec<String>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() && path.join("manifest.json").is_file() {
                if let Some(id) = entry.file_name().to_str() {
                    out.push(id.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::format::DatasetSource;

    fn tmp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sniper-ds-store-{}-{}-{}",
            label,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ))
    }

    #[test]
    fn create_load_seal_and_list_roundtrip() {
        let root = tmp_root("roundtrip");
        let store = FsDatasetStore::new(root.clone());
        let mut m = DatasetManifest::new("ds-1".into(), "test".into(), DatasetSource::Recorded);
        store.create(&m).unwrap();
        let loaded = store.load_manifest("ds-1").unwrap();
        assert_eq!(loaded.dataset_id, "ds-1");
        assert_eq!(
            loaded.status,
            crate::dataset::format::DatasetStatus::Recording
        );
        m.status = crate::dataset::format::DatasetStatus::Sealed;
        store.seal(&m).unwrap();
        assert_eq!(
            store.load_manifest("ds-1").unwrap().status,
            crate::dataset::format::DatasetStatus::Sealed
        );
        assert_eq!(store.list().unwrap(), vec!["ds-1".to_string()]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn truncate_partial_line_drops_only_the_part() {
        let root = tmp_root("trunc");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("c.ndjson");
        fs::write(&path, b"line1\nline2\npart").unwrap();
        let (len, lines) = truncate_partial_line(&path).unwrap();
        assert_eq!(len, 12);
        assert_eq!(lines, 2);
        assert_eq!(fs::read(&path).unwrap(), b"line1\nline2\n");
        let _ = fs::remove_dir_all(root);
    }
}
