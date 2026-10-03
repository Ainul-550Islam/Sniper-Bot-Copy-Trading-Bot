//! The backup/restore ledger: what the deployment can actually prove
//! about its own data protection (P1, SLO D1/D2).
//!
//! # Why this replaces a hard-coded answer
//!
//! `/api/saas/backup/status` used to return `retention_configured:
//! true` and `last_backup_at: now()` as literals, with no backup
//! machinery of any kind behind them. That is the worst class of bug in
//! this repository: it compiles, it passes its test, it returns 200, and
//! it tells a paying customer their data is protected when nothing is
//! taking backups at all. A customer would discover the truth on the one
//! day it matters.
//!
//! This module reports only what is written in an append-only ledger by
//! `scripts/backup-postgres.sh` and `scripts/verify-backup-restore.sh`.
//! No ledger means **`not_configured`** — stated plainly, not dressed up
//! as healthy.
//!
//! # The ledger format
//!
//! JSON Lines, append-only, one record per event. Written by shell, read
//! here, so the format is deliberately dull:
//!
//! ```json
//! {"event":"backup","at":"2026-10-02T02:00:04Z","backup_id":"pg-20261002T020004Z",
//!  "sha256":"…","size_bytes":418291,"encrypted":true,"retention_days":30,"ok":true}
//! {"event":"restore_verified","at":"2026-10-02T03:00:51Z","backup_id":"pg-20261002T020004Z",
//!  "tables":42,"migration_high_water":36,"ok":true}
//! ```
//!
//! An unparsable line is skipped rather than failing the whole read: a
//! truncated final line (the writer was killed mid-append) must not blind
//! the operator to the 300 good records above it.
//!
//! # What is never reported
//!
//! No path, no bucket, no hostname, no connection string. A tenant
//! asking "am I backed up?" gets freshness and verification facts; the
//! location of the backups is operator information and a target list.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

/// Environment variable naming the ledger file.
pub const LEDGER_PATH_ENV: &str = "BACKUP_LEDGER_PATH";

/// Where the compose stack writes it by default (inside the container).
pub const DEFAULT_LEDGER_PATH: &str = "/app/data/backups/backups.jsonl";

/// A backup older than this is stale for a daily schedule: one missed
/// night is a warning, not yet a crisis, but two is a pattern.
const BACKUP_STALE_AFTER_HOURS: i64 = 36;

/// Quarterly restore drills (SLO D2). Past this, the last proof that a
/// backup can actually be restored is too old to trust.
const RESTORE_DRILL_MAX_AGE_DAYS: i64 = 90;

/// An off-site copy older than this means the remote is drifting behind
/// the local backups. Deliberately looser than the local threshold: the
/// sync runs after the dump, and a single missed upload should read as
/// "stale", not as a broken schedule.
const OFFSITE_STALE_AFTER_HOURS: i64 = 48;

/// How long the WAL archive may go quiet before it is reported stale.
///
/// Deliberately generous, because an IDLE database produces no WAL at
/// all: without `archive_timeout` set, a quiet weekend would otherwise
/// read as a broken archive. The compose overlay sets
/// `archive_timeout = 300s`, which forces a segment switch on a timer
/// and makes silence here mean what it says.
const WAL_STALE_AFTER_MINUTES: i64 = 60;

/// A base backup older than this cannot be rolled forward cheaply: every
/// extra day is another day of WAL to replay (and to keep on disk).
const BASE_BACKUP_STALE_AFTER_DAYS: i64 = 8;

/// Point-in-time recovery, like the logical restore, is a claim until a
/// drill proves it. Same quarterly window as [`RESTORE_DRILL_MAX_AGE_DAYS`].
const PITR_DRILL_MAX_AGE_DAYS: i64 = 90;

/// Overall data-protection posture. Ordered best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectionState {
    /// Recent successful backup AND a restore drill inside the window.
    Verified,
    /// Backups are running, but no restore has ever been verified (or
    /// the last drill is older than the window). An unrestored backup is
    /// a hypothesis, not a protection.
    Unverified,
    /// A ledger exists but the most recent backup is too old.
    Stale,
    /// The most recent backup attempt failed.
    Failing,
    /// No ledger: nothing is taking backups in this deployment.
    NotConfigured,
}

impl ProtectionState {
    /// Stable wire string.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProtectionState::Verified => "verified",
            ProtectionState::Unverified => "unverified",
            ProtectionState::Stale => "stale",
            ProtectionState::Failing => "failing",
            ProtectionState::NotConfigured => "not_configured",
        }
    }

    /// Whether the deployment can claim its data is protected.
    ///
    /// Only `Verified` qualifies. `Unverified` is deliberately NOT
    /// healthy: a backup nobody has ever restored is an untested
    /// assumption, and SLO D2 exists precisely because untested backups
    /// fail at restore time.
    pub fn is_protected(&self) -> bool {
        matches!(self, ProtectionState::Verified)
    }
}

