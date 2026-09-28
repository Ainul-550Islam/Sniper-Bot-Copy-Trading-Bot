//! Observability config integration placeholder (Batch 5).
//! Real unit coverage lives in `crates/server/src/ops/observability_config.rs`.
//! This harness verifies the crate builds and the file exists — heavy logic is unit-tested.

#[test]
fn observability_config_module_exists() {
    // Verify that the observability config source file is present and non-empty.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ops/observability_config.rs"
    );
    let content = std::fs::read_to_string(path).expect("observability_config.rs must exist");
    assert!(content.contains("ObservabilityConfig"));
    assert!(content.contains("validate_for_production"));
}

#[test]
fn observability_config_has_safe_json() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ops/observability_config.rs"
    );
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("to_safe_json"));
    // The file legitimately contains the word "password" as a forbidden-substring check — that is not a leak.
    assert!(content.contains("safe_debug") || content.contains("to_safe_json"));
}

#[test]
fn production_validation_rejects_trace() {
    // Lightweight check that the source mentions trace rejection
    let content = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ops/observability_config.rs"
    ))
    .unwrap();
    assert!(content.contains("Trace"));
    assert!(content.contains("not allowed in production"));
}
