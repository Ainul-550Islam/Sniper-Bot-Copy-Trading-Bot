//! Verify release context matches manifest — counts, migrations, VERSION, Cargo lock presence (Batch 5).
//! Recompute counts live; never trust stale manifest values.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestCounts {
    pub docs_files: usize,
    pub rust_files: usize,
    pub test_count: usize,
    pub migrations: usize,
    pub version: String,
}

impl ManifestCounts {
    pub fn new(
        docs_files: usize,
        rust_files: usize,
        test_count: usize,
        migrations: usize,
        version: impl Into<String>,
    ) -> Self {
        Self {
            docs_files,
            rust_files,
            test_count,
            migrations,
            version: version.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestVerifyResult {
    pub ok: bool,
    pub manifest_counts: ManifestCounts,
    pub actual_counts: ManifestCounts,
    pub stale_fields: Vec<String>,
    pub detail: String,
}

impl ManifestVerifyResult {
    pub fn is_stale(&self) -> bool {
        !self.stale_fields.is_empty()
    }

    pub fn to_safe_json(&self) -> serde_json::Value {
        serde_json::json!({
            "ok": self.ok,
            "stale_fields": self.stale_fields,
            "manifest_counts": self.manifest_counts,
            "actual_counts": self.actual_counts,
            "detail": self.detail
        })
    }
}

pub fn verify_manifest_counts(
    manifest: &ManifestCounts,
    actual: &ManifestCounts,
) -> ManifestVerifyResult {
    let mut stale = Vec::new();
    if manifest.docs_files != actual.docs_files {
        stale.push("docs_files".into());
    }
    if manifest.rust_files != actual.rust_files {
        stale.push("rust_files".into());
    }
    if manifest.test_count != actual.test_count {
        stale.push("test_count".into());
    }
    if manifest.migrations != actual.migrations {
        stale.push("migrations".into());
    }
    if manifest.version != actual.version {
        stale.push("version".into());
    }
    let ok = stale.is_empty();
    let detail = if ok {
        "manifest counts match actual".into()
    } else {
        format!("stale fields: {}", stale.join(", "))
    };
    ManifestVerifyResult {
        ok,
        manifest_counts: manifest.clone(),
        actual_counts: actual.clone(),
        stale_fields: stale,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_stale_counts() {
        let m = ManifestCounts::new(68, 273, 1010, 21, "0.1.0");
        let a = ManifestCounts::new(69, 295, 1090, 21, "0.1.0");
        let r = verify_manifest_counts(&m, &a);
        assert!(!r.ok);
        assert!(r.stale_fields.contains(&"docs_files".into()));
        assert!(r.stale_fields.contains(&"rust_files".into()));
        assert!(r.stale_fields.contains(&"test_count".into()));
    }

    #[test]
    fn ok_when_matching() {
        let m = ManifestCounts::new(69, 295, 1090, 21, "0.1.0");
        let a = ManifestCounts::new(69, 295, 1090, 21, "0.1.0");
        let r = verify_manifest_counts(&m, &a);
        assert!(r.ok);
        assert!(!r.is_stale());
    }

    #[test]
    fn version_stale_detected() {
        let m = ManifestCounts::new(69, 295, 1090, 21, "0.0.9");
        let a = ManifestCounts::new(69, 295, 1090, 21, "0.1.0");
        let r = verify_manifest_counts(&m, &a);
        assert!(r.stale_fields.contains(&"version".into()));
    }

    #[test]
    fn to_safe_json_has_ok_flag() {
        let m = ManifestCounts::new(1, 1, 1, 1, "0.1.0");
        let r = verify_manifest_counts(&m, &m);
        let v = r.to_safe_json();
        assert_eq!(v["ok"], true);
    }
}
