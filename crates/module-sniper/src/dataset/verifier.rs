//! SOLE decider of `authoritative` (BATCH 02).
//!
//! `verify()` NEVER trusts the manifest's self-declared `status`, `source` or
//! `authoritative` flags. It recomputes:
//! * the hash chain — every chunk's SHA-256, each `prev_sha256` link, the
//!   `sha256_root`, strict chunk ordering and byte/record counts;
//! * the coverage — slot gaps, overlaps and out-of-order records against a
//!   [`VerifierPolicy`] (min records, min slot span, max allowed slot gap).
//!
//! `authoritative` is `true` **only** when `hash_chain_ok && coverage_ok &&
//! source == Recorded`. Synthetic fixtures are permanently non-authoritative.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::error::{BotError, BotResult};

use super::format::DatasetManifest;
use super::format::{decode_line, DatasetSource};
use super::hashing::{constant_time_eq, hash_chunk, sha256_root, GENESIS_PREV_SHA256};
use super::policy::VerifierPolicy;
use super::store::{DatasetStore, FsDatasetStore};
use super::VERIFIER_VERSION;

/// The verifier's pass/fail verdict for one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationOutcome {
    Pass,
    Fail,
}

impl VerificationOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            VerificationOutcome::Pass => "pass",
            VerificationOutcome::Fail => "fail",
        }
    }
}

/// The full, self-contained result of a verification run. Maps 1:1 onto the
/// `recorded_launch_dataset_verifications` row (plus a few convenience fields
/// the metadata store needs to update the dataset row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerificationReport {
    pub dataset_id: String,
    pub source: DatasetSource,
    pub verifier_version: String,
    pub hash_chain_ok: bool,
    pub gaps_found: i32,
    /// Largest observed gap between consecutive known slots (bigint in DB).
    pub max_slot_gap: i64,
    pub coverage_ok: bool,
    pub outcome: VerificationOutcome,
    /// ONLY this field (computed here) may ever be trusted as authoritative.
    pub authoritative: bool,
    pub detail: serde_json::Value,
    // Convenience fields (also mirrored into `detail`).
    pub record_count: u64,
    pub chunk_count: u64,
    pub slot_start: u64,
    pub slot_end: u64,
    pub sha256_root: String,
}

