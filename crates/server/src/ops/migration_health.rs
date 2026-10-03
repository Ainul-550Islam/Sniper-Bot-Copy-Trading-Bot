//! Schema/binary agreement: what this build EXPECTS versus what the
//! database HAS (P1).
//!
//! # The failure this exists to catch
//!
//! Migrations are embedded in the binary (`bot_core::db::MIGRATOR`) and
//! production runs with `DATABASE_AUTO_MIGRATE=false` (see
//! `deploy/environments/production.env.template`), which is correct: a
//! replica racing to migrate on boot is how two replicas end up applying
//! the same DDL at once. But it creates a specific, silent, dangerous
//! state — **a binary whose queries reference columns the database does
//! not have**. The process starts, `/health` is 200, `/ready` is 200,
//! and the first query that touches the new column fails at request
//! time, per request, under load.
//!
//! The reverse is just as real during a rollback: the database is AHEAD
//! of the binary. That is usually survivable (the old binary simply
//! ignores the new columns) and is explicitly NOT reported as fatal —
//! but the operator must be told, because
//! `scripts/rollback-release.sh` cannot undo a migration and the
//! deployment ledger will not tell them either.
//!
//! And the one that matters most: a **checksum mismatch**. The migration
//! file in this build and the migration recorded as applied have the
//! same version number but different content. That means someone edited
//! an applied migration, or two branches both claimed `0036`, or the
//! database belongs to a different deployment entirely. There is no safe
//! automatic response to that; it is reported as `Critical` and a human
//! has to look.
//!
//! # What this module is NOT
//!
//! It does not migrate, repair, or advise a repair. It reports. The
//! decision to apply a pending migration is a deploy-time action with a
//! backup taken first (`docs/BACKUP-RESTORE.md`), not something a health
//! endpoint should trigger.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Value};

use bot_core::db::{Database, MIGRATOR};

/// How the database compares to the binary. Ordered least to most
/// severe so the overall state is `max()` of the parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationState {
    /// Every embedded migration is applied, with matching checksums, and
    /// the database holds nothing this build does not know about.
    InSync,
    /// The database has migrations this build does not contain. Normal
    /// during a rollback or a staged rollout; the old binary ignores the
    /// newer schema. Reported, not failed.
    Ahead,
    /// Migrations this build expects are NOT applied. Queries that use
    /// them will fail at request time.
    Pending,
    /// A migration was recorded as started and never completed, or
    /// completed unsuccessfully. The schema is in an unknown state.
    Dirty,
    /// An applied migration's checksum differs from this build's copy of
    /// the same version. Two different definitions of one version.
    ChecksumMismatch,
}

impl MigrationState {
    /// Stable wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            MigrationState::InSync => "in_sync",
            MigrationState::Ahead => "ahead",
            MigrationState::Pending => "pending",
            MigrationState::Dirty => "dirty",
            MigrationState::ChecksumMismatch => "checksum_mismatch",
        }
    }

    /// Whether this state means the deployment can serve traffic safely.
    ///
    /// `Ahead` is healthy ON PURPOSE: refusing to serve during a planned
    /// rollback would turn a controlled downgrade into an outage, which
    /// is the opposite of what a rollback is for.
    pub fn is_healthy(&self) -> bool {
        matches!(self, MigrationState::InSync | MigrationState::Ahead)
    }

    /// HTTP status for the endpoint. Unhealthy is 503, not 500: the
    /// process is fine, the deployment is not, and 503 is the status an
    /// orchestrator and a load balancer both already understand.
    pub fn http_status(&self) -> u16 {
        if self.is_healthy() {
            200
        } else {
            503
        }
    }
}

/// One migration version recorded in the database.
#[derive(Debug, Clone)]
pub struct AppliedMigration {
    /// The version number (`0036` is `36`).
    pub version: i64,
    /// The description sqlx derived from the filename.
    pub description: String,
    /// sqlx's checksum of the SQL text that was applied.
    pub checksum: Vec<u8>,
    /// False when the migration did not finish successfully.
    pub success: bool,
    /// When it was applied, RFC3339. `None` when the column was null.
    pub installed_on: Option<String>,
}

/// The comparison for a single version.
#[derive(Debug, Clone, Serialize)]
pub struct VersionReport {
    /// Version number.
    pub version: i64,
    /// Description from the binary when known, else from the database.
    pub description: String,
    /// Present in this build's embedded set.
    pub embedded: bool,
    /// Present in the database.
    pub applied: bool,
    /// Checksums agree. `None` when the version is not in both places.
    pub checksum_matches: Option<bool>,
    /// The database recorded the migration as successful.
    pub success: Option<bool>,
    /// When it was applied.
    pub installed_on: Option<String>,
}

