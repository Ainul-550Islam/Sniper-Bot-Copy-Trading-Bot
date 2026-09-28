//! Product plans (TASK 7A file 12).
//!
//! A plan is the CATALOGUE entry: a stable machine code, a display name and
//! the feature limits it grants. It deliberately carries no price: money
//! amounts, currencies, tax and invoices live with the billing provider,
//! and putting them here would drag commercial assumptions into trading
//! code that must not depend on them.
//!
//! Limits are expressed as [`FeatureLimit`] values keyed by the same
//! feature strings the entitlement layer uses, so "what the plan says" and
//! "what the tenant may do" cannot drift into two vocabularies.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A plan identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlanId(pub Uuid);

impl PlanId {
    /// A fresh identifier.
    pub fn new() -> Self {
        PlanId(Uuid::new_v4())
    }

    /// The inner UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Parse the canonical string form.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(PlanId)
    }
}

impl Default for PlanId {
    fn default() -> Self {
        PlanId::new()
    }
}

impl fmt::Display for PlanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The initial tier vocabulary. Stable machine codes: renaming a tier for
/// marketing must not change what a tenant is entitled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanCode {
    /// Entry tier.
    Starter,
    /// Individual professional.
    Pro,
    /// Team.
    Business,
    /// Negotiated.
    Enterprise,
}

impl PlanCode {
    /// Every tier, weakest first.
    pub const ALL: [PlanCode; 4] = [
        PlanCode::Starter,
        PlanCode::Pro,
        PlanCode::Business,
        PlanCode::Enterprise,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanCode::Starter => "starter",
            PlanCode::Pro => "pro",
            PlanCode::Business => "business",
            PlanCode::Enterprise => "enterprise",
        }
    }

    /// Inverse of [`PlanCode::as_str`] (accepts the SCREAMING form too).
    pub fn parse(s: &str) -> Option<PlanCode> {
        let lower = s.trim().to_ascii_lowercase();
        PlanCode::ALL.iter().copied().find(|p| p.as_str() == lower)
    }
}

impl fmt::Display for PlanCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Catalogue state of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    /// Sellable.
    Active,
    /// Existing subscribers keep it; nobody new may subscribe.
    Deprecated,
    /// Not listed publicly (custom / enterprise).
    Private,
}

impl PlanStatus {
    /// Every status, stable order.
    pub const ALL: [PlanStatus; 3] = [
        PlanStatus::Active,
        PlanStatus::Deprecated,
        PlanStatus::Private,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanStatus::Active => "active",
            PlanStatus::Deprecated => "deprecated",
            PlanStatus::Private => "private",
        }
    }

    /// Inverse of [`PlanStatus::as_str`].
    pub fn parse(s: &str) -> Option<PlanStatus> {
        PlanStatus::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// May a NEW subscription be created on this plan?
    pub fn is_subscribable(&self) -> bool {
        matches!(self, PlanStatus::Active | PlanStatus::Private)
    }
}

impl fmt::Display for PlanStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a plan grants for one feature.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureLimit {
    /// The feature is off on this plan.
    Disabled,
    /// On, with no numeric ceiling.
    Unlimited,
    /// On, up to this many.
    Limited(f64),
}

impl FeatureLimit {
    /// Is the feature usable at all?
    pub fn is_enabled(&self) -> bool {
        !matches!(self, FeatureLimit::Disabled)
    }

    /// The ceiling, when there is one.
    pub fn limit(&self) -> Option<f64> {
        match self {
            FeatureLimit::Limited(v) => Some(*v),
            _ => None,
        }
    }

    /// Would `current + requested` stay inside the limit?
    pub fn allows(&self, current: f64, requested: f64) -> bool {
        match self {
            FeatureLimit::Disabled => false,
            FeatureLimit::Unlimited => true,
            FeatureLimit::Limited(max) => current + requested <= *max,
        }
    }

    /// Stable label for metrics / audit.
    pub fn as_str(&self) -> &'static str {
        match self {
            FeatureLimit::Disabled => "disabled",
            FeatureLimit::Unlimited => "unlimited",
            FeatureLimit::Limited(_) => "limited",
        }
    }
}

/// One catalogue entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// Identity.
    pub id: PlanId,
    /// Stable machine code.
    pub code: PlanCode,
    /// Display name.
    pub name: String,
    /// Catalogue state.
    pub status: PlanStatus,
    /// Feature limits, keyed by the shared feature vocabulary.
    pub limits: BTreeMap<String, FeatureLimit>,
    /// Marketing description (no pricing logic).
    pub description: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update.
    pub updated_at: DateTime<Utc>,
}

