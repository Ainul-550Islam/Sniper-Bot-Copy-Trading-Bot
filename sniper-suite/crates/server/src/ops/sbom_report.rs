//! Structured SBOM metadata model (Batch 4). Do not invent license information. Mark unknown.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SbomPackage {
    pub name: String,
    pub version: String,
    pub license: Option<String>, // None => unknown
    pub source: Option<String>,  // e.g. "crates.io" or "path"
    pub purl: Option<String>,
}

impl SbomPackage {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            license: None,
            source: None,
            purl: None,
        }
    }
    pub fn with_license(mut self, lic: impl Into<String>) -> Self {
        self.license = Some(lic.into());
        self
    }
    pub fn with_source(mut self, s: impl Into<String>) -> Self {
        self.source = Some(s.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SbomReport {
    pub tool: String,
    pub generated_at: String,
    pub packages: Vec<SbomPackage>,
    pub unknown_license_count: usize,
}

pub fn build_report(packages: Vec<SbomPackage>) -> SbomReport {
    let unknown = packages
        .iter()
        .filter(|p| p.license.is_none() || p.license.as_deref() == Some("unknown"))
        .count();
    SbomReport {
        tool: format!("sniper-suite sbom {}", env!("CARGO_PKG_VERSION")),
        generated_at: chrono::Utc::now().to_rfc3339(),
        packages,
        unknown_license_count: unknown,
    }
}

/// Try to parse Cargo.lock to extract package names/versions without inventing licenses.
pub fn from_cargo_lock(lock_text: &str) -> Vec<SbomPackage> {
    let mut out = Vec::new();
    let mut cur_name: Option<String> = None;
    let mut cur_version: Option<String> = None;
    let mut cur_source: Option<String> = None;
    for line in lock_text.lines() {
        let l = line.trim();
        if l == "[[package]]" {
            if let (Some(n), Some(v)) = (cur_name.take(), cur_version.take()) {
                let mut p = SbomPackage::new(n, v);
                if let Some(s) = cur_source.take() {
                    p.source = Some(s);
                }
                out.push(p);
            }
            cur_source = None;
        } else if let Some(rest) = l.strip_prefix("name =") {
            cur_name = Some(rest.trim().trim_matches('"').to_string());
        } else if let Some(rest) = l.strip_prefix("version =") {
            cur_version = Some(rest.trim().trim_matches('"').to_string());
        } else if let Some(rest) = l.strip_prefix("source =") {
            cur_source = Some(rest.trim().trim_matches('"').to_string());
        }
    }
    if let (Some(n), Some(v)) = (cur_name, cur_version) {
        let mut p = SbomPackage::new(n, v);
        if let Some(s) = cur_source {
            p.source = Some(s);
        }
        out.push(p);
    }
    out
}

pub fn cyclonedx_json(report: &SbomReport) -> serde_json::Value {
    serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.4",
        "metadata": { "timestamp": report.generated_at, "tools": [{"name": report.tool}] },
        "components": report.packages.iter().map(|p| serde_json::json!({
            "name": p.name, "version": p.version, "licenses": p.license.as_ref().map(|l| vec![serde_json::json!({"license": {"id": l}})]).unwrap_or_default(), "purl": p.purl
        })).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_counted() {
        let pkgs = vec![
            SbomPackage::new("a", "1.0").with_license("MIT"),
            SbomPackage::new("b", "2.0"),
        ];
        let r = build_report(pkgs);
        assert_eq!(r.unknown_license_count, 1);
    }
    #[test]
    fn parse_lock_without_license_invention() {
        let lock = "[[package]]\nname = \"tokio\"\nversion = \"1.38\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n[[package]]\nname = \"my-local\"\nversion = \"0.1.0\"\n";
        let pkgs = from_cargo_lock(lock);
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "tokio");
        assert!(pkgs[0].license.is_none());
        assert!(pkgs[1].license.is_none());
    }
    #[test]
    fn cyclonedx_structure() {
        let r = build_report(vec![SbomPackage::new("x", "1")]);
        let j = cyclonedx_json(&r);
        assert_eq!(j["bomFormat"], "CycloneDX");
    }
    #[test]
    fn does_not_guess_license() {
        let pkgs = from_cargo_lock("[[package]]\nname = \"unknown-crate\"\nversion = \"0.0.1\"\n");
        assert!(pkgs[0].license.is_none());
    }
}