/// The whole picture.
#[derive(Debug, Clone, Serialize)]
pub struct MigrationHealthReport {
    /// Aggregate state — the most severe of the per-version findings.
    pub state: MigrationState,
    /// Highest version embedded in this binary.
    pub binary_high_water: Option<i64>,
    /// Highest version applied in the database.
    pub database_high_water: Option<i64>,
    /// Count embedded in this binary.
    pub embedded_count: usize,
    /// Count recorded as applied.
    pub applied_count: usize,
    /// Versions this build expects that are not applied.
    pub pending_versions: Vec<i64>,
    /// Versions applied that this build does not contain.
    pub unknown_versions: Vec<i64>,
    /// Versions whose checksums disagree.
    pub checksum_mismatches: Vec<i64>,
    /// Versions recorded as applied but not successful.
    pub failed_versions: Vec<i64>,
    /// Per-version detail, ascending.
    pub versions: Vec<VersionReport>,
    /// One sentence an operator can act on.
    pub summary: String,
}

impl MigrationHealthReport {
    /// JSON body for the endpoint.
    ///
    /// Deliberately free of connection strings, hostnames and checksum
    /// bytes: checksums are reported as a boolean match, never as
    /// values, because the hash of a migration is of no use to an
    /// operator and the field would invite pasting schema internals into
    /// a ticket.
    pub fn to_json(&self) -> Value {
        json!({
            "state": self.state.as_str(),
            "healthy": self.state.is_healthy(),
            "binary_high_water": self.binary_high_water,
            "database_high_water": self.database_high_water,
            "embedded_count": self.embedded_count,
            "applied_count": self.applied_count,
            "pending_versions": self.pending_versions,
            "unknown_versions": self.unknown_versions,
            "checksum_mismatches": self.checksum_mismatches,
            "failed_versions": self.failed_versions,
            "versions": self.versions,
            "summary": self.summary,
        })
    }
}

/// Compare this build's embedded migrations with what the database says
/// is applied. Pure: no I/O, so every branch is unit-testable.
pub fn compare(
    embedded: &[(i64, String, Vec<u8>)],
    applied: &[AppliedMigration],
) -> MigrationHealthReport {
    let embedded_by_version: BTreeMap<i64, (&str, &[u8])> = embedded
        .iter()
        .map(|(v, d, c)| (*v, (d.as_str(), c.as_slice())))
        .collect();
    let applied_by_version: BTreeMap<i64, &AppliedMigration> =
        applied.iter().map(|m| (m.version, m)).collect();

    let mut versions: Vec<VersionReport> = Vec::new();
    let mut pending_versions = Vec::new();
    let mut unknown_versions = Vec::new();
    let mut checksum_mismatches = Vec::new();
    let mut failed_versions = Vec::new();

    let all: BTreeMap<i64, ()> = embedded_by_version
        .keys()
        .chain(applied_by_version.keys())
        .map(|v| (*v, ()))
        .collect();

    for version in all.keys().copied() {
        let in_binary = embedded_by_version.get(&version);
        let in_db = applied_by_version.get(&version);

        let checksum_matches = match (in_binary, in_db) {
            (Some((_, embedded_sum)), Some(applied)) => {
                let matches = *embedded_sum == applied.checksum.as_slice();
                if !matches {
                    checksum_mismatches.push(version);
                }
                Some(matches)
            }
            _ => None,
        };

        match (in_binary.is_some(), in_db.is_some()) {
            (true, false) => pending_versions.push(version),
            (false, true) => unknown_versions.push(version),
            _ => {}
        }
        if let Some(applied) = in_db {
            if !applied.success {
                failed_versions.push(version);
            }
        }

        versions.push(VersionReport {
            version,
            description: in_binary
                .map(|(d, _)| (*d).to_string())
                .or_else(|| in_db.map(|m| m.description.clone()))
                .unwrap_or_default(),
            embedded: in_binary.is_some(),
            applied: in_db.is_some(),
            checksum_matches,
            success: in_db.map(|m| m.success),
            installed_on: in_db.and_then(|m| m.installed_on.clone()),
        });
    }

    // Severity order: a checksum mismatch outranks a dirty migration,
    // which outranks pending work, which outranks merely being behind
    // the database. Reporting only the worst finding would hide the
    // others, so all the lists are always populated; `state` is just the
    // headline.
    let state = if !checksum_mismatches.is_empty() {
        MigrationState::ChecksumMismatch
    } else if !failed_versions.is_empty() {
        MigrationState::Dirty
    } else if !pending_versions.is_empty() {
        MigrationState::Pending
    } else if !unknown_versions.is_empty() {
        MigrationState::Ahead
    } else {
        MigrationState::InSync
    };

    let summary = match state {
        MigrationState::InSync => format!(
            "schema matches this build ({} migrations applied)",
            applied_by_version.len()
        ),
        MigrationState::Ahead => format!(
            "database is ahead of this build by {} migration(s) ({:?}) — expected during a rollback; this build ignores them",
            unknown_versions.len(),
            unknown_versions
        ),
        MigrationState::Pending => format!(
            "{} migration(s) this build requires are NOT applied ({:?}) — queries using them will fail; run the migration step before serving traffic",
            pending_versions.len(),
            pending_versions
        ),
        MigrationState::Dirty => format!(
            "migration(s) {:?} are recorded as applied but did NOT succeed — the schema is in an unknown state; do not deploy over it",
            failed_versions
        ),
        MigrationState::ChecksumMismatch => format!(
            "migration(s) {:?} differ between this build and the database — an applied migration was edited, two branches claimed one version, or this is another deployment's database; stop and investigate",
            checksum_mismatches
        ),
    };

    MigrationHealthReport {
        state,
        binary_high_water: embedded_by_version.keys().next_back().copied(),
        database_high_water: applied_by_version.keys().next_back().copied(),
        embedded_count: embedded_by_version.len(),
        applied_count: applied_by_version.len(),
        pending_versions,
        unknown_versions,
        checksum_mismatches,
        failed_versions,
        versions,
        summary,
    }
}