impl Plan {
    /// A plan with no limits set (everything falls back to disabled).
    pub fn new(code: PlanCode, name: impl Into<String>, now: DateTime<Utc>) -> Self {
        Plan {
            id: PlanId::new(),
            code,
            name: name.into(),
            status: PlanStatus::Active,
            limits: BTreeMap::new(),
            description: String::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Builder: set one limit.
    pub fn with_limit(mut self, feature: impl Into<String>, limit: FeatureLimit) -> Self {
        self.limits.insert(feature.into(), limit);
        self
    }

    /// What this plan says about `feature`. An unknown feature is
    /// `Disabled`: a plan grants only what it lists, never by omission.
    pub fn limit_for(&self, feature: &str) -> FeatureLimit {
        self.limits
            .get(feature)
            .copied()
            .unwrap_or(FeatureLimit::Disabled)
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "plan={} code={} status={} features={}",
            self.id,
            self.code,
            self.status,
            self.limits.len()
        )
    }
}

/// The feature keys the product ships with. Central so a plan, an
/// entitlement and a handler cannot invent three different spellings of
/// the same capability.
pub mod features {
    /// May the tenant run the sniper module?
    pub const MODULE_SNIPER: &str = "module.sniper";
    /// May the tenant run copy trading?
    pub const MODULE_COPY: &str = "module.copy";
    /// May the tenant run the Polymarket module?
    pub const MODULE_POLYMARKET: &str = "module.polymarket";
    /// May the tenant trade LIVE (as opposed to paper / simulate)?
    pub const LIVE_TRADING: &str = "feature.live_trading";
    /// May the tenant create API keys?
    pub const API_KEYS: &str = "feature.api_keys";
    /// May the tenant export data?
    pub const EXPORTS: &str = "feature.exports";
    /// How many members the organization may have.
    pub const MAX_MEMBERS: &str = "limit.max_members";
    /// How many API keys may exist at once.
    pub const MAX_API_KEYS: &str = "limit.max_api_keys";
    /// How many trading modules may run at once.
    pub const MAX_ACTIVE_MODULES: &str = "limit.max_active_modules";
    /// Monthly order allowance.
    pub const MONTHLY_ORDERS: &str = "limit.monthly_orders";

