//! SOLE owner of the on-disk dataset format (BATCH 02).
//!
//! Everything that becomes bytes on disk — the NDJSON line envelope, the
//! [`RecordedEvent`] enum, the manifest and the chunk manifest — is defined
//! here. Serialization is deterministic: structs keep a fixed field order,
//! numbers are integers, timestamps are RFC3339 UTC strings, and no map with
//! non-deterministic iteration order is ever serialized. `serde_json` emits
//! struct fields in declaration order, so `encode_line(record)` is byte-stable
//! for a given `record`.
//!
//! Strict versioning: both the manifest and every line carry
//! [`DATASET_SCHEMA_VERSION`]; decoding rejects any unknown version.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use bot_core::error::{BotError, BotResult};

use crate::backtest::dataset::Tick;
use crate::event::LaunchEvent;
use crate::risk_intel::bundler::FirstSlotBuy;

use super::DATASET_SCHEMA_VERSION;

/// Where a dataset's data came from. A self-declared LABEL — the verifier is
/// the only thing that turns it into a trust decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetSource {
    /// Captured from a real configured feed (the only source that CAN be
    /// authoritative, and only after the verifier proves it).
    Recorded,
    /// Converted from the checked-in `fixtures/replay/*.json` files. Never
    /// real market data; never authoritative.
    SyntheticFixture,
}

impl DatasetSource {
    pub fn as_str(self) -> &'static str {
        match self {
            DatasetSource::Recorded => "recorded",
            DatasetSource::SyntheticFixture => "synthetic_fixture",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recorded" => Some(DatasetSource::Recorded),
            "synthetic_fixture" => Some(DatasetSource::SyntheticFixture),
            _ => None,
        }
    }
}

/// Lifecycle status of a dataset. A LABEL persisted for humans/DB; the
/// verifier never trusts it — it recomputes the verdict from the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasetStatus {
    Recording,
    Sealed,
    Verified,
    Failed,
}

impl DatasetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DatasetStatus::Recording => "recording",
            DatasetStatus::Sealed => "sealed",
            DatasetStatus::Verified => "verified",
            DatasetStatus::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recording" => Some(DatasetStatus::Recording),
            "sealed" => Some(DatasetStatus::Sealed),
            "verified" => Some(DatasetStatus::Verified),
            "failed" => Some(DatasetStatus::Failed),
            _ => None,
        }
    }
}

/// The kinds of observations the recorder can persist.
///
/// Adjacently tagged (`kind` + `data`) so every variant form is representable
/// and the encoding is unambiguous and stable. Real existing domain types are
/// reused wherever they are `Serialize + Deserialize`:
/// * [`LaunchEvent`] — the normalised launch from every detection feed;
/// * [`FirstSlotBuy`] — the attributed first-slot buys (risk_intel::bundler);
/// * [`Tick`] — a backtest price tick (backtest::dataset).
///
/// `DevSell` mirrors the fields of `risk_intel::DevSellObservation` (which is
/// not serde-derived), expressed as serde-friendly primitives; conversion
/// helpers below keep the two in sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RecordedEvent {
    /// A normalised launch event (the primary replay payload). Boxed:
    /// `LaunchEvent` is ~672 bytes, which would otherwise inflate every
    /// `RecordedEvent` / channel message.
    Launch(Box<LaunchEvent>),
    /// First-slot buys attributed to a mint (bundler signal).
    FirstSlotBuys {
        slot: u64,
        mint: String,
        buys: Vec<FirstSlotBuy>,
    },
    /// A creator/dev sell observation.
    DevSell {
        slot: u64,
        mint: String,
        seller: String,
        observed_at: DateTime<Utc>,
    },
    /// A pool price tick on the launch's price path.
    PoolPriceTick { slot: u64, mint: String, tick: Tick },
}

impl RecordedEvent {
    /// Ergonomic constructor for the boxed launch variant.
    pub fn launch(event: LaunchEvent) -> Self {
        RecordedEvent::Launch(Box::new(event))
    }

    /// Chain slot of this observation. `0` means the source did not report a
    /// slot (unknown) — the verifier ignores unknown slots for gap detection,
    /// matching `event::SequenceTracker`'s "0 means unknown" convention.
    pub fn slot(&self) -> u64 {
        match self {
            RecordedEvent::Launch(e) => e.slot.unwrap_or(0),
            RecordedEvent::FirstSlotBuys { slot, .. }
            | RecordedEvent::DevSell { slot, .. }
            | RecordedEvent::PoolPriceTick { slot, .. } => *slot,
        }
    }

    /// Stable discriminant used in telemetry / hash domains.
    pub fn kind(&self) -> &'static str {
        match self {
            RecordedEvent::Launch(_) => "launch",
            RecordedEvent::FirstSlotBuys { .. } => "first_slot_buys",
            RecordedEvent::DevSell { .. } => "dev_sell",
            RecordedEvent::PoolPriceTick { .. } => "pool_price_tick",
        }
    }

    /// Build a `DevSell` record from the real `DevSellObservation` type.
    pub fn from_dev_sell_observation(
        obs: crate::risk_intel::DevSellObservation,
        slot: u64,
    ) -> Self {
        RecordedEvent::DevSell {
            slot,
            mint: obs.mint.to_string(),
            seller: obs.seller.to_string(),
            observed_at: obs.observed_at,
        }
    }
}

