//! Consent model: the durable record that lets one Telegram chat trade.
//!
//! A consent is a *capability*, not a permission flag: it carries the caps and
//! the expiry that bound every order issued under it. It is only ever created
//! by [`crate::commands::consent_cmd`] after the operator echoed the typed
//! confirmation phrase, and it is only ever interpreted by
//! [`crate::consent::policy`].
//!
//! Money fields are integer lamports (`u64`) — no `f64` appears anywhere in
//! this model.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::trade_port::TradeAction;

/// Which actions a consent covers.
///
/// Stored in SQL as `text[]` constrained to a subset of `('buy','sell')`; in
/// Rust it is a two-bit set so "scope includes the action" is a cheap,
/// total check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ConsentScope {
    buy: bool,
    sell: bool,
}

impl ConsentScope {
    /// Scope covering both actions (the default a fresh grant gets).
    pub fn both() -> Self {
        Self {
            buy: true,
            sell: true,
        }
    }

    /// Scope covering exactly one action.
    pub fn only(action: TradeAction) -> Self {
        match action {
            TradeAction::Buy => Self {
                buy: true,
                sell: false,
            },
            TradeAction::Sell => Self {
                buy: false,
                sell: true,
            },
        }
    }

    /// Build from the SQL representation. Unknown labels are rejected so a
    /// corrupted row can never silently widen a scope.
    pub fn from_labels(labels: &[String]) -> Result<Self, ConsentError> {
        if labels.is_empty() {
            return Err(ConsentError::EmptyScope);
        }
        let mut scope = Self::default();
        for label in labels {
            match TradeAction::from_label(label) {
                Some(TradeAction::Buy) => scope.buy = true,
                Some(TradeAction::Sell) => scope.sell = true,
                None => return Err(ConsentError::UnknownScopeLabel(label.clone())),
            }
        }
        Ok(scope)
    }

    /// `true` when this scope permits `action`.
    pub fn allows(&self, action: TradeAction) -> bool {
        match action {
            TradeAction::Buy => self.buy,
            TradeAction::Sell => self.sell,
        }
    }

    /// The labels enabled by this scope, in a stable order.
    pub fn labels(&self) -> Vec<&'static str> {
        let mut out = Vec::with_capacity(2);
        if self.buy {
            out.push(TradeAction::Buy.as_str());
        }
        if self.sell {
            out.push(TradeAction::Sell.as_str());
        }
        out
    }

    /// The SQL `text[]` representation.
    pub fn to_sql_array(&self) -> Vec<String> {
        self.labels().into_iter().map(str::to_string).collect()
    }

    /// `true` when the scope covers nothing (never constructible from SQL, but
    /// `Default` gives it, and it must fail closed).
    pub fn is_empty(&self) -> bool {
        !self.buy && !self.sell
    }
}

/// Lifecycle state of a consent, computed from its timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentStatus {
    /// Not revoked and not yet expired — the only state that may trade.
    Active,
    /// `expires_at` has passed.
    Expired,
    /// `revoked_at` is set (revocation always wins over expiry).
    Revoked,
}

impl ConsentStatus {
    /// Stable label used in replies and audit rows.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConsentStatus::Active => "active",
            ConsentStatus::Expired => "expired",
            ConsentStatus::Revoked => "revoked",
        }
    }

    /// `true` for [`ConsentStatus::Active`].
    pub fn is_active(&self) -> bool {
        matches!(self, ConsentStatus::Active)
    }
}

/// One consent row (`telegram_trade_consents`, migration 0066).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consent {
    /// Primary key.
    pub consent_id: Uuid,
    /// Tenant that owns the chat binding.
    pub organization_id: String,
    /// Telegram user the consent was granted by/for.
    pub user_id: i64,
    /// Telegram chat the consent applies to (one active consent per chat).
    pub chat_id: i64,
    /// Actions covered.
    pub scope: ConsentScope,
    /// Per-order ceiling in lamports.
    pub max_order_lamports: u64,
    /// Rolling 24h buy ceiling in lamports.
    pub daily_cap_lamports: u64,
    /// When the operator granted it.
    pub granted_at: DateTime<Utc>,
    /// When it stops being valid (`> granted_at`; enforced by a CHECK).
    pub expires_at: DateTime<Utc>,
    /// When it was revoked, if it was.
    pub revoked_at: Option<DateTime<Utc>>,
    /// SHA-256 of the normalized confirmation phrase that was echoed. The
    /// phrase itself is never stored, and neither is any token or key.
    pub phrase_sha256: [u8; 32],
}

