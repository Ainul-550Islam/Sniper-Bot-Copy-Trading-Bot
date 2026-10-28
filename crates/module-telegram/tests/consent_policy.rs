//! BATCH 04 — consent rules (`crate::consent::policy`).
//!
//! The policy is the single owner of "may this consent exist / may it be used",
//! so these tests cover the whole rule surface: active/expired/revoked, scope,
//! caps against the organization's risk limits, TTL bounds and the typed
//! confirmation phrase.

use chrono::{DateTime, Duration, Utc};

use module_telegram::consent::policy::{
    draft_consent, effective_daily_cap, effective_order_cap, normalize_phrase, phrase_matches,
    phrase_sha256, require_active, ConsentBounds, ConsentRequest, GrantContext, CONSENT_PHRASE,
};
use module_telegram::consent::ConsentStore;
use module_telegram::consent::{ConsentError, ConsentScope, ConsentStatus, InMemoryConsentStore};
use module_telegram::{OrgRiskLimits, TradeAction};

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

fn org() -> OrgRiskLimits {
    OrgRiskLimits::new(2_000_000_000, 10_000_000_000)
}

fn ctx(phrase: &str) -> GrantContext<'_> {
    GrantContext {
        organization_id: "org-1",
        user_id: 111,
        chat_id: 100,
        now: at(1_000_000),
        org_limits: org(),
        phrase,
    }
}

fn grant(request: ConsentRequest) -> module_telegram::Consent {
    draft_consent(&ConsentBounds::default(), &request, &ctx(CONSENT_PHRASE)).expect("grant")
}

#[test]
fn the_phrase_must_be_echoed_verbatim() {
    assert!(phrase_matches(CONSENT_PHRASE));
    assert!(phrase_matches("i  consent to telegram   trading"));
    assert!(!phrase_matches("I consent"));
    assert!(!phrase_matches(""));
    assert!(!phrase_matches("I CONSENT TO TELEGRAM TRADING please"));
    assert_eq!(normalize_phrase("  A \n B "), "a b");
    assert_eq!(
        phrase_sha256("I consent to telegram trading"),
        phrase_sha256(CONSENT_PHRASE)
    );
    assert_ne!(phrase_sha256("nope"), phrase_sha256(CONSENT_PHRASE));
}

#[test]
fn a_wrong_phrase_never_creates_a_consent() {
    let err = draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest::default(),
        &ctx("yes please"),
    )
    .unwrap_err();
    assert_eq!(err, ConsentError::PhraseMismatch);
    assert_eq!(err.code(), "phrase_mismatch");
}

#[test]
fn defaults_are_conservative_and_bounded_by_the_org() {
    let consent = grant(ConsentRequest::default());
    assert_eq!(consent.max_order_lamports, 50_000_000); // 0.05 SOL
    assert_eq!(consent.daily_cap_lamports, 200_000_000); // 0.20 SOL
    assert_eq!(consent.expires_at, at(1_000_000) + Duration::hours(24));
    assert_eq!(consent.scope, ConsentScope::both());
    assert_eq!(consent.phrase_sha256, phrase_sha256(CONSENT_PHRASE));
    assert_eq!(consent.status(at(1_000_001)), ConsentStatus::Active);

    // A tight organization shrinks the default instead of being bypassed.
    let tight = OrgRiskLimits::new(5_000_000, 8_000_000);
    let mut context = ctx(CONSENT_PHRASE);
    context.org_limits = tight;
    let consent = draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest::default(),
        &context,
    )
    .expect("grant");
    assert_eq!(consent.max_order_lamports, 5_000_000);
    assert_eq!(consent.daily_cap_lamports, 8_000_000);
}

#[test]
fn caps_can_never_exceed_the_organization_limits() {
    let err = draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest {
            order_lamports: Some(3_000_000_000),
            ..Default::default()
        },
        &ctx(CONSENT_PHRASE),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ConsentError::ExceedsOrgOrderLimit {
            requested: 3_000_000_000,
            ceiling: 2_000_000_000
        }
    );

    let err = draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest {
            daily_lamports: Some(20_000_000_000),
            ..Default::default()
        },
        &ctx(CONSENT_PHRASE),
    )
    .unwrap_err();
    assert_eq!(
        err,
        ConsentError::ExceedsOrgDailyLimit {
            requested: 20_000_000_000,
            ceiling: 10_000_000_000
        }
    );

    // The deployment's absolute ceiling binds even for an unlimited org.
    let mut unlimited = ctx(CONSENT_PHRASE);
    unlimited.org_limits = OrgRiskLimits::new(u64::MAX, u64::MAX);
    let err = draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest {
            order_lamports: Some(6_000_000_000),
            ..Default::default()
        },
        &unlimited,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ConsentError::ExceedsOrgOrderLimit {
            requested: 6_000_000_000,
            ceiling: 5_000_000_000
        }
    );
}

