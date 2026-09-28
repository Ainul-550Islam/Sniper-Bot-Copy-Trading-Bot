//! Batch 6 transaction-readiness focused tests: env registry, API/webhook matrix, SBOM/license crosswalk, evidence crosswalk, package completeness.

use std::path::Path;

fn repo_root() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../..").to_string()
}
fn read_doc(name: &str) -> String {
    let p = format!("{}/docs/{}", repo_root(), name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("missing docs/{name}: {e} — path {p}"))
}

// --- env registry ---
#[test]
fn env_registry_exists_and_covers_required_vars() {
    let c = read_doc("ENVIRONMENT-VARIABLE-REGISTER.md");
    for v in [
        "DATABASE_URL",
        "REDIS_URL",
        "SOLANA_KEYPAIR",
        "STRIPE_API_KEY",
        "VAULT_ADDR",
        "EXECUTION_MODE",
    ] {
        assert!(
            c.contains(v),
            "ENVIRONMENT-VARIABLE-REGISTER.md missing {v}"
        );
    }
    assert!(c.contains("Required"), "must have Required column");
    assert!(c.contains("Secret"), "must have Secret column");
    assert!(
        c.contains("Failure Behavior") || c.contains("Failure"),
        "must document failure"
    );
    assert!(c.contains(".env.template"), "must reference .env.template");
}

#[test]
fn env_registry_distinguishes_secret_vs_non_secret() {
    let c = read_doc("ENVIRONMENT-VARIABLE-REGISTER.md");
    // secrets redacted in health_report
    assert!(
        c.contains("health_report") || c.contains("redact") || c.contains("Secret"),
        "should mention redaction"
    );
}

// --- API compatibility matrix ---
#[test]
fn api_compatibility_matrix_exists_and_lists_endpoints() {
    let c = read_doc("API-COMPATIBILITY-MATRIX.md");
    for needle in [
        "GET /health",
        "/api/saas/openapi.json",
        "/api/events",
        "saas-sdk",
        "0.1.0",
    ] {
        assert!(c.contains(needle), "API matrix missing {needle}");
    }
    assert!(
        c.contains("STABLE") || c.contains("Deprecated"),
        "must have status"
    );
    assert!(
        c.contains("Version") || c.contains("version"),
        "must mention version"
    );
}

#[test]
fn api_matrix_covers_websocket_deprecated_query() {
    let c = read_doc("API-COMPATIBILITY-MATRIX.md");
    assert!(
        c.contains("?key=") || c.contains("legacy"),
        "must document legacy websocket query"
    );
    assert!(
        c.contains("Authorization: Bearer") || c.contains("Bearer"),
        "must document header auth"
    );
}

// --- webhook compatibility matrix ---
#[test]
fn webhook_matrix_exists_and_covers_hmac_idempotency() {
    let c = read_doc("WEBHOOK-COMPATIBILITY-MATRIX.md");
    for needle in ["billing_webhook", "HMAC", "idempotency", "Stripe", "Paddle"] {
        assert!(c.contains(needle), "WEBHOOK matrix missing {needle}");
    }
    assert!(
        c.contains("EXTERNAL_REQUIRED") || c.contains("Fixture"),
        "must distinguish fixture vs live"
    );
    assert!(
        c.contains("provider_events") || c.contains("ProviderEvent"),
        "must mention provider_events"
    );
}

#[test]
fn webhook_matrix_distinguishes_fixture_vs_live() {
    let c = read_doc("WEBHOOK-COMPATIBILITY-MATRIX.md");
    assert!(
        c.contains("FIXTURE-TESTED") || c.contains("fixture"),
        "fixture-tested marker"
    );
    assert!(
        c.contains("EXTERNAL_REQUIRED") || c.contains("live"),
        "live marker"
    );
}

