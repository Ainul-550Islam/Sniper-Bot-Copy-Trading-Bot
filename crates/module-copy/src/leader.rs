//! Leader registry and lifecycle (TASK 3 §01).
//!
//! A *leader* is a wallet the engine follows. `[copy].wallets` is the seed
//! and the source of truth for **membership and rules**; the registry adds a
//! lifecycle on top and keeps the running counters the dashboard, the
//! reconciler and the durable `copy_leaders` table need.
//!
//! ```text
//!            follow                pause               unfollow
//!  (none) ───────────► ACTIVE ◄──────────► PAUSED ───────────► REMOVED
//!                        │      resume        │                   ▲
//!                        └────────────────────┴───────────────────┘
//!                                     unfollow
//!  REMOVED ── follow (re-follow) ──► ACTIVE
//! ```
//!
//! * `ACTIVE`  — events are mirrored (subject to policy / risk).
//! * `PAUSED`  — events are observed, counted and reconciled; nothing new is
//!   mirrored; existing mirrored positions keep their exits.
//! * `REMOVED` — the wallet was dropped from config; its row is kept for the
//!   audit trail and reconciliation of positions that are still open.
//!
//! Config hot reloads flow through [`LeaderRegistry::sync_from_config`]: new
//! wallets are followed, missing ones unfollowed, `paused` toggles pause /
//! resume, and any other rule change is recorded as `rule_changed`. Every
//! transition is returned to the caller so it can be journaled, audited and
//! metered — the registry itself is pure data (no I/O, no clock beyond the
//! timestamps it stores).

use std::collections::HashMap;

use bot_core::config::CopyWallet;
use bot_core::db::copy::{LeaderEventRecord, LeaderRecord};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle state of a leader.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderStatus {
    /// Events are mirrored.
    Active,
    /// Observed but not mirrored.
    Paused,
    /// No longer followed.
    Removed,
}

impl LeaderStatus {
    /// Stable label (also the `copy_leaders.status` value).
    pub fn as_str(&self) -> &'static str {
        match self {
            LeaderStatus::Active => "active",
            LeaderStatus::Paused => "paused",
            LeaderStatus::Removed => "removed",
        }
    }

    /// Inverse of [`LeaderStatus::as_str`].
    pub fn parse(s: &str) -> Option<LeaderStatus> {
        Some(match s {
            "active" => LeaderStatus::Active,
            "paused" => LeaderStatus::Paused,
            "removed" => LeaderStatus::Removed,
            _ => return None,
        })
    }
}

impl std::fmt::Display for LeaderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A lifecycle transition kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderEvent {
    /// Added to the registry (or re-followed after removal).
    Followed,
    /// `ACTIVE → PAUSED`.
    Paused,
    /// `PAUSED → ACTIVE`.
    Resumed,
    /// `ACTIVE | PAUSED → REMOVED`.
    Unfollowed,
    /// The sizing / policy rule changed while followed.
    RuleChanged,
}

impl LeaderEvent {
    /// Stable label (also the `copy_leader_events.event` value).
    pub fn as_str(&self) -> &'static str {
        match self {
            LeaderEvent::Followed => "followed",
            LeaderEvent::Paused => "paused",
            LeaderEvent::Resumed => "resumed",
            LeaderEvent::Unfollowed => "unfollowed",
            LeaderEvent::RuleChanged => "rule_changed",
        }
    }
}

impl std::fmt::Display for LeaderEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a transition was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderError {
    /// No leader with that address.
    Unknown(String),
    /// The transition is not allowed from the current state.
    InvalidTransition {
        /// Leader address.
        address: String,
        /// Current state.
        from: LeaderStatus,
        /// Requested transition.
        event: LeaderEvent,
    },
    /// The rule has an empty address.
    EmptyAddress,
}

