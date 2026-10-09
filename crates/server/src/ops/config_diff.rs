//! Compare safe runtime config against expected production schema (Batch 5).
//! Categorize REQUIRED/OPTIONAL/UNSAFE/UNKNOWN without leaking secret contents.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ConfigCategory {
    Required,
    Optional,
    Unsafe,
    Unknown,
}

impl ConfigCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Required => "REQUIRED",
            Self::Optional => "OPTIONAL",
            Self::Unsafe => "UNSAFE",
            Self::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDiffEntry {
    pub key: String,
    pub category: ConfigCategory,
    pub current: String, // safe/redacted
    pub expected: String,
    pub detail: String,
}

impl ConfigDiffEntry {
    pub fn new(
        key: impl Into<String>,
        category: ConfigCategory,
        current: impl Into<String>,
        expected: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            category,
            current: current.into(),
            expected: expected.into(),
            detail: detail.into(),
        }
    }

    pub fn redacted_current(&self) -> String {
        let lower = self.current.to_ascii_lowercase();
        if lower.contains("password")
            || lower.contains("secret")
            || lower.contains("token")
            || self.current.contains("://") && self.current.contains('@')
        {
            "<redacted>".into()
        } else {
            self.current.clone()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigDiffReport {
    pub entries: Vec<ConfigDiffEntry>,
    pub generated_at: String,
}

impl ConfigDiffReport {
    pub fn new(entries: Vec<ConfigDiffEntry>, generated_at: impl Into<String>) -> Self {
        Self {
            entries,
            generated_at: generated_at.into(),
        }
    }

    pub fn compare(
        current: &BTreeMap<String, String>,
        expected: &BTreeMap<String, (ConfigCategory, String)>,
        generated_at: impl Into<String>,
    ) -> Self {
        let mut entries = Vec::new();
        for (key, (cat, exp)) in expected {
            match current.get(key) {
                None => {
                    if *cat == ConfigCategory::Required {
                        entries.push(ConfigDiffEntry::new(
                            key,
                            ConfigCategory::Required,
                            "<missing>",
                            exp,
                            "required key missing",
                        ));
                    } else if *cat == ConfigCategory::Unsafe {
                        // unsafe missing is ok? but report
                        entries.push(ConfigDiffEntry::new(
                            key,
                            ConfigCategory::Unsafe,
                            "<missing>",
                            exp,
                            "unsafe key not present (ok)",
                        ));
                    }
                }
                Some(cur) => {
                    // Detect unsafe values
                    let lower = cur.to_ascii_lowercase();
                    let is_unsafe = lower == "*"
                        || lower == "0.0.0.0"
                        || cur.contains("password")
                        || cur.trim().is_empty() && *cat == ConfigCategory::Required;
                    if is_unsafe {
                        entries.push(ConfigDiffEntry::new(
                            key,
                            ConfigCategory::Unsafe,
                            cur,
                            exp,
                            "value is unsafe for production",
                        ));
                    } else if cur != exp && *cat == ConfigCategory::Required {
                        entries.push(ConfigDiffEntry::new(
                            key,
                            ConfigCategory::Required,
                            cur,
                            exp,
                            "value differs from expected",
                        ));
                    }
                }
            }
        }
        // Extra keys not in expected schema
        for (k, v) in current {
            if !expected.contains_key(k) {
                entries.push(ConfigDiffEntry::new(
                    k,
                    ConfigCategory::Unknown,
                    v,
                    "<unknown>",
                    "extra key not in schema",
                ));
            }
        }
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        Self::new(entries, generated_at)
    }

    pub fn has_unsafe(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.category == ConfigCategory::Unsafe)
    }
    pub fn missing_required(&self) -> Vec<&ConfigDiffEntry> {
        self.entries
            .iter()
            .filter(|e| e.category == ConfigCategory::Required && e.current == "<missing>")
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn detects_missing_required() {
        let cur = BTreeMap::new();
        let mut exp = BTreeMap::new();
        exp.insert(
            "DATABASE_URL".into(),
            (ConfigCategory::Required, "postgres://...".into()),
        );
        let r = ConfigDiffReport::compare(&cur, &exp, "2026-09-24T00:00:00Z");
        assert_eq!(r.missing_required().len(), 1);
    }

    #[test]
    fn redacts_secret_current() {
        let e = ConfigDiffEntry::new(
            "API_KEY",
            ConfigCategory::Required,
            "secret123",
            "ref",
            "detail",
        );
        assert_eq!(e.redacted_current(), "<redacted>");
        let e2 = ConfigDiffEntry::new("LOG_LEVEL", ConfigCategory::Required, "info", "info", "ok");
        assert_eq!(e2.redacted_current(), "info");
    }

    #[test]
    fn detects_unsafe_wildcard() {
        let mut cur = BTreeMap::new();
        cur.insert("CORS_ORIGINS".into(), "*".into());
        let mut exp = BTreeMap::new();
        exp.insert(
            "CORS_ORIGINS".into(),
            (ConfigCategory::Required, "https://example.com".into()),
        );
        let r = ConfigDiffReport::compare(&cur, &exp, "now");
        assert!(r.has_unsafe());
    }

    #[test]
    fn extra_key_is_unknown() {
        let mut cur = BTreeMap::new();
        cur.insert("EXTRA".into(), "value".into());
        let exp = BTreeMap::new();
        let r = ConfigDiffReport::compare(&cur, &exp, "now");
        assert!(r
            .entries
            .iter()
            .any(|e| e.category == ConfigCategory::Unknown));
    }

    #[test]
    fn deterministic_sorting() {
        let mut cur = BTreeMap::new();
        cur.insert("Z_KEY".into(), "1".into());
        cur.insert("A_KEY".into(), "1".into());
        let mut exp = BTreeMap::new();
        exp.insert("Z_KEY".into(), (ConfigCategory::Optional, "1".into()));
        exp.insert("A_KEY".into(), (ConfigCategory::Optional, "1".into()));
        let _r = ConfigDiffReport::compare(&cur, &exp, "now");
        // entries sorted by key — but with no diff, entries empty; test sorting via extra path
        let mut cur2 = BTreeMap::new();
        cur2.insert("Z_KEY".into(), "x".into());
        cur2.insert("A_KEY".into(), "x".into());
        let r2 = ConfigDiffReport::compare(&cur2, &BTreeMap::new(), "now");
        assert_eq!(r2.entries[0].key, "A_KEY");
        assert_eq!(r2.entries[1].key, "Z_KEY");
    }
}
