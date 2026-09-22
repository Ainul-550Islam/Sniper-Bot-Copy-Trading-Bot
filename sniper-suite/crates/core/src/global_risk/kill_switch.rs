//! Per-venue and per-strategy kill switches (TASK 5 §1).
//!
//! The process-wide kill switch stays where it always was
//! (`AppState::kill_switch` / `set_kill_switch`, mirrored across replicas by
//! the runtime-flag sync). This registry adds the two narrower scopes the
//! global engine checks before any limit: a killed venue or strategy
//! refuses every NEW entry (`venue_kill_switch` / `strategy_kill_switch`)
//! while exits, cancels and reconciliation keep running.
//!
//! A switch is engaged when EITHER the configuration lists it
//! (`[global_risk].killed_venues` / `killed_strategies`) OR an operator
//! engaged it at runtime (durable in `kill_switches`, restored on start).
//! Either source can only tighten; releasing a config-listed switch at
//! runtime is refused until the configuration changes.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

use crate::models::Venue;

/// Scope of a switch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "scope", content = "key")]
pub enum KillScope {
    /// One venue.
    Venue(Venue),
    /// One strategy label.
    Strategy(String),
}

impl KillScope {
    /// Stable textual form (`venue:<venue>` / `strategy:<label>`).
    pub fn as_string(&self) -> String {
        match self {
            KillScope::Venue(v) => format!("venue:{}", v.as_str()),
            KillScope::Strategy(s) => format!("strategy:{s}"),
        }
    }

    /// Inverse of [`KillScope::as_string`].
    pub fn parse(s: &str) -> Option<KillScope> {
        let s = s.trim();
        if let Some(v) = s.strip_prefix("venue:") {
            return Venue::parse(v.trim()).map(KillScope::Venue);
        }
        if let Some(st) = s.strip_prefix("strategy:") {
            let st = st.trim();
            if st.is_empty() {
                return None;
            }
            return Some(KillScope::Strategy(st.to_string()));
        }
        None
    }

    /// `venue` / `strategy` (metric label).
    pub fn kind(&self) -> &'static str {
        match self {
            KillScope::Venue(_) => "venue",
            KillScope::Strategy(_) => "strategy",
        }
    }
}

impl std::fmt::Display for KillScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.as_string())
    }
}

/// Current state of one switch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillSwitchState {
    /// Scope.
    pub scope: KillScope,
    /// Engaged by configuration.
    pub configured: bool,
    /// Engaged by an operator at runtime.
    pub engaged: bool,
    /// Reason given.
    pub reason: String,
    /// Who engaged / released it last.
    pub actor: String,
    /// Last change.
    pub updated_at: DateTime<Utc>,
}

impl KillSwitchState {
    /// Effective state: configured OR engaged.
    pub fn is_active(&self) -> bool {
        self.configured || self.engaged
    }
}

/// One change, append-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillSwitchEvent {
    /// Scope.
    pub scope: KillScope,
    /// `engage` / `release`.
    pub action: String,
    /// Reason given.
    pub reason: String,
    /// Actor.
    pub actor: String,
    /// Replica.
    pub replica_id: String,
    /// When.
    pub ts: DateTime<Utc>,
}

/// Outcome of an engage / release request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchOutcome {
    /// State changed.
    Changed,
    /// Already in the requested state.
    Unchanged,
    /// Release refused: the scope is pinned by configuration.
    PinnedByConfig,
}

/// The registry.
#[derive(Default)]
pub struct KillSwitches {
    switches: RwLock<BTreeMap<KillScope, KillSwitchState>>,
}