impl std::fmt::Display for LeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LeaderError::Unknown(a) => write!(f, "unknown leader {a}"),
            LeaderError::InvalidTransition {
                address,
                from,
                event,
            } => write!(f, "leader {address}: cannot {event} from {from}"),
            LeaderError::EmptyAddress => f.write_str("leader address must not be empty"),
        }
    }
}

impl std::error::Error for LeaderError {}

/// Running counters for one leader.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LeaderStats {
    /// Events observed (any outcome).
    pub events_seen: u64,
    /// Events that opened (or may have opened) a mirrored position.
    pub mirrored: u64,
    /// Events refused or failed.
    pub rejected: u64,
    /// Observation time of the newest event.
    pub last_event_at: Option<DateTime<Utc>>,
    /// Highest slot observed.
    pub last_slot: Option<u64>,
    /// Reason label of the most recent rejection.
    pub last_rejection: Option<String>,
}

/// One followed wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Leader {
    /// Wallet address (base58).
    pub address: String,
    /// Operator label (falls back to a short address).
    pub label: String,
    /// Lifecycle state.
    pub status: LeaderStatus,
    /// Sizing / policy rule from config.
    pub rule: CopyWallet,
    /// Where the leader came from (`config`).
    pub source: String,
    /// First time the leader was followed.
    pub followed_at: DateTime<Utc>,
    /// When the current status was entered.
    pub status_since: DateTime<Utc>,
    /// Running counters.
    pub stats: LeaderStats,
}

impl Leader {
    /// Whether new entries may be mirrored from this leader.
    pub fn is_active(&self) -> bool {
        self.status == LeaderStatus::Active
    }

    /// Durable row for `copy_leaders`.
    pub fn record(&self) -> LeaderRecord {
        LeaderRecord {
            address: self.address.clone(),
            label: self.label.clone(),
            status: self.status.as_str().to_string(),
            source: self.source.clone(),
            followed_at: self.followed_at,
            status_since: self.status_since,
            events_seen: self.stats.events_seen.min(i64::MAX as u64) as i64,
            mirrored: self.stats.mirrored.min(i64::MAX as u64) as i64,
            rejected: self.stats.rejected.min(i64::MAX as u64) as i64,
            last_event_at: self.stats.last_event_at,
            last_slot: self.stats.last_slot.map(|s| s.min(i64::MAX as u64) as i64),
            updated_at: Utc::now(),
        }
    }
}

/// A transition the registry performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderTransition {
    /// Leader address.
    pub address: String,
    /// What happened.
    pub event: LeaderEvent,
    /// Free-text reason (`config reload`, operator note…).
    pub reason: Option<String>,
    /// When.
    pub at: DateTime<Utc>,
}

impl LeaderTransition {
    /// Durable row for `copy_leader_events`.
    pub fn record(&self, replica_id: &str) -> LeaderEventRecord {
        LeaderEventRecord {
            id: 0,
            address: self.address.clone(),
            event: self.event.as_str().to_string(),
            reason: self.reason.clone(),
            replica_id: replica_id.to_string(),
            ts: self.at,
        }
    }
}

/// Serializable view of a leader for the API / Telegram.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaderSnapshot {
    /// Wallet address.
    pub address: String,
    /// Operator label.
    pub label: String,
    /// Lifecycle state.
    pub status: LeaderStatus,
    /// Sizing summary: `fixed 0.05 SOL` or `5.0% (max 0.25 SOL)`.
    pub sizing: String,
    /// `buys_only` flag.
    pub buys_only: bool,
    /// Running counters.
    pub stats: LeaderStats,
    /// First followed.
    pub followed_at: DateTime<Utc>,
    /// Status entered.
    pub status_since: DateTime<Utc>,
}

/// Bound on the in-memory transition history.
pub const TRANSITION_HISTORY: usize = 256;

/// The set of followed leaders.
#[derive(Debug, Default)]
pub struct LeaderRegistry {
    leaders: HashMap<String, Leader>,
    /// Insertion order (stable listing).
    order: Vec<String>,
    history: std::collections::VecDeque<LeaderTransition>,
}