/// One ledger record, after parsing.
#[derive(Debug, Clone, Deserialize)]
struct LedgerRecord {
    event: String,
    at: DateTime<Utc>,
    // `backup_id` and `sha256` are present in the file but deliberately
    // NOT parsed here: nothing in a tenant-facing posture may depend on
    // them, and a field that is never read is a field that cannot leak.
    #[serde(default)]
    ok: Option<bool>,
    #[serde(default)]
    encrypted: Option<bool>,
    #[serde(default)]
    retention_days: Option<i64>,
    /// Off-site scope: "dumps" or "dumps+pitr". A sync that copies the
    /// dumps but leaves the WAL archive behind protects a different,
    /// smaller promise, and the posture must not blur the two.
    #[serde(default)]
    scope: Option<String>,
}

/// Geographic redundancy, kept as a SEPARATE axis from
/// [`ProtectionState`] on purpose.
///
/// Conflating them would make one of two lies unavoidable: either a
/// deployment with proven local restores is reported as unprotected
/// (false alarm, and operators learn to ignore the field), or a
/// deployment whose only copies sit on the same disk as the database is
/// reported as protected against losing that disk (the lie this whole
/// module exists to stop). So `protected` answers "can this data be
/// restored, proven by drill", and `offsite` answers "would a copy
/// survive losing this host" — and the summary says which one is true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsiteState {
    /// No off-site target has ever been recorded. A valid (documented)
    /// single-host choice — but losing the host loses the backups too.
    NotConfigured,
    /// Off-site copies are being made and the latest one is current.
    Current,
    /// Off-site copies were made, but the newest is older than the
    /// threshold: the remote copy is drifting behind the local one.
    Stale,
    /// The most recent off-site attempt FAILED.
    Failing,
}

impl OffsiteState {
    pub fn as_str(&self) -> &'static str {
        match self {
            OffsiteState::NotConfigured => "not_configured",
            OffsiteState::Current => "current",
            OffsiteState::Stale => "stale",
            OffsiteState::Failing => "failing",
        }
    }
}

/// Point-in-time recovery: the third axis, and the one that decides the
/// recovery point objective.
///
/// Dumps alone put the RPO at the backup interval — up to 24 hours of
/// trades on the default schedule. WAL archiving plus a base backup
/// moves it to the WAL segment interval. The axis is reported
/// separately for the same reason `offsite` is: it answers a different
/// question ("how much would we lose?") from `protected` ("can we get
/// anything back at all?").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitrState {
    /// No WAL archiving and no base backup has ever been recorded. The
    /// RPO is the dump schedule, and the summary says so.
    NotConfigured,
    /// The most recent WAL archive or base backup attempt FAILED. On a
    /// failing archive Postgres retains WAL locally — pg_wal grows and
    /// will eventually stop the database, which is the loud failure the
    /// design wants, but only if somebody is told.
    Failing,
    /// Archiving works, but the archive has gone quiet past
    /// [`WAL_STALE_AFTER_MINUTES`] or the newest base backup is older
    /// than [`BASE_BACKUP_STALE_AFTER_DAYS`].
    Stale,
    /// Archive and base are both current, but no point-in-time recovery
    /// has ever been PROVEN inside the quarterly window.
    Unverified,
    /// Current, with a proven recovery inside the window.
    Current,
}

impl PitrState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PitrState::NotConfigured => "not_configured",
            PitrState::Failing => "failing",
            PitrState::Stale => "stale",
            PitrState::Unverified => "unverified",
            PitrState::Current => "current",
        }
    }
}

/// Where the reported recovery point objective came from. A number
/// without its basis invites the reader to assume the better one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpoBasis {
    /// Derived from the last archived WAL segment (minutes, not hours).
    WalArchive,
    /// Derived from the last successful dump — the whole backup interval
    /// is at risk.
    LastBackup,
    /// Nothing recorded; no honest number can be given.
    Unknown,
}

impl RpoBasis {
    pub fn as_str(&self) -> &'static str {
        match self {
            RpoBasis::WalArchive => "wal_archive",
            RpoBasis::LastBackup => "last_backup",
            RpoBasis::Unknown => "unknown",
        }
    }
}

