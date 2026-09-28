//! Append-only JSONL persistence.
//!
//! A trading bot that loses its history is useless for post-mortems, so every
//! fill, position change and event is appended to a `.jsonl` file the moment it
//! happens. JSONL is chosen deliberately: it survives a crash mid-write (only
//! the last line is lost) and it can be replayed with `jq`.

use std::path::{Path, PathBuf};

use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
            dir,
        })
    }

    pub fn from_dir(dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref().to_path_buf();
        Store {
            trades_path: dir.join("trades.jsonl"),
            positions_path: dir.join("positions.jsonl"),
            events_path: dir.join("events.jsonl"),
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
        let mut file: File = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
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

    /// Rotate a journal: `trades.jsonl` -> `trades.jsonl.2026-09-12T10:00:00Z`.
    /// Returns the archive path, or `None` when there was nothing to rotate.
    pub async fn rotate(&self, which: JournalKind) -> BotResult<Option<PathBuf>> {
        let path = match which {
            JournalKind::Trades => self.trades_path.clone(),
            JournalKind::Positions => self.positions_path.clone(),
            JournalKind::Events => self.events_path.clone(),
        };
        if !path.exists() || self.size_of(&path).await == 0 {
            return Ok(None);
        }
        let stamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ");
        let archive = path.with_extension(format!("jsonl.{stamp}"));
        tokio::fs::rename(&path, &archive).await?;
        Ok(Some(archive))
    }

    /// Truncate a journal (used by `/api/admin/journal/rotate` after archiving).
    pub async fn truncate(&self, which: JournalKind) -> BotResult<()> {
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