impl Consent {
    /// The status as of `now`.
    pub fn status(&self, now: DateTime<Utc>) -> ConsentStatus {
        if self.revoked_at.is_some() {
            ConsentStatus::Revoked
        } else if self.expires_at <= now {
            ConsentStatus::Expired
        } else {
            ConsentStatus::Active
        }
    }

    /// `true` when this consent may authorise `action` at `now`.
    pub fn authorises(&self, action: TradeAction, now: DateTime<Utc>) -> bool {
        self.status(now).is_active() && self.scope.allows(action)
    }

    /// Seconds left before expiry (never negative).
    pub fn seconds_remaining(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_seconds().max(0)
    }
}

/// Typed consent failures. Every variant has a stable code so the audit trail
/// and the reply text stay separated (single owner: `commands::reply`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentError {
    /// The typed confirmation phrase did not match.
    PhraseMismatch,
    /// The scope array was empty or unusable.
    EmptyScope,
    /// The scope array carried a label that is not `buy`/`sell`.
    UnknownScopeLabel(String),
    /// `max_order_lamports` was zero.
    OrderCapZero,
    /// `daily_cap_lamports` was below `max_order_lamports`.
    DailyCapBelowOrder,
    /// A requested cap exceeds the organization's own risk limit.
    ExceedsOrgOrderLimit {
        /// What the operator asked for.
        requested: u64,
        /// The organization's hard ceiling.
        ceiling: u64,
    },
    /// The requested daily cap exceeds the organization's own risk limit.
    ExceedsOrgDailyLimit {
        /// What the operator asked for.
        requested: u64,
        /// The organization's hard ceiling.
        ceiling: u64,
    },
    /// The requested TTL is outside the allowed window.
    TtlOutOfRange {
        /// Requested hours.
        requested: u32,
        /// Minimum hours.
        min: u32,
        /// Maximum hours.
        max: u32,
    },
    /// No consent exists for this chat.
    Missing,
    /// The consent exists but expired.
    Expired,
    /// The consent exists but was revoked.
    Revoked,
    /// The consent is active but does not cover this action.
    ScopeDenied(TradeAction),
    /// An unexpired consent already exists (revoke it first).
    AlreadyActive,
    /// The consent store is unavailable — fail closed.
    StoreUnavailable(String),
}

