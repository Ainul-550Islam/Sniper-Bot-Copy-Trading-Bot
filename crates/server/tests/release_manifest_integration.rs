//! Release manifest integration placeholder (Batch 5).
//! Real logic in `crates/server/src/ops/release_manifest_verify.rs`.

#[test]
fn release_manifest_verify_module_exists() {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ops/release_manifest_verify.rs"
    );
    let c = std::fs::read_to_string(p).unwrap();
    assert!(c.contains("ManifestCounts"));
    assert!(c.contains("verify_manifest_counts"));
}

#[test]
fn release_manifest_has_stale_detection() {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/ops/release_manifest_verify.rs"
    );
    let c = std::fs::read_to_string(p).unwrap();
    assert!(c.contains("stale_fields"));
}

#[test]
fn release_manifest_json_exists_and_has_version() {
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../release-manifest.json"
    ))
    .unwrap();
    assert!(manifest.contains("\"version\""));
    assert!(manifest.contains("\"docs_files\""));
}

fn repo_file(rel: &str) -> String {
    std::fs::read_to_string(format!("{}/../../{}", env!("CARGO_MANIFEST_DIR"), rel))
        .unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn semver_tuple(v: &str) -> (u64, u64, u64) {
    let mut it = v.split('.');
    let major = it
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("bad semver {v}"));
    let minor = it
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("bad semver {v}"));
    let patch = it
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("bad semver {v}"));
    (major, minor, patch)
}

/// Batch 10/11 (2026-09-27): the shipped control-plane frontend must never silently regress to a
/// framework line that carries a known advisory.
///
/// History this guard encodes:
/// * Batch 10 — the pin was `next 15.5.4`, affected by CVE-2025-66478 (CVSS 10.0 remote code
///   execution in the React Server Components protocol, upstream CVE-2025-55182). It was raised to
///   the patched `15.5.26`.
/// * Batch 11 (F-2) — `next 15.5.x` still resolved `postcss 8.4.31`, which `npm audit` rates high
///   (unescaped `</style>` output / `sourceMappingURL` `.map` disclosure), and the only fix is the
///   `next >= 16.3.6` line. The pin is therefore `next 16.3.6`, which resolves `postcss 8.5.23`.
///
/// The test reads the shipped files only (no network, no npm) so a buyer or CI can re-run it
/// offline, and it fails on a deliberate revert to either affected state (the Batch-11 negative
/// controls were executed in the seller-side working report, which is not shipped).
#[test]
fn frontend_lockfile_pins_patched_nextjs() {
    let pkg: serde_json::Value =
        serde_json::from_str(&repo_file("apps/control-plane/package.json"))
            .expect("package.json parses");
    let lock: serde_json::Value =
        serde_json::from_str(&repo_file("apps/control-plane/package-lock.json"))
            .expect("package-lock.json parses");

    let next = pkg["dependencies"]["next"]
        .as_str()
        .expect("package.json pins next exactly")
        .to_string();
    let eslint_next = pkg["devDependencies"]["eslint-config-next"]
        .as_str()
        .or_else(|| pkg["dependencies"]["eslint-config-next"].as_str())
        .expect("package.json pins eslint-config-next exactly")
        .to_string();

    // (1) Framework floor: the postcss fix for the 15.x line only shipped in next >= 16.3.6.
    //     Reverting to any 15.x pin — including the vulnerable 15.5.4 and the Batch-10 patch
    //     release 15.5.26 — must fail here.
    assert!(
        semver_tuple(&next) >= (16, 3, 6),
        "next {next} is below the F-2 remediation floor of 16.3.6 (batch 11)"
    );
    assert_ne!(
        next, "15.5.26",
        "next 15.5.26 is the Batch-10 pin whose postcss chain was still affected — do not restore it"
    );
    assert_ne!(
        next, "15.5.4",
        "next 15.5.4 is the CVE-2025-66478 (CVSS 10.0 RCE) pin — do not restore it"
    );
    assert_ne!(
        next, "15.5.7",
        "next 15.5.7 is only the CVE-2025-66478 floor, not the postcss floor — do not restore it"
    );

    // (2) eslint-config-next must stay on the same compatible major/minor line as next.
    let (next_major, next_minor, _) = semver_tuple(&next);
    let (eslint_major, eslint_minor, _) = semver_tuple(&eslint_next);
    assert_eq!(
        (eslint_major, eslint_minor),
        (next_major, next_minor),
        "eslint-config-next {eslint_next} must match the next {next} major/minor line"
    );
    assert!(
        semver_tuple(&eslint_next) >= (16, 3, 6),
        "eslint-config-next {eslint_next} is below the 16.3.6 line that matches next {next}"
    );

    // (3) package.json and package-lock.json must agree (mirrors the frontend-ci.yml consistency
    //     gate, offline). The lockfile is part of the buyer-delivered dependency state.
    let lock_next = lock["packages"]["node_modules/next"]["version"]
        .as_str()
        .expect("lockfile records the next package")
        .to_string();
    assert_eq!(
        lock_next, next,
        "lockfile next must equal package.json next"
    );
    assert_eq!(
        lock["packages"][""]["dependencies"]["next"].as_str(),
        Some(next.as_str()),
        "lockfile root dependency must equal package.json next"
    );
    assert_eq!(
        lock["packages"][""]["devDependencies"]["eslint-config-next"].as_str(),
        Some(eslint_next.as_str()),
        "lockfile root devDependency must equal package.json eslint-config-next"
    );

    // (4) Every resolved `postcss` in the shipped lockfile must be at or above the F-2 floor.
    //     Scanning all entries (not just the root one) catches a re-introduced nested copy.
    let mut postcss_versions: Vec<String> = Vec::new();
    for (path, entry) in lock["packages"]
        .as_object()
        .expect("lockfile packages object")
        .iter()
    {
        let is_postcss = path == "node_modules/postcss"
            || path.ends_with("/node_modules/postcss")
            || path == "postcss";
        if !is_postcss {
            continue;
        }
        if let Some(v) = entry["version"].as_str() {
            postcss_versions.push(v.to_string());
        }
    }
    assert!(
        !postcss_versions.is_empty(),
        "the shipped lockfile must resolve postcss (next depends on it)"
    );
    for v in &postcss_versions {
        assert!(
            semver_tuple(v) >= (8, 5, 23),
            "resolved postcss {v} is below the F-2 remediation floor of 8.5.23"
        );
    }

    // (5) The F-2 remediation must stay documented for the buyer (history + no silent rewrite):
    //     the CVE that Batch 10 fixed and the dependency floor Batch 11 established.
    let limitations = repo_file("docs/KNOWN-LIMITATIONS.md");
    assert!(
        limitations.contains("CVE-2025-66478"),
        "KNOWN-LIMITATIONS must record the CVE-2025-66478 remediation"
    );
    assert!(
        limitations.contains("16.3.6"),
        "KNOWN-LIMITATIONS must record the F-2 remediation floor (next 16.3.6)"
    );
    assert!(
        limitations.contains("8.5.23"),
        "KNOWN-LIMITATIONS must record the resolved postcss floor (8.5.23)"
    );
}