/// The per-line NDJSON envelope. One physical line == one `DatasetRecord`.
/// `seq` is the dataset-wide monotonic index used for (slot, seq) ordering
/// and replay determinism. `slot` is the CAPTURE slot used by the verifier
/// for gap/overlap/order checks; it carries a value even when the embedded
/// event has no native slot (`0` = unknown). Keeping it in the envelope lets
/// the importer lay out synthetic fixtures in a clean monotonic slot order
/// without mutating the recorded event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetRecord {
    pub schema_version: u32,
    pub seq: u64,
    pub slot: u64,
    pub event: RecordedEvent,
}

impl DatasetRecord {
    /// Build a record whose capture slot is the event's native slot.
    pub fn new(seq: u64, event: RecordedEvent) -> Self {
        let slot = event.slot();
        DatasetRecord::with_slot(seq, slot, event)
    }

    /// Build a record with an explicit capture slot.
    pub fn with_slot(seq: u64, slot: u64, event: RecordedEvent) -> Self {
        DatasetRecord {
            schema_version: DATASET_SCHEMA_VERSION,
            seq,
            slot,
            event,
        }
    }

    pub fn slot(&self) -> u64 {
        self.slot
    }
}

/// Metadata for one chunk file. `sha256` is the hash of the chunk's exact
/// bytes; `prev_sha256` is the `sha256` of the previous chunk (genesis for
/// the first). See `hashing` for the chain rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkManifest {
    pub chunk_index: u64,
    pub first_slot: u64,
    pub last_slot: u64,
    pub record_count: u64,
    pub byte_len: u64,
    pub sha256: String,
    pub prev_sha256: String,
}

/// The dataset manifest — the single metadata file, written atomically.
/// `authoritative` here is a cached LABEL only; the verifier's report is the
/// authority. `chunks` is ordered by `chunk_index`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub schema_version: u32,
    pub dataset_id: String,
    pub name: String,
    pub source: DatasetSource,
    pub cluster: String,
    pub slot_start: u64,
    pub slot_end: u64,
    pub record_count: u64,
    pub sha256_root: String,
    pub status: DatasetStatus,
    pub authoritative: bool,
    pub created_at: DateTime<Utc>,
    pub sealed_at: Option<DateTime<Utc>>,
    pub chunks: Vec<ChunkManifest>,
}

impl DatasetManifest {
    /// A fresh, empty, non-authoritative manifest. Callers fill `name`,
    /// `source`, `cluster` and `dataset_id` via the struct fields.
    pub fn new(dataset_id: String, name: String, source: DatasetSource) -> Self {
        DatasetManifest {
            schema_version: DATASET_SCHEMA_VERSION,
            dataset_id,
            name,
            source,
            cluster: String::new(),
            slot_start: 0,
            slot_end: 0,
            record_count: 0,
            sha256_root: String::new(),
            status: DatasetStatus::Recording,
            authoritative: false,
            created_at: Utc::now(),
            sealed_at: None,
            chunks: Vec::new(),
        }
    }

    /// Validate strict versioning. Never accept a foreign schema.
    pub fn check_version(&self) -> BotResult<()> {
        if self.schema_version != DATASET_SCHEMA_VERSION {
            return Err(BotError::Encoding(format!(
                "unsupported dataset manifest schema_version {} (expected {})",
                self.schema_version, DATASET_SCHEMA_VERSION
            )));
        }
        Ok(())
    }
}

/// Encode one record as a single NDJSON line (trailing `\n` included).
/// Rejects a mismatched schema version so a bad producer cannot write lines
/// the verifier would silently accept.
pub fn encode_line(record: &DatasetRecord) -> BotResult<String> {
    if record.schema_version != DATASET_SCHEMA_VERSION {
        return Err(BotError::Encoding(format!(
            "unsupported dataset line schema_version {} (expected {})",
            record.schema_version, DATASET_SCHEMA_VERSION
        )));
    }
    let mut line = serde_json::to_string(record)
        .map_err(|e| BotError::Encoding(format!("dataset line encode: {e}")))?;
    line.push('\n');
    Ok(line)
}

/// Decode one NDJSON line. Rejects unknown schema versions (strict).
pub fn decode_line(line: &str) -> BotResult<DatasetRecord> {
    let body = line.trim_end_matches(['\n', '\r']);
    if body.is_empty() {
        return Err(BotError::Encoding("empty dataset line".into()));
    }
    let record: DatasetRecord = serde_json::from_str(body)
        .map_err(|e| BotError::Encoding(format!("dataset line decode: {e}")))?;
    if record.schema_version != DATASET_SCHEMA_VERSION {
        return Err(BotError::Encoding(format!(
            "unsupported dataset line schema_version {} (expected {})",
            record.schema_version, DATASET_SCHEMA_VERSION
        )));
    }
    Ok(record)
}

/// Serialize the manifest deterministically (pretty, fixed field order).
pub fn encode_manifest(manifest: &DatasetManifest) -> BotResult<String> {
    manifest.check_version()?;
    serde_json::to_string_pretty(manifest)
        .map_err(|e| BotError::Encoding(format!("dataset manifest encode: {e}")))
}

/// Parse + version-check a manifest.
pub fn decode_manifest(text: &str) -> BotResult<DatasetManifest> {
    let manifest: DatasetManifest = serde_json::from_str(text)
        .map_err(|e| BotError::Encoding(format!("dataset manifest decode: {e}")))?;
    manifest.check_version()?;
    Ok(manifest)
}