/// Verify a dataset by id against `store` using `policy`.
pub fn verify(
    store: &dyn DatasetStore,
    dataset_id: &str,
    policy: &VerifierPolicy,
) -> BotResult<VerificationReport> {
    policy.validate()?;
    let manifest = store.load_manifest(dataset_id)?;
    let chunks = store.open_chunks(dataset_id)?;

    let mut hash_chain_ok = true;
    let mut problems: Vec<String> = Vec::new();
    let mut root_hashes: Vec<String> = Vec::with_capacity(chunks.len());
    let mut expected_prev = GENESIS_PREV_SHA256.to_string();
    let mut slots: Vec<u64> = Vec::new();
    let mut decoded_records: u64 = 0;
    let mut manifest_chunk_records: u64 = 0;

    for (position, chunk) in chunks.iter().enumerate() {
        manifest_chunk_records += chunk.meta.record_count;
        let expected_index = position as u64 + 1;
        if chunk.meta.chunk_index != expected_index {
            hash_chain_ok = false;
            problems.push(format!(
                "chunk order broken: expected index {expected_index}, found {}",
                chunk.meta.chunk_index
            ));
        }
        if chunk.missing {
            hash_chain_ok = false;
            problems.push(format!("chunk {} file is missing", chunk.meta.chunk_index));
            continue;
        }

        let content = match String::from_utf8(chunk.bytes.clone()) {
            Ok(c) => c,
            Err(e) => {
                hash_chain_ok = false;
                problems.push(format!(
                    "chunk {} is not valid UTF-8: {e}",
                    chunk.meta.chunk_index
                ));
                continue;
            }
        };

        let recomputed = hash_chunk(&content);
        if !constant_time_eq(&recomputed, &chunk.meta.sha256) {
            hash_chain_ok = false;
            problems.push(format!("chunk {} sha256 mismatch", chunk.meta.chunk_index));
        }
        if !constant_time_eq(&chunk.meta.prev_sha256, &expected_prev) {
            hash_chain_ok = false;
            problems.push(format!(
                "chunk {} prev_sha256 link mismatch",
                chunk.meta.chunk_index
            ));
        }
        if chunk.meta.byte_len != content.len() as u64 {
            hash_chain_ok = false;
            problems.push(format!(
                "chunk {} byte_len mismatch",
                chunk.meta.chunk_index
            ));
        }
        // The chain advances on the ACTUAL computed hash, not the stored label.
        expected_prev = recomputed.clone();
        root_hashes.push(recomputed);

        // Decode lines for coverage + format integrity.
        let mut line_count = 0u64;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match decode_line(line) {
                Ok(rec) => {
                    slots.push(rec.slot);
                    line_count += 1;
                    decoded_records += 1;
                }
                Err(e) => {
                    hash_chain_ok = false;
                    problems.push(format!(
                        "chunk {} has an undecodable line: {e}",
                        chunk.meta.chunk_index
                    ));
                    line_count += 1;
                }
            }
        }
        if line_count != chunk.meta.record_count {
            hash_chain_ok = false;
            problems.push(format!(
                "chunk {} record_count mismatch (manifest {}, lines {})",
                chunk.meta.chunk_index, chunk.meta.record_count, line_count
            ));
        }
    }

    if manifest.chunks.len() != chunks.len() {
        hash_chain_ok = false;
        problems.push("manifest chunk count differs from files".into());
    }
    let computed_root = sha256_root(&root_hashes);
    if !constant_time_eq(&computed_root, &manifest.sha256_root) {
        hash_chain_ok = false;
        problems.push("sha256_root mismatch".into());
    }
    if manifest.record_count != manifest_chunk_records {
        hash_chain_ok = false;
        problems.push("manifest record_count differs from chunk sum".into());
    }

    // ---- Coverage: slot gaps / overlaps / out-of-order ---------------------
    let known: Vec<u64> = slots.iter().copied().filter(|&s| s > 0).collect();
    let (slot_start, slot_end, slot_span) = if known.is_empty() {
        (0u64, 0u64, 0u64)
    } else {
        let min = *known.iter().min().unwrap_or(&0);
        let max = *known.iter().max().unwrap_or(&0);
        (min, max, max.saturating_sub(min))
    };
    let mut gaps_found = 0i32;
    let mut observed_max_gap = 0u64;
    let mut out_of_order = false;
    for pair in known.windows(2) {
        let (prev, cur) = (pair[0], pair[1]);
        if cur < prev {
            out_of_order = true;
            continue;
        }
        let gap = cur - prev; // 0 = same slot (allowed)
        if gap > observed_max_gap {
            observed_max_gap = gap;
        }
        if gap > policy.max_slot_gap {
            gaps_found += 1;
        }
    }
    // Chunk-level overlap: a chunk whose first known slot precedes the
    // previous chunk's last known slot.
    let mut overlap = false;
    let mut prev_chunk_last: Option<u64> = None;
    for chunk in &chunks {
        let first = if chunk.meta.first_slot > 0 {
            Some(chunk.meta.first_slot)
        } else {
            None
        };
        let last = if chunk.meta.last_slot > 0 {
            Some(chunk.meta.last_slot)
        } else {
            None
        };
        if let (Some(p), Some(f)) = (prev_chunk_last, first) {
            if f < p {
                overlap = true;
            }
        }
        if let Some(l) = last {
            prev_chunk_last = Some(l);
        }
    }

    let record_count = decoded_records;
    let coverage_ok = hash_chain_ok
        && record_count >= policy.min_records
        && slot_span >= policy.min_slot_span
        && gaps_found == 0
        && !out_of_order
        && !overlap;

    let outcome = if hash_chain_ok && coverage_ok {
        VerificationOutcome::Pass
    } else {
        VerificationOutcome::Fail
    };
    // Honesty rule: source is a label, never trusted for integrity. Even a
    // fully verified synthetic fixture is NEVER authoritative.
    let authoritative =
        outcome == VerificationOutcome::Pass && manifest.source == DatasetSource::Recorded;

    let detail = json!({
        "problems": problems,
        "observed_max_slot_gap": observed_max_gap,
        "slot_span": slot_span,
        "out_of_order": out_of_order,
        "overlap": overlap,
        "decoded_records": record_count,
        "chunk_count": chunks.len(),
        "policy": {
            "min_records": policy.min_records,
            "min_slot_span": policy.min_slot_span,
            "max_slot_gap": policy.max_slot_gap,
        },
        "computed_root": computed_root,
    });

    Ok(VerificationReport {
        dataset_id: dataset_id.to_string(),
        source: manifest.source,
        verifier_version: VERIFIER_VERSION.to_string(),
        hash_chain_ok,
        gaps_found,
        max_slot_gap: observed_max_gap as i64,
        coverage_ok,
        outcome,
        authoritative,
        detail,
        record_count,
        chunk_count: chunks.len() as u64,
        slot_start,
        slot_end,
        sha256_root: manifest.sha256_root.clone(),
    })
}

/// Convenience for the CLI / replay source: verify a dataset DIRECTORY
/// (`<root>/<dataset_id>/`) using an [`FsDatasetStore`] rooted at its parent.
pub fn verify_dir(
    dataset_dir: &Path,
    policy: &VerifierPolicy,
) -> BotResult<(DatasetManifest, VerificationReport)> {
    let root = dataset_dir.parent().unwrap_or_else(|| Path::new("."));
    let id = dataset_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| BotError::InvalidArgument("dataset dir has no id".into()))?;
    let store = FsDatasetStore::new(root);
    let manifest = store.load_manifest(id)?;
    let report = verify(&store, id, policy)?;
    Ok((manifest, report))
}
