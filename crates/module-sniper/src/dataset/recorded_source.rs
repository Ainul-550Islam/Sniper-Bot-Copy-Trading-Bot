//! Deterministic replay from a recorded dataset (BATCH 02).
//!
//! ADAPTATION NOTE: at this repository HEAD there is no pre-existing
//! "replay event-source" trait — `replay::ReplayEngine::run` consumes a
//! `ReplayFixture` directly. This module therefore defines the source
//! contract ([`ReplayEventSource`]) here and yields the real
//! [`crate::event::LaunchEvent`] values the existing engine consumes, in a
//! deterministic (slot, index) order. A caller can build a `ReplayFixture`
//! from [`RecordedReplaySource::launch_events`] and feed `ReplayEngine`.
//!
//! Every output is stamped with its honest `data_source` (`recorded` for a
//! real capture and `synthetic_fixture` for an imported fixture), the
//! `dataset_id`, and `authoritative` COPIED FROM THE VERIFIER OUTCOME
//! ([`VerificationReport::authoritative`]) — a dataset's own flag is never
//! trusted. [`RecordedReplaySource::authoritative_replay`] refuses any
//! dataset the verifier did not pass.

use std::path::Path;

use bot_core::auth::sha256_hex;
use bot_core::error::{BotError, BotResult};
use serde_json::json;

use crate::event::LaunchEvent;

use super::format::{decode_line, DatasetManifest, DatasetRecord, RecordedEvent};
use super::policy::VerifierPolicy;
use super::store::ChunkData;
use super::verifier::{load_verified_dir, VerificationOutcome, VerificationReport};

/// The replay source contract (see module docs for the adaptation note).
pub trait ReplayEventSource {
    fn dataset_id(&self) -> &str;
    fn data_source(&self) -> &str;
    fn authoritative(&self) -> bool;
    /// Launch events in deterministic (slot, seq) order.
    fn launch_events(&self) -> Vec<LaunchEvent>;
}

/// The stamped output of one replay.
#[derive(Debug, Clone)]
pub struct ReplayOutput {
    pub data_source: String,
    pub dataset_id: String,
    pub authoritative: bool,
    /// Deterministically ordered launch events.
    pub events: Vec<LaunchEvent>,
}

impl ReplayOutput {
    /// A stable hash of the whole output. Same dataset replayed twice yields
    /// the identical hash (used by `dataset-tool replay-hash` and the
    /// determinism test). Stamps are part of the hash domain so an
    /// authoritative vs non-authoritative replay cannot collide.
    pub fn output_hash(&self) -> BotResult<String> {
        let mut acc = String::from("replay-output/v1\n");
        acc.push_str(&format!(
            "{}\n{}\n{}\n{}\n",
            self.data_source,
            self.dataset_id,
            self.authoritative,
            self.events.len()
        ));
        for event in &self.events {
            let line = serde_json::to_string(event)
                .map_err(|e| BotError::Encoding(format!("replay event encode: {e}")))?;
            acc.push_str(&line);
            acc.push('\n');
        }
        Ok(sha256_hex(&acc))
    }
}

/// A replay source backed by a verified (or at least verified-attempted)
/// recorded dataset directory.
pub struct RecordedReplaySource {
    manifest: DatasetManifest,
    report: VerificationReport,
    records: Vec<DatasetRecord>,
}

impl RecordedReplaySource {
    /// Load `dataset_dir`, run the verifier (fresh, never trusting labels),
    /// decode all records and sort them by (slot, seq).
    pub fn open(dataset_dir: &Path, policy: &VerifierPolicy) -> BotResult<Self> {
        let (manifest, chunks, report) = load_verified_dir(dataset_dir, policy)?;
        Self::from_verified_snapshot(manifest, chunks, report)
    }

    fn from_verified_snapshot(
        manifest: DatasetManifest,
        chunks: Vec<ChunkData>,
        report: VerificationReport,
    ) -> BotResult<Self> {
        let mut records: Vec<DatasetRecord> = Vec::new();
        for chunk in &chunks {
            if chunk.missing {
                continue;
            }
            let content = String::from_utf8(chunk.bytes.clone()).map_err(|e| {
                BotError::Encoding(format!(
                    "chunk {} is not UTF-8: {e}",
                    chunk.meta.chunk_index
                ))
            })?;
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                records.push(decode_line(line)?);
            }
        }
        records.sort_by(|a, b| a.slot.cmp(&b.slot).then(a.seq.cmp(&b.seq)));