/// What the ledger proves.
#[derive(Debug, Clone)]
pub struct BackupPosture {
    /// Overall state.
    pub state: ProtectionState,
    /// When the last SUCCESSFUL backup completed.
    pub last_backup_at: Option<DateTime<Utc>>,
    /// When the last SUCCESSFUL restore drill completed.
    pub last_verified_restore_at: Option<DateTime<Utc>>,
    /// Whether the last backup attempt (successful or not) failed.
    pub last_attempt_failed: bool,
    /// Retention window the writer recorded, in days.
    pub retention_days: Option<i64>,
    /// Every successful backup in the ledger was encrypted.
    pub encrypted: Option<bool>,
    /// Successful backup records in the ledger.
    pub backup_count: usize,
    /// Whether this deployment has ever recorded an off-site copy. False
    /// means the backups live on the same host as the database.
    pub offsite_configured: bool,
    /// When the last SUCCESSFUL off-site sync completed.
    pub last_offsite_sync_at: Option<DateTime<Utc>>,
    /// Off-site state, evaluated on its own axis — see [`OffsiteState`].
    pub offsite: OffsiteState,
    /// Whether the most recent successful off-site sync also carried the
    /// PITR artefacts (base backups + WAL archive). False with PITR
    /// configured means point-in-time recovery dies with this host.
    pub offsite_includes_pitr: bool,
    /// Point-in-time recovery state — see [`PitrState`].
    pub pitr: PitrState,
    /// When the last WAL segment was successfully archived.
    pub last_wal_archive_at: Option<DateTime<Utc>>,
    /// When the last physical base backup completed.
    pub last_base_backup_at: Option<DateTime<Utc>>,
    /// When a point-in-time recovery was last PROVEN by a drill.
    pub last_verified_pitr_at: Option<DateTime<Utc>>,
    /// Worst-case data loss in seconds, as evidenced by the ledger.
    /// `None` when nothing is recorded — an unknown RPO is reported as
    /// unknown rather than as zero.
    pub rpo_estimate_seconds: Option<i64>,
    /// Which record the estimate came from.
    pub rpo_basis: RpoBasis,
    /// One sentence for a human.
    pub summary: String,
}

impl BackupPosture {
    /// The tenant-facing body.
    ///
    /// Deliberately omits: file paths, bucket names, hostnames, backup
    /// ids and sizes. A tenant needs freshness and verification; the
    /// rest is operator information.
    pub fn to_json(&self, organization_id: &str) -> Value {
        json!({
            "organization_id": organization_id,
            "state": self.state.as_str(),
            "protected": self.state.is_protected(),
            "last_backup_at": self.last_backup_at.map(|t| t.to_rfc3339()),
            "last_verified_restore": self.last_verified_restore_at.map(|t| t.to_rfc3339()),
            "retention_days": self.retention_days,
            "encrypted": self.encrypted,
            "backup_count": self.backup_count,
            // Reported as its own object, never folded into `protected`:
            // a customer asking "would you survive losing the host?"
            // deserves a direct answer instead of inferring one.
            "offsite": {
                "state": self.offsite.as_str(),
                "configured": self.offsite_configured,
                "last_sync_at": self.last_offsite_sync_at.map(|t| t.to_rfc3339()),
                "includes_pitr": self.offsite_includes_pitr,
            },
            // The third axis: how much data a disaster would cost. A
            // customer reading `protected: true` on a dump-only
            // deployment would otherwise never learn that "protected"
            // can still mean "up to a day of trades gone".
            "pitr": {
                "state": self.pitr.as_str(),
                "last_wal_archive_at": self.last_wal_archive_at.map(|t| t.to_rfc3339()),
                "last_base_backup_at": self.last_base_backup_at.map(|t| t.to_rfc3339()),
                "last_verified_pitr": self.last_verified_pitr_at.map(|t| t.to_rfc3339()),
            },
            "rpo_estimate_seconds": self.rpo_estimate_seconds,
            "rpo_basis": self.rpo_basis.as_str(),
            "summary": self.summary,
            "as_of": Utc::now().to_rfc3339(),
        })
    }
}

/// The configured ledger path.
pub fn ledger_path() -> PathBuf {
    std::env::var(LEDGER_PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_LEDGER_PATH))
}

/// Read and evaluate the ledger at the configured path.
pub fn posture() -> BackupPosture {
    posture_at(&ledger_path(), Utc::now())
}