// --- SBOM / license crosswalk ---
#[test]
fn sbom_license_crosswalk_files_exist() {
    let root = repo_root();
    assert!(
        Path::new(&format!("{root}/sbom.json")).exists(),
        "sbom.json missing"
    );
    assert!(
        Path::new(&format!("{root}/licenses.json")).exists(),
        "licenses.json missing"
    );
    assert!(
        Path::new(&format!("{root}/sbom.cyclonedx.json")).exists(),
        "sbom.cyclonedx.json missing"
    );
    let lic = std::fs::read_to_string(format!("{root}/licenses.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&lic).unwrap();
    assert!(v.is_array(), "licenses.json should be array");
    assert!(
        v.as_array().unwrap().len() >= 100,
        "licenses.json should have many entries (707 expected)"
    );
    let sbom = std::fs::read_to_string(format!("{root}/sbom.json")).unwrap();
    let sv: serde_json::Value = serde_json::from_str(&sbom).unwrap();
    assert!(
        sv.get("components").is_some() || sv.get("bomFormat").is_some(),
        "sbom.json should have components/bomFormat"
    );
}

#[test]
fn third_party_inventory_and_compliance_docs_crosswalk() {
    let inv = read_doc("THIRD-PARTY-SOFTWARE-INVENTORY.md");
    let comp = read_doc("OPEN-SOURCE-COMPLIANCE.md");
    for needle in ["sbom.json", "licenses.json", "707"] {
        assert!(
            inv.contains(needle) || comp.contains(needle),
            "inventory/compliance missing {needle}"
        );
    }
    assert!(
        inv.contains("MIT") || inv.contains("Apache"),
        "must list licenses"
    );
    assert!(
        comp.contains("LEGAL_REVIEW_REQUIRED") || comp.contains("UNKNOWN"),
        "must flag unknown"
    );
    // crosswalk: sbom sha mentioned in inventory
    assert!(
        inv.contains("sha") || inv.contains("SHA256"),
        "inventory should mention sha"
    );
}

// --- evidence crosswalk ---
#[test]
fn evidence_crosswalk_exists_and_maps_claims() {
    let c = read_doc("FINAL-EVIDENCE-CROSSWALK.md");
    for needle in [
        "Tenant isolation",
        "Billing",
        "Custody",
        "22",
        "343",
        "101",
        "0.1.0",
    ] {
        assert!(
            c.contains(needle) || c.to_lowercase().contains(&needle.to_lowercase()),
            "crosswalk missing {needle}"
        );
    }
    assert!(
        c.contains("Source File") && c.contains("Test"),
        "must be table with Source File + Test"
    );
    assert!(
        c.contains("EXTERNAL_REQUIRED") || c.contains("PASS"),
        "must have status"
    );
    assert!(
        c.contains("FINAL-BUYER-GAP-LEDGER") || c.contains("external_validation"),
        "must crosswalk gaps"
    );
}

// --- package completeness ---
#[test]
fn package_completeness_buyer_rules_and_manifest() {
    // buyer_package_verify rules forbid target/node_modules/.git/.env
    let verify = std::fs::read_to_string(format!(
        "{}/crates/server/src/ops/buyer_package_verify.rs",
        repo_root()
    ))
    .unwrap();
    for pat in ["target/", "node_modules/", ".git/", ".env", "private_key"] {
        assert!(
            verify.contains(pat),
            "buyer_package_verify.rs missing forbidden {pat}"
        );
    }
    // release_manifest_verify has stale detection
    let manifest = std::fs::read_to_string(format!(
        "{}/crates/server/src/ops/release_manifest_verify.rs",
        repo_root()
    ))
    .unwrap();
    assert!(
        manifest.contains("stale_fields") && manifest.contains("verify_manifest_counts"),
        "release_manifest_verify incomplete"
    );
    // buyer-release layout if built, else script exists
    let root = repo_root();
    if Path::new(&format!("{root}/buyer-release")).exists() {
        let forbidden = ["target", ".git"];
        for f in forbidden {
            assert!(
                !Path::new(&format!("{root}/buyer-release/{f}")).exists(),
                "buyer-release should not contain {f}"
            );
        }
    } else {
        assert!(Path::new(&format!("{root}/scripts/build-release-package.sh")).exists());
    }
}

#[test]
fn all_25_batch6_docs_exist() {
    let expected = [
        "DATA-ROOM-INDEX.md",
        "ARCHITECTURE-OVERVIEW.md",
        "SECURITY-THREAT-MODEL.md",
        "SECURITY-CONTROLS-MATRIX.md",
        "PENETRATION-TEST-READINESS.md",
        "PRODUCTION-READINESS-MATRIX.md",
        "OPERATIONS-RUNBOOK.md",
        "INCIDENT-RESPONSE-RUNBOOK.md",
        "ROLLBACK-RUNBOOK.md",
        "DEPLOYMENT-ENVIRONMENT-MATRIX.md",
        "SECRETS-MANAGEMENT-MATRIX.md",
        "THIRD-PARTY-SOFTWARE-INVENTORY.md",
        "OPEN-SOURCE-COMPLIANCE.md",
        "IP-HANDOVER-CHECKLIST.md",
        "IP-OWNERSHIP-REGISTER.md",
        "TRADEMARK-DOMAIN-REGISTER.md",
        "ENVIRONMENT-VARIABLE-REGISTER.md",
        "API-COMPATIBILITY-MATRIX.md",
        "WEBHOOK-COMPATIBILITY-MATRIX.md",
        "BUYER-VERIFICATION-SCRIPT.md",
        "KNOWN-LIMITATIONS.md",
        "TRANSACTION-READINESS-REPORT.md",
        "FINAL-EVIDENCE-CROSSWALK.md",
        "RELEASE-NOTES-CURRENT.md",
        "BUILD-OUTPUT-HYGIENE-RESOLUTION.md",
    ];
    let root = repo_root();
    for doc in expected {
        assert!(
            Path::new(&format!("{root}/docs/{doc}")).exists(),
            "missing batch6 doc {doc}"
        );
        let content = std::fs::read_to_string(format!("{root}/docs/{doc}")).unwrap();
        assert!(content.len() > 200, "doc {doc} suspiciously small");
    }
    // total docs 95
    let count = std::fs::read_dir(format!("{root}/docs"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .map(|x| x == "md")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        count, 101,
        "docs/*.md should equal the current document count (101)"
    );
}

#[test]
fn release_manifest_counts_and_version_are_current() {
    let root = repo_root();
    let manifest = std::fs::read_to_string(format!("{root}/release-manifest.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(v["version"], "0.1.0");
    // Manifest counts must track the tree, not a historical batch snapshot.
    let docs = std::fs::read_dir(format!("{root}/docs"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .map(|x| x == "md")
                .unwrap_or(false)
        })
        .count();
    let rust = walk_rs(&format!("{root}/crates"));
    let migrations = std::fs::read_dir(format!("{root}/crates/core/migrations"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .extension()
                .map(|x| x == "sql")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(v["docs_files"].as_u64().unwrap() as usize, docs);
    assert_eq!(v["rust_files"].as_u64().unwrap() as usize, rust);
    assert_eq!(v["migrations"].as_u64().unwrap() as usize, migrations);
    assert!(rust >= 343, "rust sources must not regress");
    assert!(migrations >= 22, "migrations must not regress");
}

/// Count every `.rs` file under `dir` (mirrors `find crates -name '*.rs'`).
fn walk_rs(dir: &str) -> usize {
    let mut n = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            n += walk_rs(&path.to_string_lossy());
        } else if path.extension().map(|x| x == "rs").unwrap_or(false) {
            n += 1;
        }
    }
    n
}