impl LeaderRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry seeded from config: every wallet is followed; `paused`
    /// wallets start paused. Transitions are recorded in the history.
    pub fn from_config(wallets: &[CopyWallet]) -> Self {
        let mut reg = Self::new();
        reg.sync_from_config(wallets);
        reg
    }

    /// Number of leaders in any state.
    pub fn len(&self) -> usize {
        self.leaders.len()
    }

    /// Whether the registry has no leaders.
    pub fn is_empty(&self) -> bool {
        self.leaders.is_empty()
    }

    /// Look up a leader.
    pub fn get(&self, address: &str) -> Option<&Leader> {
        self.leaders.get(address.trim())
    }

    /// The rule and status of a leader (any state).
    pub fn rule_for(&self, address: &str) -> Option<(&CopyWallet, LeaderStatus)> {
        self.get(address).map(|l| (&l.rule, l.status))
    }

    /// Leaders in insertion order.
    pub fn all(&self) -> Vec<&Leader> {
        self.order
            .iter()
            .filter_map(|a| self.leaders.get(a))
            .collect()
    }

    /// Leaders that may be mirrored.
    pub fn active(&self) -> Vec<&Leader> {
        self.all().into_iter().filter(|l| l.is_active()).collect()
    }

    /// `(active, paused, removed)` counts.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for l in self.leaders.values() {
            match l.status {
                LeaderStatus::Active => c.0 += 1,
                LeaderStatus::Paused => c.1 += 1,
                LeaderStatus::Removed => c.2 += 1,
            }
        }
        c
    }

    /// Most recent transitions, oldest first (bounded by [`TRANSITION_HISTORY`]).
    pub fn history(&self) -> Vec<LeaderTransition> {
        self.history.iter().cloned().collect()
    }

    /// Serializable views in insertion order.
    pub fn snapshot(&self) -> Vec<LeaderSnapshot> {
        self.all()
            .into_iter()
            .map(|l| LeaderSnapshot {
                address: l.address.clone(),
                label: l.label.clone(),
                status: l.status,
                sizing: describe_sizing(&l.rule),
                buys_only: l.rule.buys_only,
                stats: l.stats.clone(),
                followed_at: l.followed_at,
                status_since: l.status_since,
            })
            .collect()
    }

    // ------------------------------------------------------- transitions --

    /// Follow a wallet. A new address is added `ACTIVE` (or `PAUSED` when
    /// the rule says so); a `REMOVED` leader is re-followed; an already
    /// followed leader whose rule differs gets `rule_changed`, an identical
    /// rule is a no-op (`None`).
    pub fn follow(
        &mut self,
        rule: CopyWallet,
        source: &str,
        reason: Option<&str>,
    ) -> Result<Option<LeaderTransition>, LeaderError> {
        let address = rule.address.trim().to_string();
        if address.is_empty() {
            return Err(LeaderError::EmptyAddress);
        }
        let now = Utc::now();
        let label = label_for(&rule);
        if let Some(existing) = self.leaders.get_mut(&address) {
            if existing.status == LeaderStatus::Removed {
                existing.rule = rule.clone();
                existing.label = label;
                existing.status = if rule.paused {
                    LeaderStatus::Paused
                } else {
                    LeaderStatus::Active
                };
                existing.status_since = now;
                existing.source = source.to_string();
                return Ok(Some(self.push(address, LeaderEvent::Followed, reason, now)));
            }
            if !same_rule(&existing.rule, &rule) {
                existing.rule = rule;
                existing.label = label;
                return Ok(Some(self.push(
                    address,
                    LeaderEvent::RuleChanged,
                    reason,
                    now,
                )));
            }
            return Ok(None);
        }
        let status = if rule.paused {
            LeaderStatus::Paused
        } else {
            LeaderStatus::Active
        };
        self.leaders.insert(
            address.clone(),
            Leader {
                address: address.clone(),
                label,
                status,
                rule,
                source: source.to_string(),
                followed_at: now,
                status_since: now,
                stats: LeaderStats::default(),
            },
        );
        self.order.push(address.clone());
        Ok(Some(self.push(address, LeaderEvent::Followed, reason, now)))
    }

    /// `ACTIVE → PAUSED`.
    pub fn pause(
        &mut self,
        address: &str,
        reason: Option<&str>,
    ) -> Result<LeaderTransition, LeaderError> {
        self.transition(address, LeaderEvent::Paused, LeaderStatus::Paused, reason)
    }

    /// `PAUSED → ACTIVE`.
    pub fn resume(
        &mut self,
        address: &str,
        reason: Option<&str>,
    ) -> Result<LeaderTransition, LeaderError> {
        self.transition(address, LeaderEvent::Resumed, LeaderStatus::Active, reason)
    }

    /// `ACTIVE | PAUSED → REMOVED`.
    pub fn unfollow(
        &mut self,
        address: &str,
        reason: Option<&str>,
    ) -> Result<LeaderTransition, LeaderError> {
        self.transition(
            address,
            LeaderEvent::Unfollowed,
            LeaderStatus::Removed,
            reason,
        )
    }

    fn transition(
        &mut self,
        address: &str,
        event: LeaderEvent,
        to: LeaderStatus,
        reason: Option<&str>,
    ) -> Result<LeaderTransition, LeaderError> {
        let address = address.trim().to_string();
        let leader = self
            .leaders
            .get_mut(&address)
            .ok_or_else(|| LeaderError::Unknown(address.clone()))?;
        let allowed = matches!(
            (leader.status, event),
            (LeaderStatus::Active, LeaderEvent::Paused)
                | (LeaderStatus::Paused, LeaderEvent::Resumed)
                | (LeaderStatus::Active, LeaderEvent::Unfollowed)
                | (LeaderStatus::Paused, LeaderEvent::Unfollowed)
        );
        if !allowed {
            return Err(LeaderError::InvalidTransition {
                address,
                from: leader.status,
                event,
            });
        }
        let now = Utc::now();
        leader.status = to;
        leader.status_since = now;
        if event == LeaderEvent::Paused {
            leader.rule.paused = true;
        } else if event == LeaderEvent::Resumed {
            leader.rule.paused = false;
        }
        Ok(self.push(address, event, reason, now))
    }

    fn push(
        &mut self,
        address: String,
        event: LeaderEvent,
        reason: Option<&str>,
        at: DateTime<Utc>,
    ) -> LeaderTransition {
        let t = LeaderTransition {
            address,
            event,
            reason: reason.map(|r| r.to_string()),
            at,
        };
        self.history.push_back(t.clone());
        while self.history.len() > TRANSITION_HISTORY {
            self.history.pop_front();
        }
        t
    }

    /// Reconcile the registry with a (possibly hot-reloaded) config. Returns
    /// every transition performed, in order: follows / rule changes /
    /// pause-resume toggles for configured wallets, then unfollows for
    /// leaders that disappeared from config.
    pub fn sync_from_config(&mut self, wallets: &[CopyWallet]) -> Vec<LeaderTransition> {
        let mut out = Vec::new();
        let mut configured: Vec<String> = Vec::with_capacity(wallets.len());
        for rule in wallets {
            let address = rule.address.trim().to_string();
            if address.is_empty() {
                continue;
            }
            configured.push(address.clone());
            let wanted_paused = rule.paused;
            match self.follow(rule.clone(), "config", Some("config sync")) {
                Ok(Some(t)) => out.push(t),
                Ok(None) => {}
                Err(_) => continue,
            }
            let current = self.leaders.get(&address).map(|l| l.status);
            match (current, wanted_paused) {
                (Some(LeaderStatus::Active), true) => {
                    if let Ok(t) = self.pause(&address, Some("config paused = true")) {
                        out.push(t);
                    }
                }
                (Some(LeaderStatus::Paused), false) => {
                    if let Ok(t) = self.resume(&address, Some("config paused = false")) {
                        out.push(t);
                    }
                }
                _ => {}
            }
        }
        let gone: Vec<String> = self
            .order
            .iter()
            .filter(|a| !configured.contains(a))
            .filter(|a| {
                self.leaders
                    .get(*a)
                    .map(|l| l.status != LeaderStatus::Removed)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        for address in gone {
            if let Ok(t) = self.unfollow(&address, Some("removed from config")) {
                out.push(t);
            }
        }
        out
    }

    /// Restore durable counters and first-followed times from `copy_leaders`
    /// rows. Membership, rules and the paused flag stay config-driven (the
    /// row of a leader that is no longer configured is ignored); counters
    /// take the larger of the two views so a restart never loses history.
    pub fn restore(&mut self, records: &[LeaderRecord]) -> usize {
        let mut applied = 0;
        for rec in records {
            let Some(leader) = self.leaders.get_mut(rec.address.trim()) else {
                continue;
            };
            if rec.followed_at < leader.followed_at {
                leader.followed_at = rec.followed_at;
            }
            leader.stats.events_seen = leader.stats.events_seen.max(rec.events_seen.max(0) as u64);
            leader.stats.mirrored = leader.stats.mirrored.max(rec.mirrored.max(0) as u64);
            leader.stats.rejected = leader.stats.rejected.max(rec.rejected.max(0) as u64);
            if let Some(at) = rec.last_event_at {
                if leader.stats.last_event_at.map(|c| at > c).unwrap_or(true) {
                    leader.stats.last_event_at = Some(at);
                }
            }
            if let Some(slot) = rec.last_slot {
                let slot = slot.max(0) as u64;
                if leader.stats.last_slot.map(|c| slot > c).unwrap_or(true) {
                    leader.stats.last_slot = Some(slot);
                }
            }
            applied += 1;
        }
        applied
    }

    // ----------------------------------------------------------- counters --

    /// Count an observed event.
    pub fn note_event(&mut self, address: &str, slot: u64, observed_at: DateTime<Utc>) {
        if let Some(l) = self.leaders.get_mut(address.trim()) {
            l.stats.events_seen = l.stats.events_seen.saturating_add(1);
            if l.stats
                .last_event_at
                .map(|c| observed_at > c)
                .unwrap_or(true)
            {
                l.stats.last_event_at = Some(observed_at);
            }
            if slot > 0 && l.stats.last_slot.map(|c| slot > c).unwrap_or(true) {
                l.stats.last_slot = Some(slot);
            }
        }
    }

    /// Count a mirrored (or ambiguous) entry.
    pub fn note_mirrored(&mut self, address: &str) {
        if let Some(l) = self.leaders.get_mut(address.trim()) {
            l.stats.mirrored = l.stats.mirrored.saturating_add(1);
        }
    }

    /// Count a rejection / failure.
    pub fn note_rejected(&mut self, address: &str, reason: &str) {
        if let Some(l) = self.leaders.get_mut(address.trim()) {
            l.stats.rejected = l.stats.rejected.saturating_add(1);
            l.stats.last_rejection = Some(reason.to_string());
        }
    }
}

/// Display label: the configured label, else the first 8 characters.
pub fn label_for(rule: &CopyWallet) -> String {
    rule.label
        .clone()
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| rule.address.chars().take(8).collect())
}

