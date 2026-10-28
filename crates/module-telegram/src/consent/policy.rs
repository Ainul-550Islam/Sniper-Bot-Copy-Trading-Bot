//! The **only** owner of consent rules: the confirmation phrase, the TTL
//! bounds, the cap defaults, and the rule that a consent can never widen an
//! organization's own risk limits.
//!
//! Nothing else in this crate decides whether a consent is usable — callers
//! ask [`require_active`], [`draft_consent`], [`effective_order_cap`] or
//! [`effective_daily_cap`] and act on the typed answer.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::consent::model::{Consent, ConsentError, ConsentScope};
use crate::trade_port::{OrgRiskLimits, TradeAction};

/// The phrase an operator must echo verbatim to grant a consent.
///
/// It exists so that a compromised chat cannot silently hand trading rights to
/// a bot: granting requires a deliberate, typed, multi-word acknowledgement
/// that no inline button or replay can produce.
pub const CONSENT_PHRASE: &str = "I CONSENT TO TELEGRAM TRADING";

/// Normalize a candidate phrase: trim, collapse inner whitespace, lowercase.
pub fn normalize_phrase(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// `true` when `candidate` is the confirmation phrase (case/space insensitive).
pub fn phrase_matches(candidate: &str) -> bool {
    normalize_phrase(candidate) == normalize_phrase(CONSENT_PHRASE)
}

/// SHA-256 of the normalized phrase — the only form that is ever persisted.
pub fn phrase_sha256(candidate: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(normalize_phrase(candidate).as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Hard bounds every consent is checked against.
///
/// The default is deliberately conservative: 0.05 SOL per order, 0.2 SOL per
/// rolling 24h, valid for one day. An operator may raise the defaults up to
/// the absolute ceilings below — and never past the organization's own risk
/// limits, which are queried from the executing side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsentBounds {
    /// Per-order cap applied when the operator omits one, in lamports.
    pub default_order_lamports: u64,
    /// Rolling 24h cap applied when the operator omits one, in lamports.
    pub default_daily_lamports: u64,
    /// Absolute per-order ceiling this deployment will ever grant, in lamports.
    pub max_order_lamports: u64,
    /// Absolute rolling 24h ceiling this deployment will ever grant.
    pub max_daily_lamports: u64,
    /// TTL applied when the operator omits one.
    pub default_ttl_hours: u32,
    /// Shortest TTL an operator may request.
    pub min_ttl_hours: u32,
    /// Longest TTL an operator may request.
    pub max_ttl_hours: u32,
}

impl Default for ConsentBounds {
    fn default() -> Self {
        Self {
            default_order_lamports: 50_000_000,  // 0.05 SOL
            default_daily_lamports: 200_000_000, // 0.20 SOL
            max_order_lamports: 5_000_000_000,   // 5 SOL
            max_daily_lamports: 20_000_000_000,  // 20 SOL
            default_ttl_hours: 24,
            min_ttl_hours: 1,
            max_ttl_hours: 720, // 30 days
        }
    }
}

impl ConsentBounds {
    /// Reject a self-inconsistent configuration (config hygiene; fail closed).
    pub fn validate(&self) -> Result<(), ConsentError> {
        if self.default_order_lamports == 0
            || self.max_order_lamports == 0
            || self.default_daily_lamports == 0
            || self.max_daily_lamports == 0
        {
            return Err(ConsentError::OrderCapZero);
        }
        if self.default_daily_lamports < self.default_order_lamports
            || self.max_daily_lamports < self.max_order_lamports
        {
            return Err(ConsentError::DailyCapBelowOrder);
        }
        if self.min_ttl_hours == 0
            || self.max_ttl_hours < self.min_ttl_hours
            || self.default_ttl_hours < self.min_ttl_hours
            || self.default_ttl_hours > self.max_ttl_hours
        {
            return Err(ConsentError::TtlOutOfRange {
                requested: self.default_ttl_hours,
                min: self.min_ttl_hours,
                max: self.max_ttl_hours,
            });
        }
        Ok(())
    }
}

/// What the operator asked for on the `/consent grant` line (omitted fields
/// fall back to [`ConsentBounds`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConsentRequest {
    /// Per-order cap in lamports.
    pub order_lamports: Option<u64>,
    /// Rolling 24h cap in lamports.
    pub daily_lamports: Option<u64>,
    /// Lifetime in hours.
    pub ttl_hours: Option<u32>,
}

/// Everything a grant needs besides the request itself.
#[derive(Debug, Clone)]
pub struct GrantContext<'a> {
    /// Tenant the chat is linked to.
    pub organization_id: &'a str,
    /// Telegram user granting the consent.
    pub user_id: i64,
    /// Telegram chat the consent binds to.
    pub chat_id: i64,
    /// Grant timestamp.
    pub now: DateTime<Utc>,
    /// The organization's own ceilings (from the executing side).
    pub org_limits: OrgRiskLimits,
    /// The phrase the operator typed, exactly as received.
    pub phrase: &'a str,
}