/// The migrations compiled into this binary.
pub fn embedded_migrations() -> Vec<(i64, String, Vec<u8>)> {
    MIGRATOR
        .iter()
        .map(|m| (m.version, m.description.to_string(), m.checksum.to_vec()))
        .collect()
}

/// Read `_sqlx_migrations`.
///
/// A missing table is NOT an error here: it is the honest answer "no
/// migration has ever run against this database", which `compare` then
/// reports as every embedded migration pending. Any other database error
/// propagates — reporting "in sync" because a query failed would be the
/// worst possible lie for this particular endpoint.
pub async fn applied_migrations(db: &Database) -> Result<Vec<AppliedMigration>, String> {
    use sqlx::Row;

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_name = '_sqlx_migrations')",
    )
    .fetch_one(db.pool())
    .await
    .map_err(|e| format!("could not inspect the migration table: {e}"))?;
    if !exists {
        return Ok(Vec::new());
    }

    let rows = sqlx::query(
        "SELECT version, description, checksum, success, installed_on \
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(db.pool())
    .await
    .map_err(|e| format!("could not read the migration table: {e}"))?;

    Ok(rows
        .into_iter()
        .map(|row| AppliedMigration {
            version: row.get::<i64, _>("version"),
            description: row.get::<String, _>("description"),
            checksum: row.get::<Vec<u8>, _>("checksum"),
            success: row.get::<bool, _>("success"),
            installed_on: row
                .try_get::<chrono::DateTime<chrono::Utc>, _>("installed_on")
                .ok()
                .map(|t| t.to_rfc3339()),
        })
        .collect())
}

