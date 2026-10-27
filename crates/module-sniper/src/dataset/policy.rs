//! Validated configuration for the recorder and verifier (BATCH 02).
//!
//! Every value can be supplied explicitly (for tests/libraries) or read from
//! the documented environment variables. Parsing is **fail-closed**: an
//! invalid or out-of-range value is a hard error, never a silent fallback to
//! a permissive default that could weaken verification.

use std::path::PathBuf;

use bot_core::error::{BotError, BotResult};

/// Environment variables (single source of truth for docs + tests).
pub const ENV_DATASET_DIR: &str = "DATASET_DIR";
pub const ENV_CHUNK_MAX_RECORDS: &str = "DATASET_CHUNK_MAX_RECORDS";
pub const ENV_CHUNK_MAX_BYTES: &str = "DATASET_CHUNK_MAX_BYTES";
pub const ENV_CHANNEL_CAPACITY: &str = "DATASET_CHANNEL_CAPACITY";
pub const ENV_MIN_RECORDS: &str = "DATASET_MIN_RECORDS";
pub const ENV_MIN_SLOT_SPAN: &str = "DATASET_MIN_SLOT_SPAN";
pub const ENV_MAX_SLOT_GAP: &str = "DATASET_MAX_SLOT_GAP";

/// Coverage criteria the [`crate::dataset::verifier`] enforces before it may
/// declare a dataset authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifierPolicy {
    /// Minimum number of decoded records.
    pub min_records: u64,
    /// Minimum span between the smallest and largest KNOWN slot.
    pub min_slot_span: u64,
    /// Maximum allowed gap between consecutive KNOWN slots. A larger gap is
    /// counted as a coverage defect (`gaps_found`).
    pub max_slot_gap: u64,
}

impl Default for VerifierPolicy {
    /// Permissive by default: integrity (hash chain) still ALWAYS applies;
    /// these bounds only gate the additional `coverage_ok` dimension.
    fn default() -> Self {
        VerifierPolicy {
            min_records: 1,
            min_slot_span: 0,
            max_slot_gap: u64::MAX,
        }
    }
}

impl VerifierPolicy {
    pub fn new(min_records: u64, min_slot_span: u64, max_slot_gap: u64) -> BotResult<Self> {
        let p = VerifierPolicy {
            min_records,
            min_slot_span,
            max_slot_gap,
        };
        p.validate()?;
        Ok(p)
    }

    pub fn validate(&self) -> BotResult<()> {
        if self.min_records == 0 {
            return Err(BotError::InvalidArgument(
                "DATASET_MIN_RECORDS must be >= 1".into(),
            ));
        }
        if self.max_slot_gap == 0 {
            return Err(BotError::InvalidArgument(
                "DATASET_MAX_SLOT_GAP must be >= 1".into(),
            ));
        }
        Ok(())
    }

    /// Build from the environment (missing -> safe default). Invalid values
    /// are a hard error (fail-closed).
    pub fn from_env() -> BotResult<Self> {
        let d = Self::default();
        let min_records = parse_env(ENV_MIN_RECORDS, d.min_records)?;
        let min_slot_span = parse_env(ENV_MIN_SLOT_SPAN, d.min_slot_span)?;
        let max_slot_gap = parse_env(ENV_MAX_SLOT_GAP, d.max_slot_gap)?;
        Self::new(min_records, min_slot_span, max_slot_gap)
    }
}

/// Recorder tuning: output directory, rotation thresholds and channel size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecorderConfig {
    pub dataset_dir: PathBuf,
    pub chunk_max_records: usize,
    pub chunk_max_bytes: usize,
    pub channel_capacity: usize,
}

impl RecorderConfig {
    pub fn new(
        dataset_dir: impl Into<PathBuf>,
        chunk_max_records: usize,
        chunk_max_bytes: usize,
        channel_capacity: usize,
    ) -> BotResult<Self> {
        let c = RecorderConfig {
            dataset_dir: dataset_dir.into(),
            chunk_max_records,
            chunk_max_bytes,
            channel_capacity,
        };
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> BotResult<()> {
        if self.dataset_dir.as_os_str().is_empty() {
            return Err(BotError::InvalidArgument(
                "DATASET_DIR must not be empty".into(),
            ));
        }
        if self.chunk_max_records == 0 {
            return Err(BotError::InvalidArgument(
                "DATASET_CHUNK_MAX_RECORDS must be >= 1".into(),
            ));
        }
        if self.chunk_max_bytes == 0 {
            return Err(BotError::InvalidArgument(
                "DATASET_CHUNK_MAX_BYTES must be >= 1".into(),
            ));
        }
        if self.channel_capacity == 0 {
            return Err(BotError::InvalidArgument(
                "DATASET_CHANNEL_CAPACITY must be >= 1".into(),
            ));
        }
        Ok(())
    }

    /// Config for a batch import: chunk sizes come from the environment (or
    /// safe defaults), the channel is unused (`ChunkWriter` is driven
    /// synchronously), and `dataset_dir` is supplied explicitly.
    pub fn for_import(dataset_dir: impl Into<PathBuf>) -> BotResult<Self> {
        let max_records: usize = parse_env(ENV_CHUNK_MAX_RECORDS, 1000)?;
        let max_bytes: usize = parse_env(ENV_CHUNK_MAX_BYTES, 4 * 1024 * 1024)?;
        Self::new(dataset_dir, max_records, max_bytes, 1)
    }

    /// Build from the environment. `DATASET_DIR` is required (there is no
    /// safe default that could silently write into the working tree).
    pub fn from_env() -> BotResult<Self> {
        let dir = std::env::var(ENV_DATASET_DIR)
            .map_err(|_| BotError::InvalidArgument(format!("{ENV_DATASET_DIR} is required")))?;
        let max_records: usize = parse_env(ENV_CHUNK_MAX_RECORDS, 1000)?;
        let max_bytes: usize = parse_env(ENV_CHUNK_MAX_BYTES, 4 * 1024 * 1024)?;
        let capacity: usize = parse_env(ENV_CHANNEL_CAPACITY, 1024)?;
        Self::new(dir, max_records, max_bytes, capacity)
    }
}

/// Parse `T` from an env var; `default` when unset/empty. A malformed value
/// is a hard error (never a silent fallback that could weaken a gate).
fn parse_env<T>(name: &str, default: T) -> BotResult<T>
where
    T: std::str::FromStr,
{
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => v
            .trim()
            .parse::<T>()
            .map_err(|_| BotError::InvalidArgument(format!("{name} is not a valid value: {v:?}"))),
        _ => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(VerifierPolicy::default().validate().is_ok());
    }

    #[test]
    fn invalid_verifier_policy_is_rejected() {
        assert!(VerifierPolicy::new(0, 0, 10).is_err());
        assert!(VerifierPolicy::new(1, 0, 0).is_err());
        assert!(VerifierPolicy::new(5, 10, 50).is_ok());
    }

    #[test]
    fn invalid_recorder_config_is_rejected() {
        assert!(RecorderConfig::new("/tmp/x", 0, 100, 4).is_err());
        assert!(RecorderConfig::new("/tmp/x", 10, 0, 4).is_err());
        assert!(RecorderConfig::new("/tmp/x", 10, 100, 0).is_err());
        assert!(RecorderConfig::new("", 10, 100, 4).is_err());
        assert!(RecorderConfig::new("/tmp/x", 10, 100, 4).is_ok());
    }
}