/// Build a validated consent row, or explain precisely why it cannot exist.
///
/// Rules, in order (each is a hard rejection, never a silent clamp):
/// 1. the typed phrase must match [`CONSENT_PHRASE`];
/// 2. the per-order cap must be > 0 and ≤ `min(bounds.max_order, org.max_order)`;
/// 3. the daily cap must be ≤ `min(bounds.max_daily, org.daily_cap)` and
///    ≥ the per-order cap;
/// 4. the TTL must be within `[min_ttl_hours, max_ttl_hours]`.
pub fn draft_consent(
    bounds: &ConsentBounds,
    request: &ConsentRequest,
    ctx: &GrantContext<'_>,
) -> Result<Consent, ConsentError> {
    bounds.validate()?;
    if !phrase_matches(ctx.phrase) {
        return Err(ConsentError::PhraseMismatch);
    }

    let order_ceiling = bounds
        .max_order_lamports
        .min(ctx.org_limits.max_order_lamports);
    let order = request
        .order_lamports
        .unwrap_or_else(|| bounds.default_order_lamports.min(order_ceiling));
    if order == 0 {
        return Err(ConsentError::OrderCapZero);
    }
    if order > order_ceiling {
        return Err(ConsentError::ExceedsOrgOrderLimit {
            requested: order,
            ceiling: order_ceiling,
        });
    }

    let daily_ceiling = bounds
        .max_daily_lamports
        .min(ctx.org_limits.daily_cap_lamports);
    let daily = request
        .daily_lamports
        .unwrap_or_else(|| bounds.default_daily_lamports.min(daily_ceiling));
    if daily > daily_ceiling {
        return Err(ConsentError::ExceedsOrgDailyLimit {
            requested: daily,
            ceiling: daily_ceiling,
        });
    }
    if daily < order {
        return Err(ConsentError::DailyCapBelowOrder);
    }

    let ttl = request.ttl_hours.unwrap_or(bounds.default_ttl_hours);
    if ttl < bounds.min_ttl_hours || ttl > bounds.max_ttl_hours {
        return Err(ConsentError::TtlOutOfRange {
            requested: ttl,
            min: bounds.min_ttl_hours,
            max: bounds.max_ttl_hours,
        });
    }

    let expires_at = ctx.now + Duration::hours(i64::from(ttl));
    Ok(Consent {
        consent_id: Uuid::new_v4(),
        organization_id: ctx.organization_id.to_string(),
        user_id: ctx.user_id,
        chat_id: ctx.chat_id,
        scope: ConsentScope::both(),
        max_order_lamports: order,
        daily_cap_lamports: daily,
        granted_at: ctx.now,
        expires_at,
        revoked_at: None,
        phrase_sha256: phrase_sha256(ctx.phrase),
    })
}

/// The consent that authorises `action` at `now`, or the typed reason it does
/// not exist.
pub fn require_active(
    consent: Option<&Consent>,
    action: TradeAction,
    now: DateTime<Utc>,
) -> Result<&Consent, ConsentError> {
    let consent = consent.ok_or(ConsentError::Missing)?;
    match consent.status(now) {
        crate::consent::model::ConsentStatus::Active => {}
        crate::consent::model::ConsentStatus::Expired => return Err(ConsentError::Expired),
        crate::consent::model::ConsentStatus::Revoked => return Err(ConsentError::Revoked),
    }
    if !consent.scope.allows(action) {
        return Err(ConsentError::ScopeDenied(action));
    }
    Ok(consent)
}

