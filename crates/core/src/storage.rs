//! Append-only JSONL persistence.
//!
//! Best-effort append-only JSONL journals. The event consumer can lag or fail;
//! flushed writes are not fsynced, so crash durability is not guaranteed.
//! Malformed lines are skipped on read. A new append separates any torn tail.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::config::StorageConfig;
use crate::error::BotResult;
use crate::events::AppEvent;
use crate::models::{Position, Trade};

#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
    trades_path: PathBuf,
    positions_path: PathBuf,
    events_path: PathBuf,
    /// Serializes operations across clones of this Store only, not across
    /// independently opened stores or processes.
    io_lock: Arc<Mutex<()>>,
}

impl Store {
    /// Create the data directory and resolve the three journal paths.
    pub async fn open(cfg: &StorageConfig) -> BotResult<Self> {
        let dir = PathBuf::from(&cfg.data_dir);
        tokio::fs::create_dir_all(&dir).await?;
        Ok(Store {
            trades_path: dir.join(&cfg.trades_file),
            positions_path: dir.join(&cfg.positions_file),
            events_path: dir.join(&cfg.events_file),
            io_lock: Arc::new(Mutex::new(())),
            dir,
        })
    }

    pub fn from_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref().to_path_buf();
        Store {
            trades_path: dir.join("trades.jsonl"),
            positions_path: dir.join("positions.jsonl"),
            events_path: dir.join("events.jsonl"),
            io_lock: Arc::new(Mutex::new(())),
            dir,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn trades_path(&self) -> &Path {
        &self.trades_path
    }

    pub fn positions_path(&self) -> &Path {
        &self.positions_path
    }

    pub fn events_path(&self) -> &Path {
        &self.events_path
    }

    async fn append<T: serde::Serialize>(&self, path: &Path, value: &T) -> BotResult<()> {
        let line = serde_json::to_string(value)?;
        let _guard = self.io_lock.lock().await;
        let mut file: File = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)
            .await?;
        // A previous crash can leave an unterminated JSON fragment. Without
        // this separator the next valid record would be merged into that
        // corrupt line and silently skipped on read too.
        if file.metadata().await?.len() != 0 {
            file.seek(std::io::SeekFrom::End(-1)).await?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last).await?;
            if last[0] != b'\n' {
                file.write_all(b"\n").await?;
            }
        }
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        Ok(())
    }

    pub async fn append_trade(&self, trade: &Trade) -> BotResult<()> {
        self.append(&self.trades_path, trade).await
    }

    pub async fn append_position(&self, position: &Position) -> BotResult<()> {
        self.append(&self.positions_path, position).await
    }

    pub async fn append_event(&self, event: &AppEvent) -> BotResult<()> {
        self.append(&self.events_path, event).await
    }

    /// Read a JSONL file back, skipping (and logging) any corrupt line.
    async fn read_jsonl<T: serde::de::DeserializeOwned>(&self, path: &Path) -> BotResult<Vec<T>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let mut out = Vec::new();
        let mut lineno = 0usize;
        while let Some(line) = lines.next_line().await? {
            lineno += 1;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<T>(line) {
                Ok(v) => out.push(v),
                Err(e) => warn!(
                    path = %path.display(),
                    lineno,
                    error = %e,
                    "skipping corrupt jsonl line"
                ),
            }
        }
        debug!(path = %path.display(), count = out.len(), "loaded journal");
        Ok(out)
    }

    pub async fn load_trades(&self) -> BotResult<Vec<Trade>> {
        self.read_jsonl(&self.trades_path).await
    }

    pub async fn load_positions(&self) -> BotResult<Vec<Position>> {
        self.read_jsonl(&self.positions_path).await
    }

    pub async fn load_events(&self) -> BotResult<Vec<AppEvent>> {
        self.read_jsonl(&self.events_path).await
    }

    /// Byte size of a journal, for the dashboard's disk-usage line.
    pub async fn size_of(&self, path: &Path) -> u64 {
        tokio::fs::metadata(path)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Rotate a journal to a timestamp- and UUID-suffixed archive. Returns
    /// the archive path, or `None` when there was nothing to rotate. The
    /// UUID makes collisions highly unlikely, not mathematically impossible.
    pub async fn rotate(&self, which: JournalKind) -> BotResult<Option<PathBuf>> {
        self.rotate_at(which, chrono::Utc::now()).await
    }

    async fn rotate_at(
        &self,
        which: JournalKind,
        at: chrono::DateTime<chrono::Utc>,
    ) -> BotResult<Option<PathBuf>> {
        let _guard = self.io_lock.lock().await;
        let path = match which {
            JournalKind::Trades => self.trades_path.clone(),
            JournalKind::Positions => self.positions_path.clone(),
            JournalKind::Events => self.events_path.clone(),
        };
        if !path.exists() || self.size_of(&path).await == 0 {
            return Ok(None);
        }
        let stamp = at.format("%Y-%m-%dT%H-%M-%SZ");
        let archive = path.with_extension(format!("jsonl.{stamp}.{}", uuid::Uuid::new_v4()));
        tokio::fs::rename(&path, &archive).await?;
        Ok(Some(archive))
    }

    /// Truncate a journal (used by `/api/admin/journal/rotate` after archiving).
    pub async fn truncate(&self, which: JournalKind) -> BotResult<()> {
        let _guard = self.io_lock.lock().await;
        let path = match which {
            JournalKind::Trades => &self.trades_path,
            JournalKind::Positions => &self.positions_path,
            JournalKind::Events => &self.events_path,
        };
        if path.exists() {
            OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(path)
                .await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    Trades,
    Positions,
    Events,
}

impl JournalKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "trades" => Some(JournalKind::Trades),
            "positions" => Some(JournalKind::Positions),
            "events" => Some(JournalKind::Events),
            _ => None,
        }
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    #[tokio::test]
    async fn rotations_in_same_second_preserve_both_archives() {
        let dir = std::env::temp_dir().join(format!("rotation-collision-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let store = Store::from_dir(&dir);
        let at = chrono::Utc::now();
        tokio::fs::write(store.trades_path(), b"first\n")
            .await
            .unwrap();
        let a = store
            .rotate_at(JournalKind::Trades, at)
            .await
            .unwrap()
            .unwrap();
        tokio::fs::write(store.trades_path(), b"second\n")
            .await
            .unwrap();
        let b = store
            .rotate_at(JournalKind::Trades, at)
            .await
            .unwrap()
            .unwrap();
        let first = tokio::fs::read(&a).await.unwrap();
        let second = tokio::fs::read(&b).await.unwrap();
        tokio::fs::remove_dir_all(&dir).await.unwrap();
        assert_ne!(
            a, b,
            "rotations at the same timestamp must not share an archive path"
        );
        assert_eq!(first, b"first\n");
        assert_eq!(second, b"second\n");
    }
}
