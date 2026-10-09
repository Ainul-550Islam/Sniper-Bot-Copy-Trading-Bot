//! Usage metering (TASK 7A file 15).
//!
//! Append-only, idempotent counting of what a tenant consumed. The rule is
//! the same one the TASK 5 ledger applies to money: **the same reported
//! event must produce exactly one metering effect**, whichever worker
//! reports it and however many times it is retried.
//!
//! The identity is `(organization_id, idempotency_key)`, enforced by the
//! normalized schema in migration 0017 and by the runtime projection in
//! migration 0018. [`UsageLedger`] mirrors it for tests and no-database runs.
//! Callers build the
//! key deterministically from the fact being metered
//! ([`UsageEvent::key_for`]), so a retry produces the same key rather than
//! a second row.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::tenant::OrganizationId;

/// What is being counted. Closed vocabulary — a bounded metric label and a
/// reviewable list of everything the product meters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageMetric {
    /// Orders submitted to a venue.
    OrdersSubmitted,
    /// Fills booked into the global ledger.
    FillsBooked,
    /// Control-plane API requests.
    ApiRequests,
    /// Seconds a trading module spent running.
    ModuleRuntimeSeconds,
    /// Rows exported.
    ExportRows,
    /// Members in the organization (a level, sampled, not a sum).
    ActiveMembers,
}

impl UsageMetric {
    /// Every metric, stable order.
    pub const ALL: [UsageMetric; 6] = [
        UsageMetric::OrdersSubmitted,
        UsageMetric::FillsBooked,
        UsageMetric::ApiRequests,
        UsageMetric::ModuleRuntimeSeconds,
        UsageMetric::ExportRows,
        UsageMetric::ActiveMembers,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            UsageMetric::OrdersSubmitted => "orders_submitted",
            UsageMetric::FillsBooked => "fills_booked",
            UsageMetric::ApiRequests => "api_requests",
            UsageMetric::ModuleRuntimeSeconds => "module_runtime_seconds",
            UsageMetric::ExportRows => "export_rows",
            UsageMetric::ActiveMembers => "active_members",
        }
    }

    /// Inverse of [`UsageMetric::as_str`].
    pub fn parse(s: &str) -> Option<UsageMetric> {
        UsageMetric::ALL
            .iter()
            .copied()
            .find(|m| m.as_str() == s.trim())
    }

    /// True when the metric accumulates over a period (billable volume);
    /// false when it is a level sampled at a point in time.
    pub fn is_cumulative(&self) -> bool {
        !matches!(self, UsageMetric::ActiveMembers)
    }
}

impl fmt::Display for UsageMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which part of the system reported the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    /// The sniper module.
    Sniper,
    /// The copy-trading module.
    Copy,
    /// The Polymarket module.
    Polymarket,
    /// The control-plane API.
    Api,
    /// A background/system job.
    System,
}

impl UsageSource {
    /// Every source, stable order.
    pub const ALL: [UsageSource; 5] = [
        UsageSource::Sniper,
        UsageSource::Copy,
        UsageSource::Polymarket,
        UsageSource::Api,
        UsageSource::System,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            UsageSource::Sniper => "sniper",
            UsageSource::Copy => "copy",
            UsageSource::Polymarket => "polymarket",
            UsageSource::Api => "api",
            UsageSource::System => "system",
        }
    }

    /// Inverse of [`UsageSource::as_str`].
    pub fn parse(s: &str) -> Option<UsageSource> {
        UsageSource::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }
}

impl fmt::Display for UsageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One metered fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Row identity (not the idempotency identity).
    pub id: Uuid,
    /// The tenant that consumed it.
    pub organization_id: OrganizationId,
    /// What was consumed.
    pub metric: UsageMetric,
    /// How much (`>= 0`).
    pub quantity: f64,
    /// Who reported it.
    pub source: UsageSource,
    /// THE metering identity together with the tenant.
    pub idempotency_key: String,
    /// Free-form single-line detail.
    pub detail: String,
    /// When the fact happened.
    pub occurred_at: DateTime<Utc>,
    /// When it was recorded.
    pub recorded_at: DateTime<Utc>,
}

impl UsageEvent {
    /// Build an event with an explicit idempotency key.
    pub fn new(
        organization_id: OrganizationId,
        metric: UsageMetric,
        quantity: f64,
        source: UsageSource,
        idempotency_key: impl Into<String>,
        occurred_at: DateTime<Utc>,
    ) -> Self {
        UsageEvent {
            id: Uuid::new_v4(),
            organization_id,
            metric,
            quantity,
            source,
            idempotency_key: idempotency_key.into(),
            detail: String::new(),
            occurred_at,
            recorded_at: Utc::now(),
        }
    }

