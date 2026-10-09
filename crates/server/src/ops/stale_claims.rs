//! Detect stale/currently contradictory claims in docs and release files (Batch 3).
//!
//! Searches for known historical phrases: old migration count, old workspace member count,
//! old test counts, old file counts, production verified, live trading verified, external audit complete, remote custody verified.
//! Returns machine-readable findings. Historical sections explicitly marked HISTORICAL should not be falsely flagged.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleFinding {
    pub file: String,
    pub line: usize,
    pub phrase: String,
    pub context: String,
    pub is_historical: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleReport {
    pub scanned_files: usize,
    pub findings: Vec<StaleFinding>,
    pub stale_count: usize, // non-historical
    pub historical_count: usize,
}

const STALE_PHRASES: &[&str] = &[
    "11 migrations",
    "18 migrations",
    "146 files",
    "183 files",
    "production verified",
    "live trading verified",
    "external audit complete",
    "remote custody verified",
    "funded live trading verified",
];

const TEST_COUNT_PHRASE: &str = "old test total";

pub fn scan_content(file: &str, content: &str) -> Vec<StaleFinding> {
    let mut out = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let lline = line.to_ascii_lowercase();
        let historical_context = is_historical_context(line, &lines, idx);
        for phrase in STALE_PHRASES {
            if lline.contains(&phrase.to_ascii_lowercase()) {
                out.push(StaleFinding {
                    file: file.to_string(),
                    line: idx + 1,
                    phrase: phrase.to_string(),
                    context: line.trim().chars().take(120).collect(),
                    is_historical: historical_context,
                });
            }
        }
        // Generic old test total detection
        if lline.contains("521 tests")
            || lline.contains("537 tests")
            || lline.contains("old test total")
        {
            // Check if marked historical
            out.push(StaleFinding {
                file: file.to_string(),
                line: idx + 1,
                phrase: TEST_COUNT_PHRASE.to_string(),
                context: line.trim().chars().take(120).collect(),
                is_historical: historical_context,
            });
        }
    }
    out
}

fn is_historical_context(line: &str, all_lines: &[&str], idx: usize) -> bool {
    // If line itself contains HISTORICAL or historical markers, or nearby lines do
    let window = 3usize;
    let start = idx.saturating_sub(window);
    let end = (idx + window + 1).min(all_lines.len());
    for l in &all_lines[start..end] {
        if l.to_ascii_uppercase().contains("HISTORICAL") {
            return true;
        }
        if l.to_ascii_lowercase().contains("historical") {
            return true;
        }
    }
    line.to_ascii_uppercase().contains("HISTORICAL")
}

pub fn scan_files(files: &[(String, String)]) -> StaleReport {
    let mut findings = Vec::new();
    for (path, content) in files {
        findings.extend(scan_content(path, content));
    }
    let stale_count = findings.iter().filter(|f| !f.is_historical).count();
    let historical_count = findings.iter().filter(|f| f.is_historical).count();
    StaleReport {
        scanned_files: files.len(),
        findings,
        stale_count,
        historical_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_stale_migration() {
        let content = "Current migrations: 18 migrations\n";
        let findings = scan_content("docs/file.md", content);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].phrase, "18 migrations");
        assert!(!findings[0].is_historical);
    }

    #[test]
    fn historical_not_flagged_as_stale() {
        let content = "HISTORICAL — 18 migrations at 2026-09-18\n";
        let findings = scan_content("docs/file.md", content);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].is_historical);
    }

    #[test]
    fn production_verified_flagged() {
        let content = "We are production verified and ready\n";
        let findings = scan_content("docs/file.md", content);
        assert!(findings.iter().any(|f| f.phrase == "production verified"));
    }

    #[test]
    fn no_false_positive_on_current() {
        let content = "Current migrations: 21 migrations\nCurrent members: 8\n";
        let findings = scan_content("docs/file.md", content);
        assert!(findings.is_empty());
    }

    #[test]
    fn scan_files_counts() {
        let files = vec![
            ("a.md".to_string(), "11 migrations\n".to_string()),
            ("b.md".to_string(), "HISTORICAL 11 migrations\n".to_string()),
        ];
        let report = scan_files(&files);
        assert_eq!(report.stale_count, 1);
        assert_eq!(report.historical_count, 1);
    }
}
