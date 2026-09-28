//! Singleton leases with fencing tokens (TASK 6 §2, §12).
//!
//! TASK 1–4 already own *per-execution* work through
//! [`crate::ownership::OwnershipRegistry`] (one claim per logical intent —
//! `snipe:<mint>`, `copy:<wallet>:<mint>`, `poly:entry:<token>`). TASK 6
//! adds the other half: **singleton role leases** for work that must have
//! exactly one active worker in the whole cluster regardless of how many
//! intents are in flight — the reconciliation worker, the recovery worker,
//! the accounting maintenance loop, each feed consumer.
//!
//! Model (identical semantics to the execution claims, different scope):
//!
//! * `acquire` is atomic: a free, expired or released lease is taken; a
//!   live lease held by someone else is a deterministic
//!   [`LeaseDecision::Rejected`] — never a second owner.
//! * every acquisition increments the **fencing token** (`generation`).
//!   A mutation guarded by an older token is rejected
//!   ([`FenceError::Fenced`]) — a stale worker cannot keep writing.
//! * `renew` is a compare-and-set on `(holder, generation)`: `false` means
//!   ownership was lost (expired, taken over, released).
//! * `release` is a compare-and-set too, so a fenced worker cannot release
//!   the new owner's lease.
//!
//! Store failures are NEVER "assume I own it": every fallible path fails
//! closed (the caller stops touching the guarded state).

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};

/// Singleton roles that must have exactly one active worker.
///
/// A closed vocabulary keeps the metric label bounded and makes the set of
/// cluster singletons reviewable. `Feed` carries the feed name (also a
/// closed set — see [`crate::ha::cursor::FeedId`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "role", content = "key")]
pub enum LeaseRole {
    /// The venue/chain reconciliation worker (`RecoveryWorker`).
    Reconciliation,
    /// The startup/periodic recovery + maintenance worker.
    Recovery,
    /// The TASK 5 accounting maintenance loop (flush, reconcile, gauges).
    AccountingMaintenance,
    /// The cross-replica position-book / runtime-flag sync loop.
    StateSync,
    /// One feed consumer.
    Feed(String),
}

impl LeaseRole {
    /// Stable textual key (`reconciliation`, `feed:copy_logs`, …). This is
    /// the durable primary key of the lease row.
    pub fn as_string(&self) -> String {
        match self {
            LeaseRole::Reconciliation => "reconciliation".into(),
            LeaseRole::Recovery => "recovery".into(),
            LeaseRole::AccountingMaintenance => "accounting_maintenance".into(),
            LeaseRole::StateSync => "state_sync".into(),
            LeaseRole::Feed(name) => format!("feed:{name}"),
        }
    }

    /// Inverse of [`LeaseRole::as_string`].
    pub fn parse(s: &str) -> Option<LeaseRole> {
        let s = s.trim();
        Some(match s {
            "reconciliation" => LeaseRole::Reconciliation,
            "recovery" => LeaseRole::Recovery,
            "accounting_maintenance" => LeaseRole::AccountingMaintenance,
            "state_sync" => LeaseRole::StateSync,
            other => {
                let name = other.strip_prefix("feed:")?.trim();
                if name.is_empty() {
                    return None;
                }
                LeaseRole::Feed(name.to_string())
            }
        })
    }

    /// Low-cardinality class for metrics (`feed` collapses the name).
    pub fn kind(&self) -> &'static str {
        match self {
            LeaseRole::Reconciliation => "reconciliation",
            LeaseRole::Recovery => "recovery",
            LeaseRole::AccountingMaintenance => "accounting_maintenance",
            LeaseRole::StateSync => "state_sync",
            LeaseRole::Feed(_) => "feed",
        }
    }
}

impl std::fmt::Display for LeaseRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_string())
    }
}

/// One durable lease record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    /// The singleton role.
    pub role: LeaseRole,
    /// Worker that holds it.
    pub holder: String,
    /// Fencing token: strictly increasing per role, bumped on every
    /// acquisition (including takeovers and re-acquisitions).
    pub generation: i64,
    /// When the current holder acquired it.
    pub acquired_at: DateTime<Utc>,
    /// Lease deadline; after this another worker may take over.
    pub expires_at: DateTime<Utc>,
    /// Last successful renewal.
    pub renewed_at: DateTime<Utc>,
    /// How many times this role changed hands.
    pub takeover_count: i64,
    /// Previous holder, when this record is the result of a takeover.
    pub previous_holder: Option<String>,
    /// `true` once the holder released it cleanly (immediately re-acquirable).
    pub released: bool,
}

