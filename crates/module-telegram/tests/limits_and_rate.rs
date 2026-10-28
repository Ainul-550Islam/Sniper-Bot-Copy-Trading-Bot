//! BATCH 04 — cap arithmetic and the per-chat rate limiter.
//!
//! The per-order cap, the rolling 24h cap (including its exact boundary) and
//! the token bucket (with refill, the durable counter half, and per-chat /
//! per-tenant isolation) are all pinned here with integer expectations.

use chrono::{DateTime, Utc};

use module_telegram::commands::limits::{
    applicable_caps, check_daily_cap, check_order_cap, remaining_daily_cap, rolling_window_start,
    LimitDeny, RateLimitConfig, RateLimiter, SlippageBounds, TokenBucket, TradeLimits,
    ROLLING_CAP_WINDOW_SECS,
};
use module_telegram::consent::policy::{
    draft_consent, ConsentBounds, ConsentRequest, GrantContext, CONSENT_PHRASE,
};
use module_telegram::{OrgRiskLimits, TradeAction};

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(secs, 0).unwrap()
}

fn consent(order_cap: u64, daily_cap: u64) -> module_telegram::Consent {
    draft_consent(
        &ConsentBounds::default(),
        &ConsentRequest {
            order_lamports: Some(order_cap),
            daily_lamports: Some(daily_cap),
            ..Default::default()
        },
        &GrantContext {
            organization_id: "org-1",
            user_id: 111,
            chat_id: 100,
            now: at(0),
            org_limits: OrgRiskLimits::new(u64::MAX, u64::MAX),
            phrase: CONSENT_PHRASE,
        },
    )
    .expect("consent")
}

#[test]
fn per_order_cap_is_inclusive_and_hard() {
    assert!(check_order_cap(0, 0).is_ok());
    assert!(check_order_cap(50_000_000, 50_000_000).is_ok());
    assert_eq!(
        check_order_cap(50_000_001, 50_000_000),
        Err(LimitDeny::OrderCapExceeded {
            amount_lamports: 50_000_001,
            cap_lamports: 50_000_000
        })
    );
    assert_eq!(
        check_order_cap(u64::MAX, u64::MAX - 1).unwrap_err().code(),
        "order_cap_exceeded"
    );
}

#[test]
fn rolling_cap_boundary_is_exact() {
    let cap = 200_000_000u64;
    assert!(check_daily_cap(199_999_999, 1, cap).is_ok());
    assert_eq!(
        check_daily_cap(199_999_999, 2, cap),
        Err(LimitDeny::DailyCapExceeded {
            spent_lamports: 199_999_999,
            amount_lamports: 2,
            cap_lamports: cap
        })
    );
    assert!(check_daily_cap(200_000_000, 0, cap).is_ok());
    assert!(check_daily_cap(200_000_000, 1, cap).is_err());
    // Overflow denies instead of wrapping.
    assert_eq!(
        check_daily_cap(u64::MAX, u64::MAX, u64::MAX),
        Err(LimitDeny::CapOverflow)
    );
    assert_eq!(remaining_daily_cap(50, 200), 150);
    assert_eq!(remaining_daily_cap(250, 200), 0);
}

#[test]
fn the_window_is_rolling_24h() {
    assert_eq!(ROLLING_CAP_WINDOW_SECS, 86_400);
    let now = at(1_700_000_000);
    assert_eq!(
        rolling_window_start(now, ROLLING_CAP_WINDOW_SECS),
        at(1_700_000_000 - 86_400)
    );
    // A spend at window_start - 1s is outside; at window_start it is inside.
    let spent = rolling_window_start(now, ROLLING_CAP_WINDOW_SECS);
    assert!(check_daily_cap(0, 1, 1).is_ok());
    assert!(spent < now);
}