#[test]
fn daily_cap_must_cover_one_order_and_be_positive() {
    assert_eq!(
        draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest {
                order_lamports: Some(100_000_000),
                daily_lamports: Some(90_000_000),
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
fn ttl_bounds() {
    let bounds = ConsentBounds::default();
    for ttl in [0u32, 1, 12, 720, 721, 100_000] {
        let result = draft_consent(
            &bounds,
            &ConsentRequest {
                ttl_hours: Some(ttl),
                ..Default::default()
            },
            &ctx(CONSENT_PHRASE),
        );
        let inside = (1..=720).contains(&ttl);
        assert_eq!(result.is_ok(), inside, "ttl {ttl}");
        if inside {
            let consent = result.unwrap();
            assert_eq!(
                consent.expires_at - consent.granted_at,
                Duration::hours(i64::from(ttl))
            );
        }
    }
}

#[test]
fn active_expired_revoked_and_scope() {
    let consent = grant(ConsentRequest::default());
    assert!(require_active(Some(&consent), TradeAction::Buy, at(1_000_100)).is_ok());
    assert!(require_active(Some(&consent), TradeAction::Sell, at(1_000_100)).is_ok());

    assert_eq!(
        require_active(None, TradeAction::Buy, at(1_000_100)).unwrap_err(),
        ConsentError::Missing
    );
    assert_eq!(
        require_active(Some(&consent), TradeAction::Buy, at(1_000_000 + 86_400)).unwrap_err(),
        ConsentError::Expired
    );

    let mut revoked = consent.clone();
    revoked.revoked_at = Some(at(1_000_050));
    assert_eq!(
        require_active(Some(&revoked), TradeAction::Buy, at(1_000_100)).unwrap_err(),
        ConsentError::Revoked
    );
    // Revocation wins even after the expiry date.
    assert_eq!(
        require_active(Some(&revoked), TradeAction::Buy, at(9_999_999)).unwrap_err(),
        ConsentError::Revoked
    );

    let mut buy_only = consent.clone();
    buy_only.scope = ConsentScope::only(TradeAction::Buy);
    assert!(require_active(Some(&buy_only), TradeAction::Buy, at(1_000_100)).is_ok());
    assert_eq!(
        require_active(Some(&buy_only), TradeAction::Sell, at(1_000_100)).unwrap_err(),
        ConsentError::ScopeDenied(TradeAction::Sell)
    );
}

#[test]
fn effective_caps_are_the_tighter_of_the_two() {
    let consent = grant(ConsentRequest::default());
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
    let zero = OrgRiskLimits::new(0, 0);
    assert_eq!(effective_order_cap(&consent, &zero), 0);
    assert_eq!(effective_daily_cap(&consent, &zero), 0);
}

#[tokio::test]
async fn the_store_keeps_at_most_one_live_consent() {
    let store = InMemoryConsentStore::new();
    let first = grant(ConsentRequest::default());
    store.insert(&first).await.expect("first grant");
    let second = draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest::default(),
        &GrantContext {
            organization_id: "org-1",
            user_id: 111,
            chat_id: 100,
            now: at(1_000_100),
            org_limits: org(),
            phrase: CONSENT_PHRASE,
        },
    )
    .expect("second draft");
    let err = store.insert(&second).await.unwrap_err();
    assert_eq!(err.code(), "store_conflict");
    // Revoking makes room for a new one, immediately.
    assert!(store
        .revoke("org-1", 100, at(1_000_200))
        .await
        .unwrap()
        .is_some());
    store.insert(&second).await.expect("regrant after revoke");
    assert_eq!(store.all().len(), 2);
}

#[test]
fn bounds_validation_rejects_incoherent_configuration() {
    let bounds = ConsentBounds {
        max_order_lamports: 0,
        ..ConsentBounds::default()
    };
    assert_eq!(bounds.validate(), Err(ConsentError::OrderCapZero));
    let bounds = ConsentBounds {
        default_ttl_hours: 0,
        ..ConsentBounds::default()
    };
    assert!(matches!(
        bounds.validate(),
        Err(ConsentError::TtlOutOfRange { .. })
    ));
    assert!(ConsentBounds::default().validate().is_ok());
}
