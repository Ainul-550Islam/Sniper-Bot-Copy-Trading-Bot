//! Aggregate dependency/license information (Batch 4).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LicenseClass {
    Permissive,
    Copyleft,
    Unknown,
    Unavailable,
}

impl LicenseClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Permissive => "permissive",
            Self::Copyleft => "copyleft",
            Self::Unknown => "unknown",
            Self::Unavailable => "unavailable",
        }
    }
}

fn classify(license: Option<&str>) -> LicenseClass {
    match license.map(|s| s.to_ascii_lowercase()) {
        None => LicenseClass::Unknown,
        Some(s) if s.contains("unknown") || s.trim().is_empty() => LicenseClass::Unknown,
        Some(s) if s.contains("unavailable") => LicenseClass::Unavailable,
        Some(s) if s.contains("gpl") || s.contains("agpl") || s.contains("lgpl") => {
            LicenseClass::Copyleft
        }
        Some(s)
            if s.contains("mit")
                || s.contains("apache")
                || s.contains("bsd")
                || s.contains("isc")
                || s.contains("mpl") =>
        {
            LicenseClass::Permissive
        }
        Some(_) => LicenseClass::Unknown,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseEntry {
    pub package: String,
    pub version: String,
    pub license: Option<String>,
    pub class: LicenseClass,
    pub needs_review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseReport {
    pub entries: Vec<LicenseEntry>,
    pub permissive: usize,
    pub copyleft: usize,
    pub unknown: usize,
    pub unavailable: usize,
    pub generated_at: String,
}

pub fn build(entries: Vec<(String, String, Option<String>)>) -> LicenseReport {
    let mut out = Vec::new();
    for (pkg, ver, lic) in entries {
        let class = classify(lic.as_deref());
        let needs_review = matches!(
            class,
            LicenseClass::Copyleft | LicenseClass::Unknown | LicenseClass::Unavailable
        );
        out.push(LicenseEntry {
            package: pkg,
            version: ver,
            license: lic,
            class,
            needs_review,
        });
    }
    let permissive = out
        .iter()
        .filter(|e| e.class == LicenseClass::Permissive)
        .count();
    let copyleft = out
        .iter()
        .filter(|e| e.class == LicenseClass::Copyleft)
        .count();
    let unknown = out
        .iter()
        .filter(|e| e.class == LicenseClass::Unknown)
        .count();
    let unavailable = out
        .iter()
        .filter(|e| e.class == LicenseClass::Unavailable)
        .count();
    LicenseReport {
        entries: out,
        permissive,
        copyleft,
        unknown,
        unavailable,
        generated_at: chrono::Utc::now().to_rfc3339(),
    }
}

pub fn human_summary(report: &LicenseReport) -> String {
    format!("permissive={} copyleft={} unknown={} unavailable={} total={} — copyleft/unknown require legal review; not legal approval", report.permissive, report.copyleft, report.unknown, report.unavailable, report.entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permissive_classified() {
        assert_eq!(classify(Some("MIT")), LicenseClass::Permissive);
        assert_eq!(classify(Some("Apache-2.0")), LicenseClass::Permissive);
    }
    #[test]
    fn copyleft_flagged() {
        assert_eq!(classify(Some("GPL-3.0")), LicenseClass::Copyleft);
    }
    #[test]
    fn unknown_when_missing() {
        assert_eq!(classify(None), LicenseClass::Unknown);
    }
    #[test]
    fn build_counts() {
        let r = build(vec![
            ("a".into(), "1".into(), Some("MIT".into())),
            ("b".into(), "1".into(), None),
            ("c".into(), "1".into(), Some("GPL-3.0".into())),
        ]);
        assert_eq!(r.permissive, 1);
        assert_eq!(r.copyleft, 1);
        assert_eq!(r.unknown, 1);
        assert!(r.entries.iter().any(|e| e.needs_review && e.package == "b"));
    }
    #[test]
    fn human_summary_does_not_claim_approval() {
        let r = build(vec![("a".into(), "1".into(), Some("MIT".into()))]);
        let s = human_summary(&r);
        assert!(s.contains("not legal approval"));
        assert!(!s.to_ascii_lowercase().contains("fully compliant"));
    }
}
