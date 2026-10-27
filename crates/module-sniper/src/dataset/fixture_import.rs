//! Convert the checked-in replay fixtures into a HONEST synthetic dataset
//! (BATCH 02).
//!
//! The `fixtures/replay/*.json` files are generated scenarios, not real
//! market data. `import_fixtures` therefore produces a dataset whose
//! `source` is ALWAYS [`DatasetSource::SyntheticFixture`] and whose
//! `authoritative` is ALWAYS false. The verifier (which is the sole decider
//! of authority) will never mark it authoritative, no matter how clean the
//! hash chain or coverage. This is the labelling honesty rule made concrete.

use std::path::Path;

use bot_core::auth::sha256_hex;
use bot_core::error::{BotError, BotResult};

use crate::replay::ReplayFixture;

use super::format::{DatasetManifest, DatasetSource, RecordedEvent};
use super::policy::RecorderConfig;
use super::recorder::ChunkWriter;
use super::store::FsDatasetStore;

/// Deterministic UUID (v5-shaped) derived from a 64-char hex digest. The
/// same fixture set always yields the same dataset id, which keeps replay
/// reproducible and makes the output path discoverable.
fn uuid_from_hash(hex: &str) -> String {
    let mut b = [0u8; 16];
    for (i, slot) in b.iter_mut().enumerate() {
        let byte = hex.get(i * 2..i * 2 + 2).unwrap_or("00");
        *slot = u8::from_str_radix(byte, 16).unwrap_or(0);
    }
    b[6] = (b[6] & 0x0f) | 0x50; // version 5
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
    )
}

/// Import every `*.json` replay fixture in `fixtures_dir` into ONE sealed,
/// honestly-labelled `synthetic_fixture` dataset under `out_dir`.
///
/// Returns the sealed manifest. The dataset id is derived from the fixture
/// content, so `<out_dir>/<dataset_id>/` is deterministic.
pub fn import_fixtures(fixtures_dir: &Path, out_dir: &Path) -> BotResult<DatasetManifest> {
    let fixtures = ReplayFixture::load_dir(fixtures_dir)?;
    if fixtures.is_empty() {
        return Err(BotError::InvalidArgument(format!(
            "no replay fixtures found in {}",
            fixtures_dir.display()
        )));
    }

    // Deterministic dataset id from the (sorted, canonical) fixture content.
    let mut h_input = String::new();
    let mut total_steps = 0u64;
    for (name, fixture) in &fixtures {
        h_input.push_str(name);
        h_input.push('\n');
        h_input.push_str(&fixture.to_json()?);
        h_input.push('\n');
        total_steps += fixture.steps.len() as u64;
    }
    let dataset_id = uuid_from_hash(&sha256_hex(&h_input));

    let store = FsDatasetStore::new(out_dir);
    let cfg = RecorderConfig::for_import(out_dir)?;
    let mut manifest = DatasetManifest::new(
        dataset_id,
        "replay-fixtures".into(),
        DatasetSource::SyntheticFixture,
    );
    manifest.cluster = "synthetic".into();

    let mut writer = ChunkWriter::create(store.clone(), manifest, &cfg)?;
    // Assign clean, monotonic capture slots so the importer produces a
    // dataset whose COVERAGE is well-formed. Coverage passing does NOT make
    // it authoritative: the source label blocks that in the verifier.
    let mut slot = 1u64;
    for (_name, fixture) in &fixtures {
        for step in &fixture.steps {
            writer.append_with_slot(slot, RecordedEvent::launch(step.event.clone()))?;
            slot += 1;
        }
    }
    let sealed = writer.seal()?;
    debug_assert_eq!(sealed.record_count, total_steps);
    Ok(sealed)
}
