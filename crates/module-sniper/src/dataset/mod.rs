//! Recorded launch datasets: recorder + store + verifier + replay (BATCH 02).
//!
//! ## Honesty rules (non-negotiable)
//! * A dataset carries a [`format::DatasetSource`]. Synthetic fixtures are
//!   **always** labelled `synthetic_fixture` and can **never** be
//!   `authoritative`, no matter what any flag says.
//! * `authoritative` is decided by exactly one module — [`verifier`] — as
//!   `hash_chain_ok && coverage_ok && source == recorded`. No self-declared
//!   `authoritative`/`status` field is ever trusted by the verifier.
//! * The on-disk byte format is owned by [`format`] only; the hash chain by
//!   [`hashing`] only; file persistence by [`store`] only. Other modules call
//!   into these; they never re-implement encoding, hashing or path layout.
//!
//! ## Layout (owned by [`store::FsDatasetStore`])
//! ```text
//! <root>/<dataset_id>/manifest.json
//! <root>/<dataset_id>/chunk-000001.ndjson
//! <root>/<dataset_id>/chunk-000002.ndjson
//! ...
//! ```
//!
//! Data flow: [`recorder`] appends NDJSON lines to chunk files (fsync +
//! rotation), [`verifier`] recomputes the chain and decides authoritative,
//! [`recorded_source`] replays the verified events deterministically, and
//! [`fixture_import`] labels the existing replay fixtures honestly.

pub mod fixture_import;
pub mod format;
pub mod hashing;
pub mod policy;
pub mod recorded_source;
pub mod recorder;
pub mod store;
pub mod verifier;

/// On-disk schema version. The line codec and the manifest both reject any
/// other value (strict versioning, forward/backward incompatibility is a hard
/// error rather than a silent misread).
pub const DATASET_SCHEMA_VERSION: u32 = 1;

/// Human-readable verifier identity, persisted in every verification row so
/// an audit can tell which policy/code produced a verdict.
pub const VERIFIER_VERSION: &str = "dataset-verifier/1";

pub use format::{
    ChunkManifest, DatasetManifest, DatasetRecord, DatasetSource, DatasetStatus, RecordedEvent,
};
pub use policy::{RecorderConfig, VerifierPolicy};
pub use recorded_source::{RecordedReplaySource, ReplayOutput};
pub use recorder::{Recorder, RecorderSink, RecorderStats};
pub use store::{ChunkData, DatasetStore, FsDatasetStore};
pub use verifier::{VerificationOutcome, VerificationReport};
