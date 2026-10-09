//! The exported OpenAPI artifact is kept in lockstep with the code (P1).
//!
//! # Why an exported file at all
//!
//! `/api/saas/openapi.json` only exists when a deployment is running.
//! Everyone who needs the contract *before* that — a buyer doing
//! technical due diligence, an SDK generator in CI, a partner writing an
//! integration, an auditor reading the repository — needs a file. So the
//! document is committed at `openapi/openapi.json` (authoritative) with a
//! generated `openapi/openapi.yaml` beside it.
//!
//! A committed artifact that can drift from the code is worse than none:
//! it reads as authoritative while being wrong. This test is the lock.
//!
//! ```text
//! cargo test -p sniper-suite --test openapi_artifact      # verify
//! UPDATE_OPENAPI=1 cargo test -p sniper-suite --test openapi_artifact   # regenerate
//! ./scripts/export-openapi.sh                             # both files
//! ```

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use sniper_suite::saas::openapi::{document, API_VERSION};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/server.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repo root")
}

fn artifact_path() -> PathBuf {
    repo_root().join("openapi/openapi.json")
}

/// Exactly the bytes the artifact must contain: pretty JSON, trailing
/// newline. Deterministic, because `serde_json::Value` maps are ordered
/// (the crate is built with the `preserve_order` feature off, so object
/// keys are sorted) — two runs of the generator produce identical bytes
/// and a diff means a real contract change.
fn rendered() -> String {
    let doc = document();
    let mut out = serde_json::to_string_pretty(&doc).expect("serialize openapi document");
    out.push('\n');
    out
}

#[test]
fn exported_artifact_matches_the_code() {
    let path = artifact_path();
    let want = rendered();

    if std::env::var("UPDATE_OPENAPI").is_ok() {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create openapi/");
        std::fs::write(&path, &want).expect("write artifact");
        eprintln!("UPDATE_OPENAPI: wrote {}", path.display());
        return;
    }

    let got = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{} is missing ({e}).\n\
             Generate it with:  UPDATE_OPENAPI=1 cargo test -p sniper-suite --test openapi_artifact",
            path.display()
        )
    });

    if got != want {
        // Report the SHAPE of the drift, not a 6000-line diff.
        let got_doc: Value = serde_json::from_str(&got).expect("committed artifact is valid JSON");
        let want_doc = document();
        let got_paths: BTreeSet<&str> = got_doc["paths"]
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let want_paths: BTreeSet<&str> = want_doc["paths"]
            .as_object()
            .map(|m| m.keys().map(String::as_str).collect())
            .unwrap_or_default();

        let added: Vec<&&str> = want_paths.difference(&got_paths).collect();
        let removed: Vec<&&str> = got_paths.difference(&want_paths).collect();

        panic!(
            "openapi/openapi.json is STALE.\n\
             paths added by the code: {added:?}\n\
             paths only in the file:  {removed:?}\n\
             (an empty list on both sides means a schema/description change)\n\n\
             Regenerate and commit:\n  ./scripts/export-openapi.sh"
        );
    }
}

/// The document must declare the contract version, not the crate version.
#[test]
fn artifact_declares_the_contract_version() {
    let doc = document();
    assert_eq!(
        doc["info"]["version"].as_str(),
        Some(API_VERSION),
        "info.version must be saas::openapi::API_VERSION"
    );
    assert_ne!(
        API_VERSION,
        env!("CARGO_PKG_VERSION"),
        "the contract version must be tracked independently of the crate version \
         (see docs/API-VERSIONING.md); if they coincide, bump one deliberately"
    );
}

/// Every operation is identifiable and uniquely named. SDK generators
/// derive method names from `operationId`; a duplicate silently drops an
/// endpoint from the generated client.
#[test]
fn every_operation_id_is_present_and_unique() {
    let doc = document();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let paths = doc["paths"].as_object().expect("paths object");
    assert!(!paths.is_empty(), "the contract declares no paths");

    for (path, item) in paths {
        for (method, op) in item.as_object().expect("path item object") {
            if method == "parameters" {
                continue;
            }
            let id = op["operationId"]
                .as_str()
                .unwrap_or_else(|| panic!("{path} {method} has no operationId"));
            assert!(
                seen.insert(id.to_string()),
                "duplicate operationId '{id}' (at {path} {method})"
            );
        }
    }
}

/// The merged fragments actually arrived. Before the merge these four
/// modules were compiled and never called, so the published contract
/// omitted every path they describe. This test is what stops that from
/// silently happening again.
#[test]
fn the_surface_fragments_are_merged_in() {
    let doc = document();
    let paths = doc["paths"].as_object().expect("paths");
    let all: Vec<&str> = paths.keys().map(String::as_str).collect();

    for (surface, needle) in [
        ("billing", "/api/saas/billing"),
        ("custody", "/api/saas/custody"),
        ("ops", "/api/ops"),
    ] {
        assert!(
            all.iter().any(|p| p.starts_with(needle)),
            "the {surface} fragment is not merged into the contract \
             (no path starts with {needle}); merge_api_fragments regressed"
        );
    }
}