    /// A deterministic idempotency key for a fact: the same
    /// `(metric, subject)` always yields the same key, so a retry from any
    /// worker collapses onto one row.
    pub fn key_for(metric: UsageMetric, subject: &str) -> String {
        let mut h = Sha256::new();
        h.update(b"usage-v1|");
        h.update(metric.as_str().as_bytes());
        h.update(b"|");
        h.update(subject.trim().as_bytes());
        format!("use_{}", &hex::encode(h.finalize())[..32])
    }

    /// Structural validation. `Err` names the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if self.idempotency_key.trim().is_empty() {
            return Err("idempotency_key must not be empty".into());
        }
        if !self.quantity.is_finite() {
            return Err("quantity must be finite".into());
        }
        if self.quantity < 0.0 {
            return Err("quantity must be >= 0".into());
        }
        Ok(())
    }

    /// The `YYYY-MM` bucket this event belongs to (monthly allowances).
    pub fn period(&self) -> String {
        format!(
            "{:04}-{:02}",
            self.occurred_at.year(),
            self.occurred_at.month()
        )
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "usage organization={} metric={} quantity={} source={} key={} period={}",
            self.organization_id,
            self.metric,
            self.quantity,
            self.source,
            self.idempotency_key,
            self.period()
        )
    }
}

/// What [`UsageLedger::record`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageOutcome {
    /// First sighting: counted.
    Recorded,
    /// Already counted for this tenant — no second effect.
    Duplicate,
    /// Malformed; nothing changed.
    Rejected(String),
}

impl UsageOutcome {
    /// True when the event was counted.
    pub fn is_recorded(&self) -> bool {
        matches!(self, UsageOutcome::Recorded)
    }

    /// Stable label for metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            UsageOutcome::Recorded => "recorded",
            UsageOutcome::Duplicate => "duplicate",
            UsageOutcome::Rejected(_) => "rejected",
        }
    }
}

/// In-memory metering with the same idempotency semantics as the durable
/// table. Used by tests and no-database runs.
#[derive(Debug, Default)]
pub struct UsageLedger {
    seen: HashSet<(OrganizationId, String)>,
    events: Vec<UsageEvent>,
}

impl UsageLedger {
    /// An empty ledger.
    pub fn new() -> Self {
        UsageLedger::default()
    }

    /// Record one event. The same `(tenant, idempotency_key)` is counted
    /// exactly once.
    pub fn record(&mut self, event: UsageEvent) -> UsageOutcome {
        if let Err(e) = event.validate() {
            return UsageOutcome::Rejected(e);
        }
        let key = (event.organization_id, event.idempotency_key.clone());
        if !self.seen.insert(key) {
            return UsageOutcome::Duplicate;
        }
        self.events.push(event);
        UsageOutcome::Recorded
    }

    /// Total of one metric for one tenant in one `YYYY-MM` period.
    pub fn total(&self, organization_id: OrganizationId, metric: UsageMetric, period: &str) -> f64 {
        self.events
            .iter()
            .filter(|e| {
                e.organization_id == organization_id && e.metric == metric && e.period() == period
            })
            .map(|e| e.quantity)
            .sum()
    }

    /// Every metric's total for one tenant in one period (billing rollup).
    pub fn totals(
        &self,
        organization_id: OrganizationId,
        period: &str,
    ) -> BTreeMap<UsageMetric, f64> {
        let mut out = BTreeMap::new();
        for e in self
            .events
            .iter()
            .filter(|e| e.organization_id == organization_id && e.period() == period)
        {
            *out.entry(e.metric).or_insert(0.0) += e.quantity;
        }
        out
    }

    /// Events of one tenant, oldest first. Tenant-scoped by construction:
    /// there is no "all events" accessor that could leak across tenants.
    pub fn events_of(&self, organization_id: OrganizationId) -> Vec<&UsageEvent> {
        self.events
            .iter()
            .filter(|e| e.organization_id == organization_id)
            .collect()
    }

