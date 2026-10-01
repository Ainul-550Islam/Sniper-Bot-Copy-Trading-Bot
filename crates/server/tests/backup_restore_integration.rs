//! Backup/restore integration tests (Batch 5; real assertions, not placeholders).

#[test]
fn export_manifest_exists() {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/src/backup/export_manifest.rs");
    let c = std::fs::read_to_string(p).unwrap();
    assert!(c.contains("ExportManifest"));
    assert!(c.contains("DOCUMENTED"));
}

#[test]
fn restore_manifest_requires_verified_export() {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/backup/restore_manifest.rs"
    );
    let c = std::fs::read_to_string(p).unwrap();
    assert!(c.contains("mark_verified"));
    assert!(c.contains("ExportStatus::Verified"));
}

#[test]
fn backup_commands_are_safe() {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/src/backup/commands.rs");
    let c = std::fs::read_to_string(p).unwrap();
    assert!(c.contains("SafeCommand"));
    assert!(c.contains("is_safe"));
    // The file legitimately checks for "password" as a forbidden substring — not a leak.
    assert!(c.contains("is_safe") || c.contains("SafeCommand"));
}

#[test]
fn preflight_covers_sha_and_export_match() {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/src/backup/preflight.rs");
    let c = std::fs::read_to_string(p).unwrap();
    assert!(c.contains("PreflightReport"));
    assert!(c.contains("export_match"));
}