impl ConsentError {
    /// Stable machine code (also used as the command `failure_code`).
    pub fn code(&self) -> &'static str {
        match self {
            ConsentError::PhraseMismatch => "phrase_mismatch",
            ConsentError::EmptyScope => "empty_scope",
            ConsentError::UnknownScopeLabel(_) => "unknown_scope_label",
            ConsentError::OrderCapZero => "order_cap_zero",
            ConsentError::DailyCapBelowOrder => "daily_cap_below_order",
            ConsentError::ExceedsOrgOrderLimit { .. } => "exceeds_org_order_limit",
            ConsentError::ExceedsOrgDailyLimit { .. } => "exceeds_org_daily_limit",
            ConsentError::TtlOutOfRange { .. } => "ttl_out_of_range",
            ConsentError::Missing => "no_consent",
            ConsentError::Expired => "consent_expired",
            ConsentError::Revoked => "consent_revoked",
            ConsentError::ScopeDenied(_) => "consent_scope_denied",
            ConsentError::AlreadyActive => "consent_already_active",
            ConsentError::StoreUnavailable(_) => "consent_store_unavailable",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consent() -> Consent {
        Consent {
            consent_id: Uuid::from_u128(7),
            organization_id: "org-1".into(),
            user_id: 42,
            chat_id: 100,
            scope: ConsentScope::both(),
            max_order_lamports: 50_000_000,
            daily_cap_lamports: 200_000_000,
            granted_at: DateTime::from_timestamp(1_000, 0).unwrap(),
            expires_at: DateTime::from_timestamp(4_600, 0).unwrap(),
            revoked_at: None,
            phrase_sha256: [0u8; 32],
        }
    }

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn scope_labels_round_trip_through_sql_form() {
        let both = ConsentScope::both();
        assert_eq!(both.to_sql_array(), vec!["buy".to_string(), "sell".into()]);
        assert_eq!(
            ConsentScope::from_labels(&both.to_sql_array()).unwrap(),
            both
        );
        let buy = ConsentScope::only(TradeAction::Buy);
        assert!(buy.allows(TradeAction::Buy));
        assert!(!buy.allows(TradeAction::Sell));
        assert_eq!(buy.labels(), vec!["buy"]);
        assert_eq!(ConsentScope::default().labels(), Vec::<&str>::new());
        assert!(ConsentScope::default().is_empty());
    }

    #[test]
    fn scope_rejects_garbage_and_empty() {
        assert_eq!(
            ConsentScope::from_labels(&[]),
            Err(ConsentError::EmptyScope)
        );
        assert_eq!(
            ConsentScope::from_labels(&["buy".into(), "transfer".into()]),
            Err(ConsentError::UnknownScopeLabel("transfer".into()))
        );
        assert!(ConsentScope::from_labels(&["SELL".into()])
            .unwrap()
            .allows(TradeAction::Sell));
    }

    #[test]
    fn status_prefers_revocation_then_expiry() {
        let c = consent();
        assert_eq!(c.status(at(2_000)), ConsentStatus::Active);
        assert!(c.status(at(2_000)).is_active());
        assert_eq!(c.status(at(4_600)), ConsentStatus::Expired);
        assert_eq!(c.status(at(9_999)), ConsentStatus::Expired);
        let mut revoked = consent();
        revoked.revoked_at = Some(at(2_000));
        assert_eq!(revoked.status(at(2_001)), ConsentStatus::Revoked);
        assert_eq!(revoked.status(at(9_999)), ConsentStatus::Revoked);
        assert_eq!(ConsentStatus::Active.as_str(), "active");
        assert_eq!(ConsentStatus::Expired.as_str(), "expired");
        assert_eq!(ConsentStatus::Revoked.as_str(), "revoked");
    }

    #[test]
    fn authorises_requires_active_scope() {
        let mut c = consent();
        assert!(c.authorises(TradeAction::Buy, at(2_000)));
        assert!(c.authorises(TradeAction::Sell, at(2_000)));
        assert!(!c.authorises(TradeAction::Buy, at(5_000)));
        c.scope = ConsentScope::only(TradeAction::Sell);
        assert!(!c.authorises(TradeAction::Buy, at(2_000)));
        assert!(c.authorises(TradeAction::Sell, at(2_000)));
        assert_eq!(c.seconds_remaining(at(4_000)), 600);
        assert_eq!(c.seconds_remaining(at(5_000)), 0);
    }

    #[test]
    fn every_error_has_a_distinct_code() {
        let codes = [
            ConsentError::PhraseMismatch.code(),
            ConsentError::EmptyScope.code(),
            ConsentError::UnknownScopeLabel("x".into()).code(),
            ConsentError::OrderCapZero.code(),
            ConsentError::DailyCapBelowOrder.code(),
            ConsentError::ExceedsOrgOrderLimit {
                requested: 2,
                ceiling: 1,
            }
            .code(),
            ConsentError::ExceedsOrgDailyLimit {
                requested: 2,
                ceiling: 1,
            }
            .code(),
            ConsentError::TtlOutOfRange {
                requested: 0,
                min: 1,
                max: 2,
            }
            .code(),
            ConsentError::Missing.code(),
            ConsentError::Expired.code(),
            ConsentError::Revoked.code(),
            ConsentError::ScopeDenied(TradeAction::Buy).code(),
            ConsentError::AlreadyActive.code(),
            ConsentError::StoreUnavailable("db".into()).code(),
        ];
        let mut unique = codes.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), codes.len());
    }
}
