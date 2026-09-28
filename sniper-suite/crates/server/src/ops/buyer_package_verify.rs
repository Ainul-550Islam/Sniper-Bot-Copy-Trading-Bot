//! Buyer-package verification — confirm required/expected/forbidden layout (Batch 5).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuyerPackageRules {
    pub required_paths: Vec<String>,
    pub expected_dirs: Vec<String>,
    pub forbidden_substrings: Vec<String>,
}

impl BuyerPackageRules {
    pub fn default_rules() -> Self {
        Self {
            required_paths: vec![
                "VERSION".into(),
                "LICENSE".into(),
                "CHANGELOG.md".into(),
                "docs/FINAL-BUYER-GAP-LEDGER.md".into(),
                "release-manifest.json".into(),
            ],
            expected_dirs: vec![
                "source".into(),
                "docs".into(),
                "evidence".into(),
                "sbom".into(),
                "licenses".into(),
                "checksums".into(),
                "manifests".into(),
            ],
            forbidden_substrings: vec![
                "target/".into(),
                "node_modules/".into(),
                ".git/".into(),
                ".env".into(),
                "secrets/".into(),
                "private_key".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuyerPackageVerifyResult {
    pub ok: bool,
    pub missing_required: Vec<String>,
    pub missing_dirs: Vec<String>,
    pub forbidden_found: Vec<String>,
    pub detail: String,
}

pub fn verify_package_file_list(
    files: &[String],
    rules: &BuyerPackageRules,
) -> BuyerPackageVerifyResult {
    let set: HashSet<&str> = files.iter().map(|s| s.as_str()).collect();
    let mut missing_required = Vec::new();
    for r in &rules.required_paths {
        if !set.contains(r.as_str()) {
            missing_required.push(r.clone());
        }
    }
    let mut missing_dirs = Vec::new();
    for d in &rules.expected_dirs {
        let present = files
            .iter()
            .any(|f| f.starts_with(&format!("{d}/")) || f == d);
        if !present {
            missing_dirs.push(d.clone());
        }
    }
    let mut forbidden_found = Vec::new();
    for f in files {
        for pat in &rules.forbidden_substrings {
            if f.contains(pat) {
                forbidden_found.push(f.clone());
                break;
            }
        }
    }
    let ok = missing_required.is_empty() && missing_dirs.is_empty() && forbidden_found.is_empty();
    let detail = if ok {
        "buyer package layout ok".into()
    } else {
        format!(
            "missing_required={:?} missing_dirs={:?} forbidden={:?}",
            missing_required, missing_dirs, forbidden_found
        )
    };
    BuyerPackageVerifyResult {
        ok,
        missing_required,
        missing_dirs,
        forbidden_found,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_when_all_present() {
        let rules = BuyerPackageRules::default_rules();
        let files = vec![
            "VERSION".into(),
            "LICENSE".into(),
            "CHANGELOG.md".into(),
            "docs/FINAL-BUYER-GAP-LEDGER.md".into(),
            "release-manifest.json".into(),
            "source/x".into(),
            "docs/a".into(),
            "evidence/a".into(),
            "sbom/a".into(),
            "licenses/a".into(),
            "checksums/a".into(),
            "manifests/a".into(),
        ];
        let r = verify_package_file_list(&files, &rules);
        assert!(r.ok, "{}", r.detail);
    }

    #[test]
    fn detects_missing_required() {
        let rules = BuyerPackageRules::default_rules();
        let files: Vec<String> = vec!["VERSION".into()];
        let r = verify_package_file_list(&files, &rules);
        assert!(!r.ok);
        assert!(!r.missing_required.is_empty());
    }

    #[test]
    fn detects_forbidden_target() {
        let rules = BuyerPackageRules::default_rules();
        let files = vec![
            "VERSION".into(),
            "LICENSE".into(),
            "CHANGELOG.md".into(),
            "docs/FINAL-BUYER-GAP-LEDGER.md".into(),
            "release-manifest.json".into(),
            "source/x".into(),
            "docs/a".into(),
            "evidence/a".into(),
            "sbom/a".into(),
            "licenses/a".into(),
            "checksums/a".into(),
            "manifests/a".into(),
            "target/debug/x".into(),
        ];
        let r = verify_package_file_list(&files, &rules);
        assert!(r.forbidden_found.iter().any(|f| f.contains("target/")));
    }

    #[test]
    fn forbidden_node_modules() {
        let rules = BuyerPackageRules::default_rules();
        let files = vec!["node_modules/pkg/index.js".into()];
        let r = verify_package_file_list(&files, &rules);
        assert!(!r.forbidden_found.is_empty());
    }
}