/// `fixed 0.0500 SOL` or `5.0% of theirs (max 0.2500 SOL)`.
pub fn describe_sizing(rule: &CopyWallet) -> String {
    match rule.fixed_sol {
        Some(f) if f > 0.0 => format!("fixed {f:.4} SOL"),
        _ => {
            if rule.max_sol > 0.0 {
                format!(
                    "{:.1}% of theirs (max {:.4} SOL)",
                    rule.fraction_of_their_size * 100.0,
                    rule.max_sol
                )
            } else {
                format!("{:.1}% of theirs", rule.fraction_of_their_size * 100.0)
            }
        }
    }
}

/// Field-wise rule comparison (the `paused` flag is lifecycle, not rule).
pub fn same_rule(a: &CopyWallet, b: &CopyWallet) -> bool {
    a.address.trim() == b.address.trim()
        && a.label == b.label
        && a.fixed_sol == b.fixed_sol
        && a.fraction_of_their_size == b.fraction_of_their_size
        && a.max_sol == b.max_sol
        && a.min_sol == b.min_sol
        && a.buys_only == b.buys_only
        && a.slippage_pct == b.slippage_pct
        && a.max_staleness_secs == b.max_staleness_secs
        && a.max_exposure_sol == b.max_exposure_sol
        && a.max_open_positions == b.max_open_positions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(addr: &str) -> CopyWallet {
        CopyWallet {
            address: addr.into(),
            label: Some(format!("L-{addr}")),
            ..CopyWallet::default()
        }
    }

    #[test]
    fn follow_pause_resume_unfollow_refollow() {
        let mut reg = LeaderRegistry::new();
        let t = reg.follow(rule("A"), "config", None).unwrap().unwrap();
        assert_eq!(t.event, LeaderEvent::Followed);
        assert_eq!(reg.get("A").unwrap().status, LeaderStatus::Active);
        assert!(reg.follow(rule("A"), "config", None).unwrap().is_none());
        assert_eq!(
            reg.pause("A", Some("op")).unwrap().event,
            LeaderEvent::Paused
        );
        assert!(reg.get("A").unwrap().rule.paused);
        assert!(matches!(
            reg.pause("A", None),
            Err(LeaderError::InvalidTransition { .. })
        ));
        assert_eq!(reg.resume("A", None).unwrap().event, LeaderEvent::Resumed);
        assert!(!reg.get("A").unwrap().rule.paused);
        assert!(matches!(
            reg.resume("A", None),
            Err(LeaderError::InvalidTransition { .. })
        ));
        assert_eq!(
            reg.unfollow("A", None).unwrap().event,
            LeaderEvent::Unfollowed
        );
        assert_eq!(reg.get("A").unwrap().status, LeaderStatus::Removed);
        assert!(matches!(
            reg.pause("A", None),
            Err(LeaderError::InvalidTransition { .. })
        ));
        assert!(matches!(
            reg.unfollow("Z", None),
            Err(LeaderError::Unknown(_))
        ));
        let t = reg.follow(rule("A"), "config", None).unwrap().unwrap();
        assert_eq!(t.event, LeaderEvent::Followed, "re-follow after removal");
        assert_eq!(reg.get("A").unwrap().status, LeaderStatus::Active);
        assert_eq!(reg.history().len(), 5);
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn empty_address_is_refused() {
        let mut reg = LeaderRegistry::new();
        assert_eq!(
            reg.follow(rule("  "), "config", None),
            Err(LeaderError::EmptyAddress)
        );
        assert!(reg.is_empty());
    }

    #[test]
    fn config_sync_adds_updates_pauses_and_removes() {
        let mut reg = LeaderRegistry::from_config(&[rule("A"), rule("B")]);
        assert_eq!(reg.counts(), (2, 0, 0));
        let mut b = rule("B");
        b.max_sol = 9.0;
        b.paused = true;
        let mut c = rule("C");
        c.paused = true;
        let ts = reg.sync_from_config(&[b, c]);
        let kinds: Vec<(String, LeaderEvent)> =
            ts.iter().map(|t| (t.address.clone(), t.event)).collect();
        assert!(kinds.contains(&("B".into(), LeaderEvent::RuleChanged)));
        assert!(kinds.contains(&("B".into(), LeaderEvent::Paused)));
        assert!(kinds.contains(&("C".into(), LeaderEvent::Followed)));
        assert!(kinds.contains(&("A".into(), LeaderEvent::Unfollowed)));
        assert_eq!(reg.get("C").unwrap().status, LeaderStatus::Paused);
        assert_eq!(reg.counts(), (0, 2, 1));
        let mut b2 = rule("B");
        b2.max_sol = 9.0;
        let ts = reg.sync_from_config(&[b2.clone(), rule("C")]);
        assert!(ts
            .iter()
            .any(|t| t.address == "B" && t.event == LeaderEvent::Resumed));
        assert!(ts
            .iter()
            .any(|t| t.address == "C" && t.event == LeaderEvent::Resumed));
        assert!(
            reg.sync_from_config(&[b2, rule("C")]).is_empty(),
            "idempotent"
        );
        assert_eq!(reg.active().len(), 2);
    }

    #[test]
    fn counters_and_restore_take_the_larger_view() {
        let mut reg = LeaderRegistry::from_config(&[rule("A")]);
        let t0 = Utc::now();
        reg.note_event("A", 10, t0);
        reg.note_event("A", 8, t0 - chrono::Duration::seconds(1));
        reg.note_mirrored("A");
        reg.note_rejected("A", "STALE_EVENT");
        let s = &reg.get("A").unwrap().stats;
        assert_eq!(s.events_seen, 2);
        assert_eq!(s.last_slot, Some(10));
        assert_eq!(s.last_event_at, Some(t0));
        assert_eq!(s.mirrored, 1);
        assert_eq!(s.rejected, 1);
        assert_eq!(s.last_rejection.as_deref(), Some("STALE_EVENT"));

        let mut rec = reg.get("A").unwrap().record();
        rec.events_seen = 50;
        rec.mirrored = 0;
        rec.last_slot = Some(7);
        rec.followed_at = t0 - chrono::Duration::days(3);
        let ghost = LeaderRecord {
            address: "GHOST".into(),
            ..rec.clone()
        };
        assert_eq!(
            reg.restore(&[rec, ghost]),
            1,
            "unconfigured rows are ignored"
        );
        let l = reg.get("A").unwrap();
        assert_eq!(l.stats.events_seen, 50);
        assert_eq!(l.stats.mirrored, 1);
        assert_eq!(l.stats.last_slot, Some(10));
        assert_eq!(l.followed_at, t0 - chrono::Duration::days(3));
        assert_eq!(l.record().status, "active");
    }

    #[test]
    fn snapshot_describes_sizing() {
        let mut fixed = rule("F");
        fixed.fixed_sol = Some(0.05);
        let mut frac = rule("P");
        frac.fraction_of_their_size = 0.1;
        frac.max_sol = 0.5;
        let mut uncapped = rule("U");
        uncapped.max_sol = 0.0;
        let reg = LeaderRegistry::from_config(&[fixed, frac, uncapped]);
        let snap = reg.snapshot();
        assert_eq!(snap[0].sizing, "fixed 0.0500 SOL");
        assert_eq!(snap[1].sizing, "10.0% of theirs (max 0.5000 SOL)");
        assert_eq!(snap[2].sizing, "5.0% of theirs");
        assert_eq!(snap[0].label, "L-F");
        let unlabeled = CopyWallet {
            address: "ABCDEFGHIJKLMNOP".into(),
            ..CopyWallet::default()
        };
        assert_eq!(label_for(&unlabeled), "ABCDEFGH");
        assert_eq!(LeaderStatus::parse("paused"), Some(LeaderStatus::Paused));
        assert_eq!(LeaderStatus::parse("nope"), None);
    }

    #[test]
    fn history_is_bounded() {
        let mut reg = LeaderRegistry::from_config(&[rule("A")]);
        for _ in 0..(TRANSITION_HISTORY * 2) {
            reg.pause("A", None).unwrap();
            reg.resume("A", None).unwrap();
        }
        assert_eq!(reg.history().len(), TRANSITION_HISTORY);
    }
}