/// A public contract must not describe a secret as a readable field.
/// `writeOnly` request fields are fine; a RESPONSE schema that returns a
/// token, a key or a hash is not.
#[test]
fn no_response_schema_exposes_a_credential() {
    const FORBIDDEN: [&str; 7] = [
        "password",
        "private_key",
        "secret_key",
        "api_key_plaintext",
        "key_hash",
        "password_hash",
        "token_hash",
    ];

    fn walk(node: &Value, trail: &str, hits: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                for (k, v) in map {
                    let lower = k.to_ascii_lowercase();
                    if FORBIDDEN.contains(&lower.as_str())
                        && !v.get("writeOnly").and_then(Value::as_bool).unwrap_or(false)
                    {
                        hits.push(format!("{trail}/{k}"));
                    }
                    walk(v, &format!("{trail}/{k}"), hits);
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    walk(v, &format!("{trail}/{i}"), hits);
                }
            }
            _ => {}
        }
    }

    let doc = document();
    let mut hits = Vec::new();
    for (path, item) in doc["paths"].as_object().expect("paths") {
        for (method, op) in item.as_object().expect("path item") {
            if method == "parameters" {
                continue;
            }
            if let Some(responses) = op.get("responses") {
                walk(responses, &format!("{path} {method} responses"), &mut hits);
            }
        }
    }
    assert!(
        hits.is_empty(),
        "response schemas expose credential-shaped fields: {hits:?}"
    );
}

/// Every `$ref` resolves inside the document.
///
/// This caught a real defect: the surface fragments referenced
/// `#/components/responses/{Unauthorized,Forbidden}`, which the base
/// document never declared. A dangling `$ref` makes the whole document
/// invalid — most SDK generators abort, and the ones that do not emit
/// an endpoint with an untyped error path.
#[test]
fn every_ref_resolves() {
    let doc = document();
    let mut dangling: Vec<String> = Vec::new();
    collect_dangling_refs(&doc, &doc, &mut dangling);
    dangling.sort();
    dangling.dedup();
    assert!(
        dangling.is_empty(),
        "the contract contains unresolvable $ref(s): {dangling:?}"
    );
}

fn collect_dangling_refs(root: &Value, node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(target)) = map.get("$ref") {
                if resolve_pointer(root, target).is_none() {
                    out.push(target.clone());
                }
            }
            for value in map.values() {
                collect_dangling_refs(root, value, out);
            }
        }
        Value::Array(items) => {
            for value in items {
                collect_dangling_refs(root, value, out);
            }
        }
        _ => {}
    }
}

/// Resolve a local JSON pointer (`#/a/b`). External refs are not used by
/// this contract and are reported as unresolvable on purpose: an
/// external reference would make the artifact non-self-contained, which
/// defeats the point of committing it.
fn resolve_pointer<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let path = reference.strip_prefix("#/")?;
    let mut cursor = root;
    for segment in path.split('/') {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        cursor = cursor.get(segment)?;
    }
    Some(cursor)
}

/// No schema sits in the document unreachable from any operation.
///
/// An orphan schema is dead weight that SDK generators turn into a
/// public type nobody can obtain, and it is usually the fingerprint of
/// an endpoint that was removed, renamed, or never documented. The
/// allow-list below is deliberately short and each entry states why it
/// is still here; adding to it should be an argued decision in review,
/// not a reflex to make the test pass.
#[test]
fn no_schema_is_orphaned_without_justification() {
    /// Shapes that exist in the code but that no documented operation
    /// returns yet:
    ///   * `BillingState`  — superseded by the commercial surface's
    ///     `BillingStatus`; kept until the billing fragment's consumers
    ///     are migrated off it.
    ///   * `CredentialRefView` — custody credential *references* (never
    ///     values) are modelled but not yet exposed by any endpoint.
    ///
    /// `SecurityEvidence`, `DeploymentPreflight`, `RuntimeConfigReport`,
    /// `DependencyHealth` and `ReleaseArtifact` used to be here. They are
    /// no longer published at all: their endpoints are not routed, so
    /// they moved to
    /// `crate::api::openapi_ops::unrouted_schemas_pending_implementation`.
    const ALLOWED_ORPHANS: &[&str] = &["BillingState", "CredentialRefView"];

    let doc = document();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    collect_schema_refs(&doc, &mut referenced);

    let declared: BTreeSet<String> = doc["components"]["schemas"]
        .as_object()
        .expect("components.schemas is an object")
        .keys()
        .cloned()
        .collect();

    let orphans: Vec<&String> = declared
        .iter()
        .filter(|name| !referenced.contains(*name) && !ALLOWED_ORPHANS.contains(&name.as_str()))
        .collect();
    assert!(
        orphans.is_empty(),
        "schema(s) declared but referenced by no operation: {orphans:?}. \
         Either reference them from the operation that returns them, delete them, \
         or justify them in ALLOWED_ORPHANS."
    );

    // The allow-list must not rot: an entry that became referenced (or
    // that no longer exists) is removed, so the list stays meaningful.
    let stale: Vec<&&str> = ALLOWED_ORPHANS
        .iter()
        .filter(|name| !declared.contains(**name) || referenced.contains(**name))
        .collect();
    assert!(
        stale.is_empty(),
        "ALLOWED_ORPHANS is stale: {stale:?} is now referenced or no longer declared — remove it"
    );
}

fn collect_schema_refs(node: &Value, out: &mut BTreeSet<String>) {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(target)) = map.get("$ref") {
                if let Some(name) = target.strip_prefix("#/components/schemas/") {
                    out.insert(name.to_string());
                }
            }
            for value in map.values() {
                collect_schema_refs(value, out);
            }
        }
        Value::Array(items) => {
            for value in items {
                collect_schema_refs(value, out);
            }
        }
        _ => {}
    }
}