    /// How many events are stored in total.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// True when nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn event(org: OrganizationId, key: &str, qty: f64, at: DateTime<Utc>) -> UsageEvent {
        UsageEvent::new(
            org,
            UsageMetric::OrdersSubmitted,
            qty,
            UsageSource::Sniper,
            key,
            at,
        )
    }

    #[test]
    fn the_same_event_is_counted_exactly_once() {
        let org = OrganizationId::new();
        let now = Utc::now();
        let mut ledger = UsageLedger::new();
        let e = event(org, "order-1", 1.0, now);

        assert_eq!(ledger.record(e.clone()), UsageOutcome::Recorded);
        assert_eq!(ledger.record(e.clone()), UsageOutcome::Duplicate);
        // A retry that carries a different quantity is STILL the same fact.
        let mut retry = e.clone();
        retry.quantity = 999.0;
        retry.id = Uuid::new_v4();
        assert_eq!(ledger.record(retry), UsageOutcome::Duplicate);

        assert_eq!(ledger.len(), 1);
        assert_eq!(
            ledger.total(org, UsageMetric::OrdersSubmitted, &e.period()),
            1.0
        );
    }

    #[test]
    fn the_same_key_in_two_tenants_is_two_facts() {
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let now = Utc::now();
        let mut ledger = UsageLedger::new();
        assert_eq!(
            ledger.record(event(a, "order-1", 1.0, now)),
            UsageOutcome::Recorded
        );
        assert_eq!(
            ledger.record(event(b, "order-1", 1.0, now)),
            UsageOutcome::Recorded,
            "idempotency is scoped to the tenant"
        );
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger.events_of(a).len(), 1);
        assert_eq!(ledger.events_of(b).len(), 1);
    }

    #[test]
    fn deterministic_keys_collapse_retries_from_any_worker() {
        let k1 = UsageEvent::key_for(UsageMetric::OrdersSubmitted, "order-abc");
        let k2 = UsageEvent::key_for(UsageMetric::OrdersSubmitted, " order-abc ");
        assert_eq!(k1, k2, "whitespace must not create a second fact");
        assert_ne!(
            k1,
            UsageEvent::key_for(UsageMetric::FillsBooked, "order-abc"),
            "a different metric is a different fact"
        );
        assert!(k1.starts_with("use_"));
        assert_eq!(k1.len(), 4 + 32);
    }

    #[test]
    fn malformed_events_change_nothing() {
        let org = OrganizationId::new();
        let now = Utc::now();
        let mut ledger = UsageLedger::new();
        let mut bad = event(org, "", 1.0, now);
        assert!(matches!(
            ledger.record(bad.clone()),
            UsageOutcome::Rejected(_)
        ));
        bad.idempotency_key = "ok".into();
        bad.quantity = -1.0;
        assert!(matches!(
            ledger.record(bad.clone()),
            UsageOutcome::Rejected(_)
        ));
        bad.quantity = f64::NAN;
        assert!(matches!(ledger.record(bad), UsageOutcome::Rejected(_)));
        assert!(ledger.is_empty());
    }

    #[test]
    fn totals_are_per_tenant_metric_and_period() {
        let org = OrganizationId::new();
        let jan = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
        let feb = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let mut ledger = UsageLedger::new();
        ledger.record(event(org, "a", 2.0, jan));
        ledger.record(event(org, "b", 3.0, jan));
        ledger.record(event(org, "c", 7.0, feb));
        ledger.record(UsageEvent::new(
            org,
            UsageMetric::ApiRequests,
            10.0,
            UsageSource::Api,
            "d",
            jan,
        ));

        assert_eq!(
            ledger.total(org, UsageMetric::OrdersSubmitted, "2026-01"),
            5.0
        );
        assert_eq!(
            ledger.total(org, UsageMetric::OrdersSubmitted, "2026-02"),
            7.0
        );
        let totals = ledger.totals(org, "2026-01");
        assert_eq!(totals[&UsageMetric::OrdersSubmitted], 5.0);
        assert_eq!(totals[&UsageMetric::ApiRequests], 10.0);
        assert_eq!(totals.len(), 2);
        // Another tenant sees nothing.
        let other = OrganizationId::new();
        assert_eq!(
            ledger.total(other, UsageMetric::OrdersSubmitted, "2026-01"),
            0.0
        );
        assert!(ledger.events_of(other).is_empty());
    }

    #[test]
    fn vocabularies_round_trip() {
        for m in UsageMetric::ALL {
            assert_eq!(UsageMetric::parse(m.as_str()), Some(m));
        }
        for s in UsageSource::ALL {
            assert_eq!(UsageSource::parse(s.as_str()), Some(s));
        }
        assert_eq!(UsageMetric::parse("nope"), None);
        assert!(UsageMetric::OrdersSubmitted.is_cumulative());
        assert!(!UsageMetric::ActiveMembers.is_cumulative());
        assert_eq!(UsageOutcome::Recorded.as_str(), "recorded");
    }
}