impl Lease {
    /// Is the lease still live at `now`?
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        !self.released && self.expires_at > now
    }

    /// Seconds until expiry (negative once expired).
    pub fn ttl_secs(&self, now: DateTime<Utc>) -> i64 {
        self.expires_at.signed_duration_since(now).num_seconds()
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "role={} holder={} generation={} expires={} takeovers={} previous={} released={}",
            self.role,
            self.holder,
            self.generation,
            self.expires_at.to_rfc3339(),
            self.takeover_count,
            self.previous_holder.as_deref().unwrap_or("-"),
            self.released
        )
    }
}

/// Result of an acquisition attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum LeaseDecision {
    /// This worker is now the sole holder.
    Acquired(Lease),
    /// Someone else holds a live lease — deterministic loser outcome.
    Rejected {
        /// Current holder.
        holder: String,
        /// Its fencing token.
        generation: i64,
        /// When it expires (the earliest a takeover can happen).
        expires_at: DateTime<Utc>,
    },
}

impl LeaseDecision {
    /// The lease on success.
    pub fn acquired(&self) -> Option<&Lease> {
        match self {
            LeaseDecision::Acquired(l) => Some(l),
            LeaseDecision::Rejected { .. } => None,
        }
    }

    /// True when this worker won.
    pub fn is_acquired(&self) -> bool {
        matches!(self, LeaseDecision::Acquired(_))
    }
}

/// Why a fenced mutation was refused (TASK 6 §12). Every variant is a
/// deterministic failure — the caller must stop mutating the guarded state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceError {
    /// The lease is held by someone else, or by a newer generation of this
    /// worker: this holder is stale.
    Fenced {
        /// Role that was checked.
        role: String,
        /// Generation the caller presented.
        presented: i64,
        /// Generation (and holder) the store actually has, when known.
        actual: Option<(String, i64)>,
    },
    /// The lease expired and nobody re-acquired it.
    Expired {
        /// Role that was checked.
        role: String,
        /// Generation the caller presented.
        presented: i64,
    },
    /// The durable store could not answer — fail closed, never assume
    /// ownership.
    StoreUnavailable {
        /// Role that was checked.
        role: String,
        /// Store error text.
        detail: String,
    },
}

impl FenceError {
    /// Stable reason label (metrics, audit).
    pub fn reason(&self) -> &'static str {
        match self {
            FenceError::Fenced { .. } => "fenced",
            FenceError::Expired { .. } => "expired",
            FenceError::StoreUnavailable { .. } => "store_unavailable",
        }
    }

    /// The role the failure is about.
    pub fn role(&self) -> &str {
        match self {
            FenceError::Fenced { role, .. }
            | FenceError::Expired { role, .. }
            | FenceError::StoreUnavailable { role, .. } => role,
        }
    }
}

impl std::fmt::Display for FenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FenceError::Fenced {
                role,
                presented,
                actual,
            } => match actual {
                Some((holder, gen)) => write!(
                    f,
                    "fenced: lease {role} is held by {holder} at generation {gen}, this worker presented {presented}"
                ),
                None => write!(
                    f,
                    "fenced: lease {role} no longer accepts generation {presented}"
                ),
            },
            FenceError::Expired { role, presented } => write!(
                f,
                "lease {role} expired (generation {presented}); ownership must be re-acquired"
            ),
            FenceError::StoreUnavailable { role, detail } => write!(
                f,
                "lease {role} could not be verified ({detail}); refusing the mutation"
            ),
        }
    }
}

impl std::error::Error for FenceError {}

/// Parameters of one acquisition.
#[derive(Debug, Clone)]
pub struct LeaseRequest {
    /// The singleton role.
    pub role: LeaseRole,
    /// Worker asking for it.
    pub holder: String,
    /// How long the lease is granted for.
    pub ttl: ChronoDuration,
}