    /// Every shipped feature key, stable order.
    pub const ALL: [&str; 10] = [
        MODULE_SNIPER,
        MODULE_COPY,
        MODULE_POLYMARKET,
        LIVE_TRADING,
        API_KEYS,
        EXPORTS,
        MAX_MEMBERS,
        MAX_API_KEYS,
        MAX_ACTIVE_MODULES,
        MONTHLY_ORDERS,
    ];
}

/// The default catalogue: four tiers with increasing limits. Deployments
/// may replace it entirely — this is a starting point, not a pricing
/// commitment.
pub fn default_catalogue(now: DateTime<Utc>) -> Vec<Plan> {
    use features as f;
    use FeatureLimit::{Disabled, Limited, Unlimited};

    vec![
        Plan::new(PlanCode::Starter, "Starter", now)
            .with_limit(f::MODULE_SNIPER, Limited(1.0))
            .with_limit(f::MODULE_COPY, Disabled)
            .with_limit(f::MODULE_POLYMARKET, Disabled)
            .with_limit(f::LIVE_TRADING, Disabled)
            .with_limit(f::API_KEYS, Limited(1.0))
            .with_limit(f::EXPORTS, Disabled)
            .with_limit(f::MAX_MEMBERS, Limited(2.0))
            .with_limit(f::MAX_API_KEYS, Limited(1.0))
            .with_limit(f::MAX_ACTIVE_MODULES, Limited(1.0))
            .with_limit(f::MONTHLY_ORDERS, Limited(1_000.0)),
        Plan::new(PlanCode::Pro, "Pro", now)
            .with_limit(f::MODULE_SNIPER, Unlimited)
            .with_limit(f::MODULE_COPY, Unlimited)
            .with_limit(f::MODULE_POLYMARKET, Disabled)
            .with_limit(f::LIVE_TRADING, Unlimited)
            .with_limit(f::API_KEYS, Unlimited)
            .with_limit(f::EXPORTS, Unlimited)
            .with_limit(f::MAX_MEMBERS, Limited(5.0))
            .with_limit(f::MAX_API_KEYS, Limited(5.0))
            .with_limit(f::MAX_ACTIVE_MODULES, Limited(2.0))
            .with_limit(f::MONTHLY_ORDERS, Limited(25_000.0)),
        Plan::new(PlanCode::Business, "Business", now)
            .with_limit(f::MODULE_SNIPER, Unlimited)
            .with_limit(f::MODULE_COPY, Unlimited)
            .with_limit(f::MODULE_POLYMARKET, Unlimited)
            .with_limit(f::LIVE_TRADING, Unlimited)
            .with_limit(f::API_KEYS, Unlimited)
            .with_limit(f::EXPORTS, Unlimited)
            .with_limit(f::MAX_MEMBERS, Limited(25.0))
            .with_limit(f::MAX_API_KEYS, Limited(25.0))
            .with_limit(f::MAX_ACTIVE_MODULES, Limited(3.0))
            .with_limit(f::MONTHLY_ORDERS, Limited(250_000.0)),
        {
            let mut p = Plan::new(PlanCode::Enterprise, "Enterprise", now)
                .with_limit(f::MODULE_SNIPER, Unlimited)
                .with_limit(f::MODULE_COPY, Unlimited)
                .with_limit(f::MODULE_POLYMARKET, Unlimited)
                .with_limit(f::LIVE_TRADING, Unlimited)
                .with_limit(f::API_KEYS, Unlimited)
                .with_limit(f::EXPORTS, Unlimited)
                .with_limit(f::MAX_MEMBERS, Unlimited)
                .with_limit(f::MAX_API_KEYS, Unlimited)
                .with_limit(f::MAX_ACTIVE_MODULES, Unlimited)
                .with_limit(f::MONTHLY_ORDERS, Unlimited);
            p.status = PlanStatus::Private;
            p
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabularies_round_trip() {
        for c in PlanCode::ALL {
            assert_eq!(PlanCode::parse(c.as_str()), Some(c));
            assert_eq!(PlanCode::parse(&c.as_str().to_ascii_uppercase()), Some(c));
        }
        assert_eq!(PlanCode::parse("platinum"), None);
        for s in PlanStatus::ALL {
            assert_eq!(PlanStatus::parse(s.as_str()), Some(s));
        }
        let id = PlanId::new();
        assert_eq!(PlanId::parse(&id.to_string()), Some(id));
    }

    #[test]
    fn a_plan_grants_only_what_it_lists() {
        let p = Plan::new(PlanCode::Starter, "Starter", Utc::now())
            .with_limit(features::MODULE_SNIPER, FeatureLimit::Limited(1.0));
        assert!(p.limit_for(features::MODULE_SNIPER).is_enabled());
        assert_eq!(
            p.limit_for(features::MODULE_POLYMARKET),
            FeatureLimit::Disabled,
            "an unlisted feature is off, never on by omission"
        );
        assert!(!p.limit_for("feature.invented").is_enabled());
    }

    #[test]
    fn limit_arithmetic_is_explicit() {
        assert!(!FeatureLimit::Disabled.allows(0.0, 1.0));
        assert!(FeatureLimit::Unlimited.allows(1e9, 1e9));
        let l = FeatureLimit::Limited(5.0);
        assert!(l.allows(4.0, 1.0), "exactly at the limit is allowed");
        assert!(!l.allows(5.0, 1.0));
        assert_eq!(l.limit(), Some(5.0));
        assert_eq!(FeatureLimit::Unlimited.limit(), None);
        assert_eq!(l.as_str(), "limited");
    }

    #[test]
    fn the_default_catalogue_is_monotonic_and_complete() {
        let now = Utc::now();
        let cat = default_catalogue(now);
        assert_eq!(cat.len(), 4);
        for p in &cat {
            for key in features::ALL {
                assert!(
                    p.limits.contains_key(key),
                    "{} must state {key} explicitly",
                    p.code
                );
            }
        }
        let starter = &cat[0];
        let pro = &cat[1];
        let business = &cat[2];
        let enterprise = &cat[3];
        // Higher tiers never take a capability away.
        for key in features::ALL {
            let order = [starter, pro, business, enterprise];
            let mut enabled_seen = false;
            for p in order {
                let en = p.limit_for(key).is_enabled();
                if enabled_seen {
                    assert!(en, "{} loses {key} that a lower tier had", p.code);
                }
                enabled_seen |= en;
            }
        }
        assert!(!starter.limit_for(features::LIVE_TRADING).is_enabled());
        assert!(pro.limit_for(features::LIVE_TRADING).is_enabled());
        assert!(!pro.limit_for(features::MODULE_POLYMARKET).is_enabled());
        assert!(business.limit_for(features::MODULE_POLYMARKET).is_enabled());
        assert_eq!(enterprise.status, PlanStatus::Private);
        assert!(enterprise.status.is_subscribable());
        assert!(!PlanStatus::Deprecated.is_subscribable());
    }

    #[test]
    fn no_price_leaks_into_the_plan_model() {
        let json = serde_json::to_string(&default_catalogue(Utc::now())[0]).unwrap();
        for word in ["price", "amount", "currency", "usd", "cents"] {
            assert!(
                !json.to_ascii_lowercase().contains(word),
                "the plan model must stay provider-neutral: found {word}"
            );
        }
    }
}