/// Evaluate a specific ledger file against a specific clock (testable).
pub fn posture_at(path: &Path, now: DateTime<Utc>) -> BackupPosture {
    let Ok(contents) = std::fs::read_to_string(path) else {
        // Includes "file does not exist" and "cannot read it". Both mean
        // the same thing to a customer: this deployment cannot show
        // evidence of a backup.
        return BackupPosture {
            state: ProtectionState::NotConfigured,
            last_backup_at: None,
            last_verified_restore_at: None,
            last_attempt_failed: false,
            retention_days: None,
            encrypted: None,
            backup_count: 0,
            offsite_configured: false,
            last_offsite_sync_at: None,
            offsite: OffsiteState::NotConfigured,
            offsite_includes_pitr: false,
            pitr: PitrState::NotConfigured,
            last_wal_archive_at: None,
            last_base_backup_at: None,
            last_verified_pitr_at: None,
            rpo_estimate_seconds: None,
            rpo_basis: RpoBasis::Unknown,
            summary:
                "no backup ledger is present in this deployment — nothing is taking scheduled backups"
                    .to_string(),
        };
    };

    evaluate(&contents, now)
}

/// Parse ledger text and derive the posture.
pub fn evaluate(contents: &str, now: DateTime<Utc>) -> BackupPosture {
    let records: Vec<LedgerRecord> = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        // A malformed line is skipped, not fatal: a half-written final
        // record must not hide the history above it.
        .filter_map(|line| serde_json::from_str::<LedgerRecord>(line).ok())
        .collect();

    let successful_backups: Vec<&LedgerRecord> = records
        .iter()
        .filter(|r| r.event == "backup" && r.ok.unwrap_or(false))
        .collect();
    let last_backup_at = successful_backups.iter().map(|r| r.at).max();
    let last_verified_restore_at = records
        .iter()
        .filter(|r| r.event == "restore_verified" && r.ok.unwrap_or(false))
        .map(|r| r.at)
        .max();

    let last_attempt = records
        .iter()
        .filter(|r| r.event == "backup")
        .max_by_key(|r| r.at);
    let last_attempt_failed = last_attempt
        .map(|r| !r.ok.unwrap_or(false))
        .unwrap_or(false);

    let retention_days = successful_backups
        .iter()
        .max_by_key(|r| r.at)
        .and_then(|r| r.retention_days);
    let encrypted = if successful_backups.is_empty() {
        None
    } else {
        Some(
            successful_backups
                .iter()
                .all(|r| r.encrypted.unwrap_or(false)),
        )
    };

    // --- the off-site axis -------------------------------------------
    let offsite_records: Vec<&LedgerRecord> = records
        .iter()
        .filter(|r| r.event == "offsite_synced")
        .collect();
    let offsite_configured = !offsite_records.is_empty();
    let last_offsite_sync_at = offsite_records
        .iter()
        .filter(|r| r.ok.unwrap_or(false))
        .map(|r| r.at)
        .max();
    // Taken from the most recent SUCCESSFUL sync, not from any record:
    // a failed attempt that intended to carry PITR proves nothing about
    // what is actually off-site.
    let offsite_includes_pitr = offsite_records
        .iter()
        .filter(|r| r.ok.unwrap_or(false))
        .max_by_key(|r| r.at)
        .and_then(|r| r.scope.as_deref())
        .map(|scope| scope.contains("pitr"))
        .unwrap_or(false);
    let last_offsite_failed = offsite_records
        .iter()
        .max_by_key(|r| r.at)
        .map(|r| !r.ok.unwrap_or(false))
        .unwrap_or(false);
    let offsite = if !offsite_configured {
        OffsiteState::NotConfigured
    } else if last_offsite_failed {
        // Same rule as the local axis: the newest attempt wins. An older
        // success must not paper over a sync that is failing now.
        OffsiteState::Failing
    } else if last_offsite_sync_at
        .map(|t| now - t <= Duration::hours(OFFSITE_STALE_AFTER_HOURS))
        .unwrap_or(false)
    {
        OffsiteState::Current
    } else {
        OffsiteState::Stale
    };

    // --- the point-in-time-recovery axis ------------------------------
    let wal_records: Vec<&LedgerRecord> = records
        .iter()
        .filter(|r| r.event == "wal_archived")
        .collect();
    let base_records: Vec<&LedgerRecord> =
        records.iter().filter(|r| r.event == "basebackup").collect();

    let last_wal_archive_at = wal_records
        .iter()
        .filter(|r| r.ok.unwrap_or(false))
        .map(|r| r.at)
        .max();
    let last_base_backup_at = base_records
        .iter()
        .filter(|r| r.ok.unwrap_or(false))
        .map(|r| r.at)
        .max();
    let last_verified_pitr_at = records
        .iter()
        .filter(|r| r.event == "pitr_verified" && r.ok.unwrap_or(false))
        .map(|r| r.at)
        .max();

    let pitr_configured = !wal_records.is_empty() || !base_records.is_empty();
    let last_wal_failed = wal_records
        .iter()
        .max_by_key(|r| r.at)
        .map(|r| !r.ok.unwrap_or(false))
        .unwrap_or(false);
    let last_base_failed = base_records
        .iter()
        .max_by_key(|r| r.at)
        .map(|r| !r.ok.unwrap_or(false))
        .unwrap_or(false);

    let wal_fresh = last_wal_archive_at
        .map(|t| now - t <= Duration::minutes(WAL_STALE_AFTER_MINUTES))
        .unwrap_or(false);
    let base_fresh = last_base_backup_at
        .map(|t| now - t <= Duration::days(BASE_BACKUP_STALE_AFTER_DAYS))
        .unwrap_or(false);
    let pitr_drill_fresh = last_verified_pitr_at
        .map(|t| now - t <= Duration::days(PITR_DRILL_MAX_AGE_DAYS))
        .unwrap_or(false);

    let pitr = if !pitr_configured {
        PitrState::NotConfigured
    } else if last_wal_failed || last_base_failed {
        // Same rule as every other axis: the newest attempt decides.
        PitrState::Failing
    } else if !wal_fresh || !base_fresh {
        PitrState::Stale
    } else if !pitr_drill_fresh {
        PitrState::Unverified
    } else {
        PitrState::Current
    };

    // The RPO is only as good as the evidence behind it. WAL archiving
    // that is failing or stale must NOT be used as the basis — that is
    // precisely the situation where the optimistic number is wrong.
    let (rpo_estimate_seconds, rpo_basis) = match (
        matches!(pitr, PitrState::Current | PitrState::Unverified),
        last_wal_archive_at,
        last_backup_at,
    ) {
        (true, Some(t), _) => (Some((now - t).num_seconds().max(0)), RpoBasis::WalArchive),
        (_, _, Some(t)) => (Some((now - t).num_seconds().max(0)), RpoBasis::LastBackup),
        _ => (None, RpoBasis::Unknown),
    };

    let backup_fresh = last_backup_at
        .map(|t| now - t <= Duration::hours(BACKUP_STALE_AFTER_HOURS))
        .unwrap_or(false);
    let restore_fresh = last_verified_restore_at
        .map(|t| now - t <= Duration::days(RESTORE_DRILL_MAX_AGE_DAYS))
        .unwrap_or(false);

    // Order matters: a failing backup is reported even when an older
    // successful one is still inside the freshness window, because the
    // schedule is broken NOW and the window will expire.
    let state = if records.is_empty() {
        ProtectionState::NotConfigured
    } else if last_attempt_failed {
        ProtectionState::Failing
    } else if !backup_fresh {
        ProtectionState::Stale
    } else if !restore_fresh {
        ProtectionState::Unverified
    } else {
        ProtectionState::Verified
    };

    let summary = match state {
        ProtectionState::NotConfigured =>
            "the backup ledger is empty — no backup has ever been recorded in this deployment".to_string(),
        ProtectionState::Failing =>
            "the most recent backup attempt FAILED — fix the backup job before relying on the older copies".to_string(),
        ProtectionState::Stale => match last_backup_at {
            Some(t) => format!(
                "the last successful backup was {} hours ago, past the {BACKUP_STALE_AFTER_HOURS}h threshold — the schedule is not running",
                (now - t).num_hours()
            ),
            None => "no successful backup is recorded in the ledger".to_string(),
        },
        ProtectionState::Unverified => match last_verified_restore_at {
            Some(t) => format!(
                "backups are current, but the last verified RESTORE was {} days ago (limit {RESTORE_DRILL_MAX_AGE_DAYS}) — run a restore drill",
                (now - t).num_days()
            ),
            None =>
                "backups are current, but no restore has EVER been verified — an untested backup is an assumption, not a protection"
                    .to_string(),
        },
        ProtectionState::Verified =>
            "backups are current and a restore drill has been verified inside the quarterly window".to_string(),
    };

    // The local answer alone is misleading in exactly one direction, so
    // the sentence always ends by saying where the copies live.
    let summary = match offsite {
        OffsiteState::NotConfigured => format!(
            "{summary}; copies are LOCAL ONLY — losing this host would take the database and its backups together"
        ),
        OffsiteState::Failing => format!(
            "{summary}; the off-site copy is FAILING — only this host holds the backups right now"
        ),
        OffsiteState::Stale => match last_offsite_sync_at {
            Some(t) => format!(
                "{summary}; the last off-site copy was {} hours ago (limit {OFFSITE_STALE_AFTER_HOURS}h)",
                (now - t).num_hours()
            ),
            None => format!("{summary}; no off-site copy has ever succeeded"),
        },
        OffsiteState::Current => format!("{summary}; an off-site copy is current"),
    };

    // And finally: how much would a disaster cost? Stated in the unit
    // the reader cares about, with the basis attached so an optimistic
    // number cannot be quoted without its caveat.
    let summary = match (pitr, rpo_estimate_seconds, rpo_basis) {
        (PitrState::NotConfigured, Some(secs), _) => format!(
            "{summary}; there is no WAL archiving, so up to {} hours of trades would be lost (RPO = the dump interval)",
            (secs / 3600).max(1)
        ),
        (PitrState::NotConfigured, None, _) => format!(
            "{summary}; there is no WAL archiving and no dump to measure against — the recovery point is unknown"
        ),
        (PitrState::Failing, _, _) => format!(
            "{summary}; WAL archiving is FAILING — the archive is behind and pg_wal will keep growing until it is fixed"
        ),
        (PitrState::Stale, _, _) => format!(
            "{summary}; point-in-time recovery is STALE (archive quiet past {WAL_STALE_AFTER_MINUTES}m or base backup older than {BASE_BACKUP_STALE_AFTER_DAYS}d)"
        ),
        (PitrState::Unverified, Some(secs), RpoBasis::WalArchive) => format!(
            "{summary}; WAL archiving puts the recovery point at about {} minutes, but no point-in-time recovery has ever been proven",
            secs / 60
        ),
        (PitrState::Unverified, _, _) => format!(
            "{summary}; WAL archiving is running but no point-in-time recovery has ever been proven"
        ),
        (PitrState::Current, Some(secs), _) => format!(
            "{summary}; point-in-time recovery is proven, with a recovery point of about {} minutes",
            secs / 60
        ),
        (PitrState::Current, None, _) => format!("{summary}; point-in-time recovery is proven"),
    };

    // The combination that reads best and protects least: dumps shipped
    // off-site, WAL left on the machine. Recovery then has a choice
    // between "off-site, hours old" and "minutes old, on the host that
    // just died" — which is not the choice anyone thinks they bought.
    let summary = if matches!(
        pitr,
        PitrState::Current | PitrState::Unverified | PitrState::Stale
    ) && matches!(offsite, OffsiteState::Current | OffsiteState::Stale)
        && !offsite_includes_pitr
    {
        format!(
            "{summary}; WARNING: the dumps go off-site but the WAL archive and base backups do NOT — point-in-time recovery would not survive losing this host"
        )
    } else {
        summary
    };

    BackupPosture {
        state,
        last_backup_at,
        last_verified_restore_at,
        last_attempt_failed,
        retention_days,
        encrypted,
        backup_count: successful_backups.len(),
        offsite_configured,
        last_offsite_sync_at,
        offsite,
        offsite_includes_pitr,
        pitr,
        last_wal_archive_at,
        last_base_backup_at,
        last_verified_pitr_at,
        rpo_estimate_seconds,
        rpo_basis,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-02T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn backup(at: &str, ok: bool) -> String {
        format!(
            r#"{{"event":"backup","at":"{at}","backup_id":"pg-x","sha256":"ab","size_bytes":10,"encrypted":true,"retention_days":30,"ok":{ok}}}"#
        )
    }

    fn restore(at: &str, ok: bool) -> String {
        format!(
            r#"{{"event":"restore_verified","at":"{at}","backup_id":"pg-x","tables":42,"migration_high_water":36,"ok":{ok}}}"#
        )
    }

    fn wal(at: &str, ok: bool) -> String {
        format!(r#"{{"event":"wal_archived","at":"{at}","segments":4,"ok":{ok}}}"#)
    }

    fn basebackup(at: &str, ok: bool) -> String {
        format!(
            r#"{{"event":"basebackup","at":"{at}","base_id":"base-x","sha256":"ab","size_bytes":99,"ok":{ok}}}"#
        )
    }

    fn pitr_drill(at: &str, ok: bool) -> String {
        format!(
            r#"{{"event":"pitr_verified","at":"{at}","base_id":"base-x","tables":42,"ok":{ok}}}"#
        )
    }

    fn offsite(at: &str, ok: bool) -> String {
        format!(
            r#"{{"event":"offsite_synced","at":"{at}","kind":"s3","scope":"dumps","files":1,"ok":{ok}}}"#
        )
    }

    fn offsite_with_pitr(at: &str, ok: bool) -> String {
        format!(
            r#"{{"event":"offsite_synced","at":"{at}","kind":"s3","scope":"dumps+pitr","files":9,"ok":{ok}}}"#
        )
    }

    #[test]
    fn a_missing_ledger_is_not_configured_not_healthy() {
        let p = posture_at(Path::new("/nonexistent/backups.jsonl"), now());
        assert_eq!(p.state, ProtectionState::NotConfigured);
        assert!(!p.state.is_protected());
        assert_eq!(p.last_backup_at, None);
        assert!(p.summary.contains("nothing is taking scheduled backups"));
    }

    #[test]
    fn current_backup_plus_recent_drill_is_verified() {
        let ledger = format!(
            "{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.state, ProtectionState::Verified);
        assert!(p.state.is_protected());
        assert_eq!(p.retention_days, Some(30));
        assert_eq!(p.encrypted, Some(true));
    }

    #[test]
    fn backups_without_any_restore_drill_are_unverified() {
        let p = evaluate(&backup("2026-10-02T02:00:00Z", true), now());
        assert_eq!(p.state, ProtectionState::Unverified);
        assert!(
            !p.state.is_protected(),
            "an untested backup is not protection"
        );
        assert!(p.summary.contains("no restore has EVER been verified"));
    }

    #[test]
    fn an_old_drill_expires() {
        let ledger = format!(
            "{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-01-01T03:00:00Z", true) // > 90 days
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.state, ProtectionState::Unverified);
        assert!(p.summary.contains("days ago"));
    }

    #[test]
    fn an_old_backup_is_stale() {
        let ledger = format!(
            "{}\n{}\n",
            backup("2026-09-20T02:00:00Z", true),
            restore("2026-09-21T03:00:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.state, ProtectionState::Stale);
        assert!(p.summary.contains("the schedule is not running"));
    }

    #[test]
    fn a_failed_last_attempt_outranks_an_older_success() {
        let ledger = format!(
            "{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            backup("2026-10-02T08:00:00Z", false)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.state, ProtectionState::Failing);
        assert!(!p.state.is_protected());
        // the older success is still reported — the operator needs both facts
        assert!(p.last_backup_at.is_some());
    }

    #[test]
    fn a_truncated_final_line_does_not_hide_the_history() {
        let ledger = format!(
            "{}\n{}\n{{\"event\":\"backup\",\"at\":\"2026-10-0",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.state, ProtectionState::Verified);
        assert_eq!(p.backup_count, 1);
    }

    #[test]
    fn unencrypted_backups_are_reported_as_such() {
        let ledger = r#"{"event":"backup","at":"2026-10-02T02:00:00Z","encrypted":false,"retention_days":7,"ok":true}"#;
        let p = evaluate(ledger, now());
        assert_eq!(p.encrypted, Some(false));
    }

    /// The default posture of this system: backups on the same host.
    /// `protected` may be true (the data IS restorable) but the summary
    /// must say out loud that losing the host loses both.
    #[test]
    fn local_only_backups_say_so_even_when_verified() {
        let ledger = format!(
            "{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.state, ProtectionState::Verified);
        assert_eq!(p.offsite, OffsiteState::NotConfigured);
        assert!(!p.offsite_configured);
        assert!(
            p.summary.contains("LOCAL ONLY"),
            "a verified-but-local posture must not read as disaster recovery: {}",
            p.summary
        );
    }

    #[test]
    fn a_current_offsite_copy_is_reported_as_current() {
        let ledger = format!(
            "{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            offsite("2026-10-02T02:05:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.offsite, OffsiteState::Current);
        assert!(p.offsite_configured);
        assert!(p.last_offsite_sync_at.is_some());
        assert!(p.summary.contains("off-site copy is current"));
    }

    /// Same invariant as the local axis: the newest attempt decides. A
    /// sync that succeeded yesterday does not excuse one failing today.
    #[test]
    fn a_failed_offsite_attempt_outranks_an_older_success() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            offsite("2026-10-01T02:05:00Z", true),
            offsite("2026-10-02T02:05:00Z", false)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.offsite, OffsiteState::Failing);
        // The local axis is untouched — the dumps themselves are fine.
        assert_eq!(p.state, ProtectionState::Verified);
        assert!(p.summary.contains("off-site copy is FAILING"));
    }

    #[test]
    fn an_offsite_copy_older_than_the_window_is_stale() {
        let ledger = format!(
            "{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            offsite("2026-09-20T02:05:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.offsite, OffsiteState::Stale);
        assert!(p.summary.contains("hours ago"));
    }

    /// The default shape of this system before batch 5: dumps only.
    /// `protected` can be true while a day of trades is at risk, so the
    /// summary must quantify it.
    #[test]
    fn dump_only_deployments_report_the_dump_interval_as_the_rpo() {
        let ledger = format!(
            "{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.pitr, PitrState::NotConfigured);
        assert_eq!(p.rpo_basis, RpoBasis::LastBackup);
        // 02:00 → 12:00 is ten hours of trades.
        assert_eq!(p.rpo_estimate_seconds, Some(10 * 3600));
        assert!(
            p.summary.contains("no WAL archiving"),
            "a dump-only posture must say what it costs: {}",
            p.summary
        );
    }

    #[test]
    fn wal_archiving_plus_a_base_backup_shortens_the_rpo() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-10-01T02:00:00Z", true),
            wal("2026-10-02T11:45:00Z", true),
            pitr_drill("2026-09-20T04:00:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.pitr, PitrState::Current);
        assert_eq!(p.rpo_basis, RpoBasis::WalArchive);
        // 11:45 → 12:00 is fifteen minutes, not ten hours.
        assert_eq!(p.rpo_estimate_seconds, Some(15 * 60));
        assert!(p.summary.contains("point-in-time recovery is proven"));
    }

    /// Archiving without a proven recovery is the same class of claim as
    /// a backup nobody has restored.
    #[test]
    fn wal_archiving_without_a_drill_is_unverified() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-10-01T02:00:00Z", true),
            wal("2026-10-02T11:45:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.pitr, PitrState::Unverified);
        assert!(p
            .summary
            .contains("no point-in-time recovery has ever been proven"));
    }

    /// The dangerous case: the optimistic number must NOT be quoted from
    /// an archive that is broken.
    #[test]
    fn a_failing_archive_never_supplies_the_rpo() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-10-01T02:00:00Z", true),
            wal("2026-10-02T11:00:00Z", true),
            wal("2026-10-02T11:50:00Z", false)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.pitr, PitrState::Failing);
        assert_eq!(p.rpo_basis, RpoBasis::LastBackup);
        assert_eq!(p.rpo_estimate_seconds, Some(10 * 3600));
        assert!(p.summary.contains("WAL archiving is FAILING"));
    }

    /// A quiet archive is stale even when every recorded attempt
    /// succeeded — silence is not success. (`archive_timeout` is what
    /// makes this signal trustworthy; see the constant's documentation.)
    #[test]
    fn a_quiet_archive_goes_stale() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-10-01T02:00:00Z", true),
            wal("2026-10-02T06:00:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.pitr, PitrState::Stale);
        assert_eq!(p.rpo_basis, RpoBasis::LastBackup);
        assert!(p.summary.contains("STALE"));
    }

    /// An old base backup is as disqualifying as a quiet archive: the
    /// WAL has to land on something.
    #[test]
    fn an_old_base_backup_makes_pitr_stale() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-09-01T02:00:00Z", true),
            wal("2026-10-02T11:45:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.pitr, PitrState::Stale);
    }

    /// The combination that reads best and protects least: dumps are
    /// off-site, the WAL archive is not.
    #[test]
    fn dumps_offsite_but_wal_local_is_called_out() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-10-01T02:00:00Z", true),
            wal("2026-10-02T11:45:00Z", true),
            pitr_drill("2026-09-20T04:00:00Z", true),
            offsite("2026-10-02T02:05:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert_eq!(p.offsite, OffsiteState::Current);
        assert_eq!(p.pitr, PitrState::Current);
        assert!(!p.offsite_includes_pitr);
        assert!(
            p.summary.contains("WAL archive and base backups do NOT"),
            "the gap must be named, not inferred: {}",
            p.summary
        );
    }

    #[test]
    fn a_full_scope_sync_reports_pitr_as_covered() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-10-01T02:00:00Z", true),
            wal("2026-10-02T11:45:00Z", true),
            pitr_drill("2026-09-20T04:00:00Z", true),
            offsite_with_pitr("2026-10-02T02:05:00Z", true)
        );
        let p = evaluate(&ledger, now());
        assert!(p.offsite_includes_pitr);
        assert!(!p.summary.contains("would not survive losing this host"));
    }

    /// A FAILED sync that intended to carry PITR proves nothing about
    /// what is actually off-site.
    #[test]
    fn a_failed_full_scope_sync_does_not_count_as_coverage() {
        let ledger = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true),
            basebackup("2026-10-01T02:00:00Z", true),
            wal("2026-10-02T11:45:00Z", true),
            offsite_with_pitr("2026-10-02T02:05:00Z", false)
        );
        let p = evaluate(&ledger, now());
        assert!(!p.offsite_includes_pitr);
        assert_eq!(p.offsite, OffsiteState::Failing);
    }

    #[test]
    fn the_tenant_body_never_carries_a_location_or_an_id() {
        let ledger = format!(
            "{}\n{}\n",
            backup("2026-10-02T02:00:00Z", true),
            restore("2026-09-15T03:00:00Z", true)
        );
        let body = evaluate(&ledger, now())
            .to_json("11111111-1111-1111-1111-111111111111")
            .to_string()
            .to_ascii_lowercase();
        for banned in [
            "s3://",
            "postgres://",
            "/app/data",
            "pg-x",
            "sha256",
            "backup_id",
        ] {
            assert!(
                !body.contains(banned),
                "tenant body leaked {banned}: {body}"
            );
        }
    }
}