impl KillSwitches {
    /// Empty registry.
    pub fn new() -> Self {
        KillSwitches::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<KillScope, KillSwitchState>> {
        self.switches.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<KillScope, KillSwitchState>> {
        self.switches.write().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the configuration-pinned set (startup and config reload).
    /// Runtime-engaged switches are kept.
    pub fn apply_config(&self, venues: &[Venue], strategies: &[String]) {
        let now = Utc::now();
        let mut map = self.write();
        for st in map.values_mut() {
            st.configured = false;
        }
        let scopes = venues
            .iter()
            .map(|v| KillScope::Venue(*v))
            .chain(strategies.iter().map(|s| KillScope::Strategy(s.clone())));
        for scope in scopes {
            let st = map.entry(scope.clone()).or_insert_with(|| KillSwitchState {
                scope,
                configured: false,
                engaged: false,
                reason: String::new(),
                actor: "config".into(),
                updated_at: now,
            });
            st.configured = true;
            if st.reason.is_empty() {
                st.reason = "configured".into();
            }
        }
        map.retain(|_, st| st.is_active());
    }

    /// Restore runtime-engaged switches from the durable store (startup).
    pub fn restore(&self, states: Vec<KillSwitchState>) -> usize {
        let mut map = self.write();
        let mut n = 0;
        for st in states {
            if !st.engaged {
                continue;
            }
            let entry = map.entry(st.scope.clone()).or_insert_with(|| st.clone());
            entry.engaged = true;
            if entry.reason.is_empty() || !entry.configured {
                entry.reason = st.reason.clone();
                entry.actor = st.actor.clone();
                entry.updated_at = st.updated_at;
            }
            n += 1;
        }
        n
    }

    /// Engage a scope at runtime.
    pub fn engage(&self, scope: KillScope, reason: &str, actor: &str) -> SwitchOutcome {
        let mut map = self.write();
        let now = Utc::now();
        let st = map.entry(scope.clone()).or_insert_with(|| KillSwitchState {
            scope,
            configured: false,
            engaged: false,
            reason: String::new(),
            actor: String::new(),
            updated_at: now,
        });
        if st.engaged {
            return SwitchOutcome::Unchanged;
        }
        st.engaged = true;
        st.reason = reason.to_string();
        st.actor = actor.to_string();
        st.updated_at = now;
        SwitchOutcome::Changed
    }

    /// Release a runtime-engaged scope. Refused while the configuration
    /// pins it.
    pub fn release(&self, scope: &KillScope, reason: &str, actor: &str) -> SwitchOutcome {
        let mut map = self.write();
        let Some(st) = map.get_mut(scope) else {
            return SwitchOutcome::Unchanged;
        };
        if st.configured {
            return SwitchOutcome::PinnedByConfig;
        }
        if !st.engaged {
            return SwitchOutcome::Unchanged;
        }
        st.engaged = false;
        st.reason = reason.to_string();
        st.actor = actor.to_string();
        st.updated_at = Utc::now();
        map.remove(scope);
        SwitchOutcome::Changed
    }

    /// Is the venue killed?
    pub fn venue_killed(&self, venue: Venue) -> Option<KillSwitchState> {
        self.read()
            .get(&KillScope::Venue(venue))
            .filter(|s| s.is_active())
            .cloned()
    }

    /// Is the strategy killed?
    pub fn strategy_killed(&self, strategy: &str) -> Option<KillSwitchState> {
        self.read()
            .get(&KillScope::Strategy(strategy.to_string()))
            .filter(|s| s.is_active())
            .cloned()
    }

    /// Every active switch, stable order.
    pub fn active(&self) -> Vec<KillSwitchState> {
        self.read()
            .values()
            .filter(|s| s.is_active())
            .cloned()
            .collect()
    }

    /// State of one scope (active or not).
    pub fn get(&self, scope: &KillScope) -> Option<KillSwitchState> {
        self.read().get(scope).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_text_round_trips() {
        let v = KillScope::Venue(Venue::PolymarketClob);
        assert_eq!(v.as_string(), "venue:polymarket");
        assert_eq!(KillScope::parse("venue:polymarket"), Some(v));
        let s = KillScope::Strategy("copy:leader".into());
        assert_eq!(KillScope::parse(&s.as_string()), Some(s));
        assert_eq!(KillScope::parse("venue:nowhere"), None);
        assert_eq!(KillScope::parse("strategy:"), None);
        assert_eq!(KillScope::parse("module:sniper"), None);
    }

    #[test]
    fn config_pins_and_runtime_toggles_compose() {
        let ks = KillSwitches::new();
        ks.apply_config(&[Venue::PumpFun], &["value".into()]);
        assert!(ks.venue_killed(Venue::PumpFun).is_some());
        assert!(ks.strategy_killed("value").is_some());
        assert!(ks.venue_killed(Venue::Jupiter).is_none());
        // Runtime engage on a new scope.
        assert_eq!(
            ks.engage(KillScope::Venue(Venue::Jupiter), "incident", "op"),
            SwitchOutcome::Changed
        );
        assert_eq!(
            ks.engage(KillScope::Venue(Venue::Jupiter), "again", "op"),
            SwitchOutcome::Unchanged
        );
        assert!(ks.venue_killed(Venue::Jupiter).is_some());
        // Config-pinned scopes cannot be released at runtime.
        assert_eq!(
            ks.release(&KillScope::Venue(Venue::PumpFun), "done", "op"),
            SwitchOutcome::PinnedByConfig
        );
        assert_eq!(
            ks.release(&KillScope::Venue(Venue::Jupiter), "done", "op"),
            SwitchOutcome::Changed
        );
        assert!(ks.venue_killed(Venue::Jupiter).is_none());
        // Config reload drops the pin; a runtime-engaged one survives.
        ks.engage(KillScope::Strategy("search".into()), "x", "op");
        ks.apply_config(&[], &[]);
        assert!(ks.venue_killed(Venue::PumpFun).is_none());
        assert!(ks.strategy_killed("value").is_none());
        assert!(ks.strategy_killed("search").is_some());
        assert_eq!(ks.active().len(), 1);
    }

    #[test]
    fn restore_only_brings_back_engaged_switches() {
        let ks = KillSwitches::new();
        let now = Utc::now();
        let n = ks.restore(vec![
            KillSwitchState {
                scope: KillScope::Venue(Venue::RaydiumAmmV4),
                configured: false,
                engaged: true,
                reason: "r".into(),
                actor: "op".into(),
                updated_at: now,
            },
            KillSwitchState {
                scope: KillScope::Strategy("old".into()),
                configured: false,
                engaged: false,
                reason: "released".into(),
                actor: "op".into(),
                updated_at: now,
            },
        ]);
        assert_eq!(n, 1);
        assert!(ks.venue_killed(Venue::RaydiumAmmV4).is_some());
        assert!(ks.strategy_killed("old").is_none());
    }
}