/// The full check against a live database.
pub async fn evaluate(db: &Database) -> Result<MigrationHealthReport, String> {
    let applied = applied_migrations(db).await?;
    Ok(compare(&embedded_migrations(), &applied))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded(versions: &[(i64, &str)]) -> Vec<(i64, String, Vec<u8>)> {
        versions
            .iter()
            .map(|(v, d)| (*v, (*d).to_string(), vec![*v as u8, 0xAA]))
            .collect()
    }

    fn applied(
        version: i64,
        description: &str,
        checksum: Vec<u8>,
        success: bool,
    ) -> AppliedMigration {
        AppliedMigration {
            version,
            description: description.to_string(),
            checksum,
            success,
            installed_on: Some("2026-10-02T00:00:00Z".to_string()),
        }
    }

    #[test]
    fn identical_sets_are_in_sync() {
        let e = embedded(&[(1, "init"), (2, "tenants")]);
        let a = vec![
            applied(1, "init", vec![1, 0xAA], true),
            applied(2, "tenants", vec![2, 0xAA], true),
        ];
        let report = compare(&e, &a);
        assert_eq!(report.state, MigrationState::InSync);
        assert!(report.state.is_healthy());
        assert_eq!(report.state.http_status(), 200);
        assert_eq!(report.binary_high_water, Some(2));
        assert_eq!(report.database_high_water, Some(2));
    }

    #[test]
    fn a_binary_ahead_of_the_database_is_pending_and_unhealthy() {
        // This is the dangerous one: the build's queries reference a
        // schema the database does not have.
        let e = embedded(&[(1, "init"), (2, "tenants"), (3, "durable_controls")]);
        let a = vec![applied(1, "init", vec![1, 0xAA], true)];
        let report = compare(&e, &a);
        assert_eq!(report.state, MigrationState::Pending);
        assert_eq!(report.pending_versions, vec![2, 3]);
        assert!(!report.state.is_healthy());
        assert_eq!(report.state.http_status(), 503);
        assert!(report.summary.contains("NOT applied"));
    }

    #[test]
    fn a_database_ahead_of_the_binary_is_reported_but_healthy() {
        // A rollback leaves exactly this state. Failing here would turn
        // a controlled downgrade into an outage.
        let e = embedded(&[(1, "init")]);
        let a = vec![
            applied(1, "init", vec![1, 0xAA], true),
            applied(2, "newer", vec![2, 0xAA], true),
        ];
        let report = compare(&e, &a);
        assert_eq!(report.state, MigrationState::Ahead);
        assert_eq!(report.unknown_versions, vec![2]);
        assert!(report.state.is_healthy());
        assert_eq!(report.state.http_status(), 200);
        assert!(report.summary.contains("rollback"));
    }

    #[test]
    fn an_edited_migration_is_a_checksum_mismatch() {
        let e = embedded(&[(1, "init")]);
        let a = vec![applied(1, "init", vec![0xFF, 0xFF], true)];
        let report = compare(&e, &a);
        assert_eq!(report.state, MigrationState::ChecksumMismatch);
        assert_eq!(report.checksum_mismatches, vec![1]);
        assert!(!report.state.is_healthy());
    }

    #[test]
    fn an_unsuccessful_migration_is_dirty() {
        let e = embedded(&[(1, "init"), (2, "tenants")]);
        let a = vec![
            applied(1, "init", vec![1, 0xAA], true),
            applied(2, "tenants", vec![2, 0xAA], false),
        ];
        let report = compare(&e, &a);
        assert_eq!(report.state, MigrationState::Dirty);
        assert_eq!(report.failed_versions, vec![2]);
        assert!(!report.state.is_healthy());
    }

    #[test]
    fn a_checksum_mismatch_outranks_everything_else() {
        let e = embedded(&[(1, "init"), (2, "tenants"), (3, "pending")]);
        let a = vec![
            applied(1, "init", vec![0xFF], true),        // mismatch
            applied(2, "tenants", vec![2, 0xAA], false), // dirty
            applied(9, "unknown", vec![9, 0xAA], true),  // ahead
        ];
        let report = compare(&e, &a);
        assert_eq!(report.state, MigrationState::ChecksumMismatch);
        // every finding is still reported, not just the headline
        assert_eq!(report.checksum_mismatches, vec![1]);
        assert_eq!(report.failed_versions, vec![2]);
        assert_eq!(report.pending_versions, vec![3]);
        assert_eq!(report.unknown_versions, vec![9]);
    }

    #[test]
    fn an_empty_database_means_everything_is_pending() {
        let e = embedded(&[(1, "init"), (2, "tenants")]);
        let report = compare(&e, &[]);
        assert_eq!(report.state, MigrationState::Pending);
        assert_eq!(report.pending_versions, vec![1, 2]);
        assert_eq!(report.database_high_water, None);
    }

    #[test]
    fn the_report_never_carries_checksum_bytes_or_connection_detail() {
        let e = embedded(&[(1, "init")]);
        let a = vec![applied(1, "init", vec![0xDE, 0xAD, 0xBE, 0xEF], true)];
        let body = compare(&e, &a).to_json().to_string().to_ascii_lowercase();
        for banned in ["deadbeef", "postgres://", "password", "222", "checksum\":["] {
            assert!(!body.contains(banned), "report leaked {banned}: {body}");
        }
        // The boolean verdict IS present — that is the useful part.
        assert!(body.contains("checksum_matches"));
    }

    #[test]
    fn this_builds_embedded_set_is_contiguous_and_non_empty() {
        // Guards the repository itself: a gap in the migration series
        // means a file was deleted or mis-numbered.
        let embedded = embedded_migrations();
        assert!(!embedded.is_empty(), "the binary embeds no migrations");
        let mut versions: Vec<i64> = embedded.iter().map(|(v, _, _)| *v).collect();
        versions.sort_unstable();
        for (index, version) in versions.iter().enumerate() {
            assert_eq!(
                *version,
                index as i64 + 1,
                "migration series is not contiguous at position {index}: {versions:?}"
            );
        }
    }

    #[test]
    fn comparing_the_real_embedded_set_against_itself_is_in_sync() {
        let embedded = embedded_migrations();
        let applied: Vec<AppliedMigration> = embedded
            .iter()
            .map(|(v, d, c)| applied(*v, d, c.clone(), true))
            .collect();
        let report = compare(&embedded, &applied);
        assert_eq!(report.state, MigrationState::InSync);
        assert_eq!(report.applied_count, embedded.len());
    }
}