        Ok(RecordedReplaySource {
            manifest,
            report,
            records,
        })
    }

    pub fn manifest(&self) -> &DatasetManifest {
        &self.manifest
    }

    pub fn report(&self) -> &VerificationReport {
        &self.report
    }

    /// Build the stamped, deterministic output. Works for ANY dataset; the
    /// `authoritative` stamp is exactly the verifier's verdict (so a
    /// synthetic_fixture dataset always yields `false`). The source stamp is
    /// equally strict: real captures are `recorded`, while imported fixtures
    /// remain `synthetic_fixture` even though this same reader supports them.
    pub fn output(&self) -> ReplayOutput {
        ReplayOutput {
            data_source: self.manifest.source.as_str().into(),
            dataset_id: self.manifest.dataset_id.clone(),
            authoritative: self.report.authoritative,
            events: self.launch_events(),
        }
    }

    /// Authoritative replay: REFUSE any dataset the verifier did not pass
    /// (equivalent to "status != verified"). A `synthetic_fixture` dataset
    /// can never satisfy this, even if its hash chain is pristine.
    pub fn authoritative_replay(&self) -> BotResult<ReplayOutput> {
        if self.report.outcome != VerificationOutcome::Pass || !self.report.authoritative {
            let detail = json!({
                "dataset_id": self.manifest.dataset_id,
                "source": self.manifest.source.as_str(),
                "verifier_outcome": self.report.outcome.as_str(),
                "hash_chain_ok": self.report.hash_chain_ok,
                "coverage_ok": self.report.coverage_ok,
                "authoritative": self.report.authoritative,
            });
            return Err(BotError::Unauthorized(format!(
                "authoritative replay refused: dataset was not verified by the verifier: {detail}"
            )));
        }
        Ok(self.output())
    }
}

impl ReplayEventSource for RecordedReplaySource {
    fn dataset_id(&self) -> &str {
        &self.manifest.dataset_id
    }

    fn data_source(&self) -> &str {
        self.manifest.source.as_str()
    }

    fn authoritative(&self) -> bool {
        self.report.authoritative
    }

    fn launch_events(&self) -> Vec<LaunchEvent> {
        self.records
            .iter()
            .filter_map(|r| match &r.event {
                RecordedEvent::Launch(e) => Some(e.as_ref().clone()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::dataset::fixture_import::import_fixtures;
    use crate::dataset::format::DatasetSource;
    use crate::dataset::store::{DatasetStore, FsDatasetStore};
    use crate::dataset::verifier::verify;

    #[test]
    fn replay_uses_the_verified_bytes_after_the_files_change() {
        let root = std::env::temp_dir().join(format!(
            "sniper-snapshot-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/replay");
        let store = FsDatasetStore::new(&root);
        let mut manifest = import_fixtures(&fixtures, &root).unwrap();
        // Test-only label, not an attestation of real-world fixture provenance.
        manifest.source = DatasetSource::Recorded;
        store.write_manifest(&manifest).unwrap();
        let policy = VerifierPolicy::default();
        let (snapshot_manifest, chunks, report) =
            load_verified_dir(&root.join(&manifest.dataset_id), &policy).unwrap();
        assert!(report.authoritative);
        let expected_records = report.record_count;

        // A replacement after verification must not replace replay's bytes.
        std::fs::write(
            store.chunk_path(&manifest.dataset_id, 1),
            b"unverified replacement\n",
        )
        .unwrap();
        let source =
            RecordedReplaySource::from_verified_snapshot(snapshot_manifest, chunks, report)
                .unwrap();
        let reread_report = verify(&store, &manifest.dataset_id, &policy).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert!(
            !reread_report.hash_chain_ok,
            "disk replacement control must be corrupt"
        );
        assert_eq!(source.launch_events().len() as u64, expected_records);
        assert!(
            source.authoritative_replay().unwrap().authoritative,
            "the retained, verified ORIGINAL snapshot must remain usable"
        );
    }
}