/// Compute the next generation for a role. Fencing tokens are strictly
/// increasing per role and never reused, so a delayed write from an old
/// holder can always be recognised.
pub fn next_generation(previous: Option<i64>) -> i64 {
    previous.unwrap_or(0).saturating_add(1)
}

/// The renewal interval a holder should use for `ttl`: a third of the
/// lease, floored at one second, so two renewals may fail before the lease
/// is at risk.
pub fn renew_interval(ttl: ChronoDuration) -> ChronoDuration {
    let secs = (ttl.num_seconds() / 3).max(1);
    ChronoDuration::seconds(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(now: DateTime<Utc>, ttl: i64) -> Lease {
        Lease {
            role: LeaseRole::Reconciliation,
            holder: "w1".into(),
            generation: 1,
            acquired_at: now,
            expires_at: now + ChronoDuration::seconds(ttl),
            renewed_at: now,
            takeover_count: 0,
            previous_holder: None,
            released: false,
        }
    }

    #[test]
    fn role_keys_round_trip() {
        for r in [
            LeaseRole::Reconciliation,
            LeaseRole::Recovery,
            LeaseRole::AccountingMaintenance,
            LeaseRole::StateSync,
            LeaseRole::Feed("copy_logs".into()),
        ] {
            assert_eq!(LeaseRole::parse(&r.as_string()), Some(r.clone()), "{r}");
        }
        assert_eq!(LeaseRole::parse("feed:"), None);
        assert_eq!(LeaseRole::parse("nonsense"), None);
        assert_eq!(LeaseRole::Feed("x".into()).kind(), "feed");
        assert_eq!(LeaseRole::Recovery.kind(), "recovery");
    }

    #[test]
    fn liveness_is_a_pure_function_of_the_clock() {
        let t0 = Utc::now();
        let l = lease(t0, 30);
        assert!(l.is_live(t0 + ChronoDuration::seconds(29)));
        assert!(!l.is_live(t0 + ChronoDuration::seconds(31)));
        assert_eq!(l.ttl_secs(t0 + ChronoDuration::seconds(10)), 20);
        let mut released = lease(t0, 30);
        released.released = true;
        assert!(!released.is_live(t0), "a released lease is never live");
    }

    #[test]
    fn generations_are_strictly_increasing() {
        assert_eq!(next_generation(None), 1);
        assert_eq!(next_generation(Some(1)), 2);
        assert_eq!(next_generation(Some(i64::MAX)), i64::MAX);
    }

    #[test]
    fn renew_interval_leaves_room_for_two_failures() {
        assert_eq!(
            renew_interval(ChronoDuration::seconds(30)).num_seconds(),
            10
        );
        assert_eq!(renew_interval(ChronoDuration::seconds(2)).num_seconds(), 1);
        assert_eq!(renew_interval(ChronoDuration::seconds(0)).num_seconds(), 1);
    }

    #[test]
    fn fence_errors_carry_a_deterministic_reason() {
        let e = FenceError::Fenced {
            role: "recovery".into(),
            presented: 1,
            actual: Some(("w2".into(), 2)),
        };
        assert_eq!(e.reason(), "fenced");
        assert_eq!(e.role(), "recovery");
        assert!(e.to_string().contains("w2"));
        let e = FenceError::Expired {
            role: "feed:copy_logs".into(),
            presented: 3,
        };
        assert_eq!(e.reason(), "expired");
        assert!(e.to_string().contains("re-acquired"));
        let e = FenceError::StoreUnavailable {
            role: "recovery".into(),
            detail: "connection refused".into(),
        };
        assert_eq!(e.reason(), "store_unavailable");
        assert!(e.to_string().contains("refusing"));
    }

    #[test]
    fn decisions_name_the_winner() {
        let t0 = Utc::now();
        let d = LeaseDecision::Acquired(lease(t0, 30));
        assert!(d.is_acquired());
        assert_eq!(d.acquired().unwrap().holder, "w1");
        let d = LeaseDecision::Rejected {
            holder: "w2".into(),
            generation: 7,
            expires_at: t0,
        };
        assert!(!d.is_acquired());
        assert!(d.acquired().is_none());
    }
}
