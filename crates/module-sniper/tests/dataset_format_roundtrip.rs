//! BATCH 02 — on-disk format round-trip, byte stability, strict versioning.

use std::path::Path;

use chrono::Utc;
use module_sniper::backtest::dataset::Tick;
use module_sniper::dataset::format::{
    decode_line, decode_manifest, encode_line, encode_manifest, DatasetManifest, DatasetRecord,
    DatasetSource, RecordedEvent,
};
use module_sniper::replay::ReplayFixture;
use module_sniper::risk_intel::bundler::FirstSlotBuy;
use solana_sdk::pubkey::Pubkey;

fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/replay")
}

/// A REAL LaunchEvent, loaded from a checked-in replay fixture.
fn launch_event() -> module_sniper::event::LaunchEvent {
    let path = fixtures_dir().join("01_valid_launch.json");
    let fixture = ReplayFixture::load(&path).expect("load replay fixture");
    fixture
        .steps
        .first()
        .expect("fixture has a step")
        .event
        .clone()
}

fn all_variants() -> Vec<RecordedEvent> {
    vec![
        RecordedEvent::launch(launch_event()),
        RecordedEvent::FirstSlotBuys {
            slot: 42,
            mint: "Mint1111111111111111111111111111111111111111".into(),
            buys: vec![
                FirstSlotBuy {
                    buyer: Pubkey::new_from_array([1u8; 32]),
                    funder: Pubkey::new_from_array([9u8; 32]),
                },
                FirstSlotBuy {
                    buyer: Pubkey::new_from_array([2u8; 32]),
                    funder: Pubkey::new_from_array([9u8; 32]),
                },
            ],
        },
        RecordedEvent::DevSell {
            slot: 43,
            mint: "Mint1111111111111111111111111111111111111111".into(),
            seller: "Creator1111111111111111111111111111111111111".into(),
            observed_at: Utc::now(),
        },
        RecordedEvent::PoolPriceTick {
            slot: 44,
            mint: "Mint1111111111111111111111111111111111111111".into(),
            tick: Tick {
                t_ms: 1000,
                price_sol: 3.1e-8,
                quote_reserve_lamports: 30_500_000_000,
            },
        },
    ]
}

#[test]
fn every_variant_roundtrips_byte_stably() {
    for (i, event) in all_variants().into_iter().enumerate() {
        let slot = event.slot();
        let record = DatasetRecord::with_slot(i as u64, slot, event);
        let line = encode_line(&record).expect("encode");
        assert!(line.ends_with('\n'), "line must be newline-terminated");
        // Deterministic: same input -> identical bytes.
        let line2 = encode_line(&record).expect("encode again");
        assert_eq!(line, line2, "encoding must be byte-stable");
        // Round-trip: decode -> re-encode is byte-identical (stable field
        // order, exact numeric/timestamp round-trip).
        let decoded = decode_line(&line).expect("decode");
        assert_eq!(decoded.seq, i as u64);
        assert_eq!(decoded.slot, slot);
        assert_eq!(decoded.event.kind(), record.event.kind());
        let re_encoded = encode_line(&decoded).expect("re-encode");
        assert_eq!(
            line,
            re_encoded,
            "variant {} not byte-stable",
            record.event.kind()
        );
    }
}

#[test]
fn new_derives_capture_slot_from_event() {
    let e = RecordedEvent::PoolPriceTick {
        slot: 777,
        mint: "m".into(),
        tick: Tick {
            t_ms: 1,
            price_sol: 1.0,
            quote_reserve_lamports: 10,
        },
    };
    let rec = DatasetRecord::new(0, e);
    assert_eq!(rec.slot(), 777);
    assert_eq!(
        rec.schema_version,
        module_sniper::dataset::DATASET_SCHEMA_VERSION
    );
}

#[test]
fn unknown_schema_version_is_rejected() {
    // Line encode rejects a foreign version.
    let mut record = DatasetRecord::new(
        0,
        RecordedEvent::PoolPriceTick {
            slot: 1,
            mint: "m".into(),
            tick: Tick {
                t_ms: 1,
                price_sol: 1.0,
                quote_reserve_lamports: 10,
            },
        },
    );
    record.schema_version = 999;
    assert!(
        encode_line(&record).is_err(),
        "foreign version must not encode"
    );

    // Line decode rejects a foreign version embedded in raw JSON.
    let mut raw = encode_line(&DatasetRecord::new(
        0,
        RecordedEvent::DevSell {
            slot: 1,
            mint: "m".into(),
            seller: "s".into(),
            observed_at: Utc::now(),
        },
    ))
    .unwrap();
    raw = raw.replacen(
        &format!(
            "\"schema_version\":{}",
            module_sniper::dataset::DATASET_SCHEMA_VERSION
        ),
        "\"schema_version\":998",
        1,
    );
    assert!(
        decode_line(&raw).is_err(),
        "foreign version must not decode"
    );

    // Manifest both encode and decode reject a foreign version.
    let mut manifest =
        DatasetManifest::new("ds".into(), "n".into(), DatasetSource::SyntheticFixture);
    manifest.schema_version = 12345;
    assert!(encode_manifest(&manifest).is_err());
    let mut good = DatasetManifest::new("ds".into(), "n".into(), DatasetSource::SyntheticFixture);
    good.schema_version = module_sniper::dataset::DATASET_SCHEMA_VERSION;
    let mut text = encode_manifest(&good).unwrap();
    text = text.replacen(
        &format!(
            "\"schema_version\": {}",
            module_sniper::dataset::DATASET_SCHEMA_VERSION
        ),
        "\"schema_version\": 4242",
        1,
    );
    assert!(decode_manifest(&text).is_err());
}

#[test]
fn manifest_roundtrip_preserves_all_fields() {
    let mut m = DatasetManifest::new("id-1".into(), "name".into(), DatasetSource::Recorded);
    m.cluster = "mainnet".into();
    m.record_count = 5;
    m.sha256_root = "ab".repeat(32);
    let text = encode_manifest(&m).unwrap();
    let back = decode_manifest(&text).unwrap();
    assert_eq!(m, back);
}