#[test]
fn applicable_caps_never_exceed_either_side() {
    let consent = consent(50_000_000, 200_000_000);
    assert_eq!(
        applicable_caps(&consent, &OrgRiskLimits::new(1_000_000_000, 5_000_000_000)),
        (50_000_000, 200_000_000)
    );
    assert_eq!(
        applicable_caps(&consent, &OrgRiskLimits::new(10_000_000, 20_000_000)),
        (10_000_000, 20_000_000)
    );
    assert_eq!(
        applicable_caps(&consent, &OrgRiskLimits::new(0, 0)),
        (0, 0),
        "a zero org limit blocks every order"
    );
}

#[test]
fn token_bucket_refills_at_the_configured_rate() {
    let cfg = RateLimitConfig {
        capacity: 4,
        window_secs: 200,
    };
    let mut bucket = TokenBucket::full(&cfg, at(0));
    assert_eq!(bucket.millitokens(), 4_000);
    for _ in 0..4 {
        assert!(bucket.try_consume(&cfg, at(0)));
    }
    assert!(!bucket.try_consume(&cfg, at(0)));
    // 4 tokens / 200s = 20 millitokens/s ⇒ one token after 50s.
    assert!(!bucket.try_consume(&cfg, at(49)));
    assert!(bucket.try_consume(&cfg, at(50)));
    assert!(!bucket.try_consume(&cfg, at(50)));
    // A full window restores the bucket, never beyond capacity.
    assert!(bucket.try_consume(&cfg, at(1_000)));
    bucket.refill(&cfg, at(2_000));
    assert_eq!(bucket.millitokens(), 4_000);
}

#[test]
fn limiter_is_per_chat_and_per_tenant() {
    let limiter = RateLimiter::new(RateLimitConfig {
        capacity: 1,
        window_secs: 60,
    })
    .expect("limiter");
    assert!(limiter.check("org-1", 100, at(0), 0).is_ok());
    assert_eq!(
        limiter.check("org-1", 100, at(0), 0),
        Err(LimitDeny::RateLimited {
            capacity: 1,
            window_secs: 60
        })
    );
    assert!(limiter.check("org-1", 101, at(0), 0).is_ok());
    assert!(limiter.check("org-2", 100, at(0), 0).is_ok());
    // The refill is time-based, not call-based.
    assert!(limiter.check("org-1", 100, at(60), 0).is_ok());
}

#[test]
fn the_durable_counter_caps_even_a_cold_bucket() {
    let limiter = RateLimiter::new(RateLimitConfig {
        capacity: 3,
        window_secs: 60,
    })
    .expect("limiter");
    // Simulates a process restart: the in-memory bucket is empty of history,
    // but the ledger says the chat already used 3 slots.
    assert!(limiter.check("org-1", 100, at(0), 3).is_err());
    assert!(limiter.check("org-1", 100, at(0), 2).is_ok());
    assert!(limiter.check("org-1", 100, at(0), 3).is_err());
}

#[test]
fn invalid_configuration_is_refused_at_construction() {
    assert!(RateLimiter::new(RateLimitConfig {
        capacity: 0,
        window_secs: 60
    })
    .is_err());
    assert!(RateLimiter::new(RateLimitConfig {
        capacity: 1,
        window_secs: 0
    })
    .is_err());
    assert!(SlippageBounds {
        min_bps: 0,
        max_bps: 1
    }
    .validate()
    .is_err());
    assert!(SlippageBounds {
        min_bps: 10,
        max_bps: 9
    }
    .validate()
    .is_err());
    assert!(TradeLimits::default().validate().is_ok());
}

#[test]
fn sell_caps_use_the_same_consent_numbers() {
    // A sell carries a percentage, so only the per-order cap and the rate
    // limit apply; the SOL daily cap is irrelevant to it (documented in
    // docs/TELEGRAM-TRADING.md).
    let consent = consent(50_000_000, 200_000_000);
    let (order_cap, daily_cap) =
        applicable_caps(&consent, &OrgRiskLimits::new(60_000_000, 400_000_000));
    assert_eq!(order_cap, 50_000_000);
    assert_eq!(daily_cap, 200_000_000);
    // A 100% sell is valid input; scope (not amount) decides it.
    assert!(module_telegram::commands::sell::is_full_exit(10_000));
    assert!(consent.scope.allows(TradeAction::Sell));
}