/// The per-order cap that actually applies: `min(consent, org)`.
///
/// A consent can only ever tighten the organization's ceiling; if either side
/// is zero the answer is zero and every order is refused downstream.
pub fn effective_order_cap(consent: &Consent, org: &OrgRiskLimits) -> u64 {
    consent.max_order_lamports.min(org.max_order_lamports)
}

/// The rolling 24h cap that actually applies: `min(consent, org)`.
pub fn effective_daily_cap(consent: &Consent, org: &OrgRiskLimits) -> u64 {
    consent.daily_cap_lamports.min(org.daily_cap_lamports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn org() -> OrgRiskLimits {
        OrgRiskLimits::new(1_000_000_000, 4_000_000_000)
    }

    fn ctx<'a>(phrase: &'a str) -> GrantContext<'a> {
        GrantContext {
            organization_id: "org-1",
            user_id: 42,
            chat_id: 100,
            now: at(1_000),
            org_limits: org(),
            phrase,
        }
    }

    #[test]
    fn phrase_is_case_and_space_insensitive() {
        assert!(phrase_matches("I CONSENT TO TELEGRAM TRADING"));
        assert!(phrase_matches("  i consent   to telegram trading "));
        assert!(!phrase_matches("i consent"));
        assert!(!phrase_matches(""));
        assert!(!phrase_matches("I CONSENT TO TELEGRAM TRADING!"));
        assert_eq!(normalize_phrase("  A  B "), "a b");
    }

    #[test]
    fn phrase_hash_is_stable_and_phrase_independent() {
        let a = phrase_sha256("I CONSENT TO TELEGRAM TRADING");
        let b = phrase_sha256("i  consent to telegram trading");
        assert_eq!(a, b, "normalization must precede hashing");
        assert_ne!(a, phrase_sha256("something else"));
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn defaults_are_conservative_and_lower_than_the_org_ceiling() {
        let c = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest::default(),
            &ctx(CONSENT_PHRASE),
        )
        .expect("grant");
        assert_eq!(c.max_order_lamports, 50_000_000);
        assert_eq!(c.daily_cap_lamports, 200_000_000);
        assert_eq!(c.expires_at, at(1_000) + Duration::hours(24));
        assert_eq!(c.scope, ConsentScope::both());
        assert_eq!(c.phrase_sha256, phrase_sha256(CONSENT_PHRASE));
        assert_eq!(c.organization_id, "org-1");
        assert_eq!(c.user_id, 42);
        assert_eq!(c.chat_id, 100);
    }

    #[test]
    fn defaults_shrink_to_a_tighter_org_limit() {
        let tight = OrgRiskLimits::new(10_000_000, 20_000_000);
        let mut c = ctx(CONSENT_PHRASE);
        c.org_limits = tight;
        let consent =
            draft_consent(&ConsentBounds::default(), &ConsentRequest::default(), &c).unwrap();
        assert_eq!(consent.max_order_lamports, 10_000_000);
        assert_eq!(consent.daily_cap_lamports, 20_000_000);
    }

    #[test]
    fn wrong_phrase_is_rejected() {
        let err = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest::default(),
            &ctx("yes"),
        )
        .unwrap_err();
        assert_eq!(err, ConsentError::PhraseMismatch);
        assert_eq!(err.code(), "phrase_mismatch");
    }

    #[test]
    fn caps_never_exceed_org_limits_and_are_never_clamped() {
        let err = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest {
                order_lamports: Some(2_000_000_000),
                ..Default::default()
            },
            &ctx(CONSENT_PHRASE),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ConsentError::ExceedsOrgOrderLimit {
                requested: 2_000_000_000,
                ceiling: 1_000_000_000,
            }
        );

        let err = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest {
                daily_lamports: Some(5_000_000_000),
                ..Default::default()
            },
            &ctx(CONSENT_PHRASE),
        )
        .unwrap_err();
        assert_eq!(
            err,
            ConsentError::ExceedsOrgDailyLimit {
                requested: 5_000_000_000,
                ceiling: 4_000_000_000,
            }
        );

        // The deployment's own absolute ceiling binds even for a generous org.
        let mut generous = ctx(CONSENT_PHRASE);
        generous.org_limits = OrgRiskLimits::new(u64::MAX, u64::MAX);
        let err = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest {
                order_lamports: Some(6_000_000_000),
                ..Default::default()
            },
            &generous,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ConsentError::ExceedsOrgOrderLimit {
                requested: 6_000_000_000,
                ceiling: 5_000_000_000,
            }
        );
    }

    #[test]
    fn daily_below_order_and_zero_order_are_rejected() {
        assert_eq!(
            draft_consent(
                &ConsentBounds::default(),
                &ConsentRequest {
                    order_lamports: Some(60_000_000),
                    daily_lamports: Some(55_000_000),
                    ..Default::default()
                },
                &ctx(CONSENT_PHRASE),
            )
            .unwrap_err(),
            ConsentError::DailyCapBelowOrder
        );
        assert_eq!(
            draft_consent(
                &ConsentBounds::default(),
                &ConsentRequest {
                    order_lamports: Some(0),
                    ..Default::default()
                },
                &ctx(CONSENT_PHRASE),
            )
            .unwrap_err(),
            ConsentError::OrderCapZero
        );
    }

    #[test]
    fn ttl_bounds_are_enforced() {
        for (ttl, ok) in [
            (0u32, false),
            (1, true),
            (24, true),
            (720, true),
            (721, false),
        ] {
            let got = draft_consent(
                &ConsentBounds::default(),
                &ConsentRequest {
                    ttl_hours: Some(ttl),
                    ..Default::default()
                },
                &ctx(CONSENT_PHRASE),
            );
            assert_eq!(got.is_ok(), ok, "ttl {ttl}");
            if !ok {
                assert!(matches!(
                    got.unwrap_err(),
                    ConsentError::TtlOutOfRange { .. }
                ));
            }
        }
    }

    #[test]
    fn bounds_validation_rejects_incoherent_config() {
        let b = ConsentBounds {
            max_order_lamports: 0,
            ..ConsentBounds::default()
        };
        assert_eq!(b.validate().unwrap_err(), ConsentError::OrderCapZero);

        let b = ConsentBounds {
            max_daily_lamports: ConsentBounds::default().max_order_lamports / 2,
            ..ConsentBounds::default()
        };
        assert_eq!(b.validate().unwrap_err(), ConsentError::DailyCapBelowOrder);

        let b = ConsentBounds {
            default_ttl_hours: 0,
            ..ConsentBounds::default()
        };
        assert!(matches!(
            b.validate().unwrap_err(),
            ConsentError::TtlOutOfRange { .. }
        ));
        assert!(ConsentBounds::default().validate().is_ok());
    }

    #[test]
    fn require_active_covers_every_refusal() {
        let consent = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest::default(),
            &ctx(CONSENT_PHRASE),
        )
        .unwrap();
        assert!(require_active(Some(&consent), TradeAction::Buy, at(2_000)).is_ok());
        assert_eq!(
            require_active(None, TradeAction::Buy, at(2_000)).unwrap_err(),
            ConsentError::Missing
        );
        assert_eq!(
            require_active(Some(&consent), TradeAction::Buy, at(1_000 + 86_400)).unwrap_err(),
            ConsentError::Expired
        );
        let mut revoked = consent.clone();
        revoked.revoked_at = Some(at(1_500));
        assert_eq!(
            require_active(Some(&revoked), TradeAction::Buy, at(2_000)).unwrap_err(),
            ConsentError::Revoked
        );
        let mut buy_only = consent.clone();
        buy_only.scope = ConsentScope::only(TradeAction::Buy);
        assert_eq!(
            require_active(Some(&buy_only), TradeAction::Sell, at(2_000)).unwrap_err(),
            ConsentError::ScopeDenied(TradeAction::Sell)
        );
    }

    #[test]
    fn effective_caps_take_the_tighter_side() {
        let consent = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest::default(),
            &ctx(CONSENT_PHRASE),
        )
        .unwrap();
        assert_eq!(
            effective_order_cap(&consent, &org()),
            consent.max_order_lamports
        );
        assert_eq!(
            effective_daily_cap(&consent, &org()),
            consent.daily_cap_lamports
        );
        let tight = OrgRiskLimits::new(1_000, 2_000);
        assert_eq!(effective_order_cap(&consent, &tight), 1_000);
        assert_eq!(effective_daily_cap(&consent, &tight), 2_000);
    }
}
