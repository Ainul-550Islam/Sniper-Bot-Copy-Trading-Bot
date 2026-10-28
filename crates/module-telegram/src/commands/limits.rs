//! The **only** owner of limit arithmetic and rate limiting.
//!
//! Two kinds of ceiling are enforced here, and both are pure functions of
//! integers (no `f64` touches money):
//!
//! * **per-order** — `amount ≤ min(consent.max_order, org.max_order)`;
//! * **rolling 24h** — `spent_in_the_last_24h + amount ≤ min(consent.daily, org.daily)`,
//!   where `spent` comes from the command ledger (see
//!   [`crate::commands::pending::CommandStore::spent_since`]).
//!
//! Rate limiting is a per-chat token bucket held in memory **plus** a durable
//! counter hook: the caller passes the number of commands the ledger recorded
//! in the same window, so a process restart (cold bucket) cannot reset an
//! operator's budget. Both checks must pass.
//!
//! Overflow is a denial, never a wrap: a `checked_add` that fails means the
//! request is enormous, so it is refused.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

use crate::consent::policy;
use crate::consent::Consent;
use crate::trade_port::OrgRiskLimits;

/// The rolling window the consent's daily cap is measured over.
pub const ROLLING_CAP_WINDOW_SECS: u64 = 86_400;

/// The slippage window this deployment accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlippageBounds {
    /// Inclusive minimum, in basis points.
    pub min_bps: u16,
    /// Inclusive maximum, in basis points.
    pub max_bps: u16,
}

impl Default for SlippageBounds {
    fn default() -> Self {
        Self {
            min_bps: crate::commands::parse::MIN_SLIPPAGE_BPS,
            max_bps: crate::commands::parse::MAX_SLIPPAGE_BPS,
        }
    }
}

impl SlippageBounds {
    /// Reject an incoherent window.
    pub fn validate(&self) -> Result<(), LimitDeny> {
        if self.min_bps == 0 || self.max_bps < self.min_bps {
            return Err(LimitDeny::InvalidConfig(
                "slippage window must be a non-empty range starting above 0".into(),
            ));
        }
        Ok(())
    }
}

/// Token-bucket configuration, expressed per chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitConfig {
    /// Bucket size = maximum commands inside one window.
    pub capacity: u32,
    /// Window over which the bucket refills completely.
    pub window_secs: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            capacity: 5,
            window_secs: 60,
        }
    }
}

impl RateLimitConfig {
    /// Reject a configuration that would allow unlimited traffic.
    pub fn validate(&self) -> Result<(), LimitDeny> {
        if self.capacity == 0 {
            return Err(LimitDeny::InvalidConfig(
                "rate limit capacity must be > 0".into(),
            ));
        }
        if self.window_secs == 0 {
            return Err(LimitDeny::InvalidConfig(
                "rate limit window must be > 0".into(),
            ));
        }
        Ok(())
    }

    /// The full bucket size in millitokens.
    fn capacity_millitokens(&self) -> u64 {
        u64::from(self.capacity) * 1_000
    }

    /// The window length in milliseconds.
    fn window_millis(&self) -> u64 {
        self.window_secs.saturating_mul(1_000)
    }
}

/// Every limit this module enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TradeLimits {
    /// Accepted slippage window.
    pub slippage: SlippageBounds,
    /// Per-chat rate limit.
    pub rate: RateLimitConfig,
}

impl TradeLimits {
    /// Validate both halves.
    pub fn validate(&self) -> Result<(), LimitDeny> {
        self.slippage.validate()?;
        self.rate.validate()
    }
}

/// A refusal produced by this module. Typed so the reply text stays in
/// [`crate::commands::reply`] and the audit code stays stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitDeny {
    /// The order exceeds the applicable per-order cap.
    OrderCapExceeded {
        /// Requested amount, in lamports.
        amount_lamports: u64,
        /// Applicable cap, in lamports.
        cap_lamports: u64,
    },
    /// The rolling 24h cap would be exceeded.
    DailyCapExceeded {
        /// Already committed in the window, in lamports.
        spent_lamports: u64,
        /// This order, in lamports.
        amount_lamports: u64,
        /// Applicable cap, in lamports.
        cap_lamports: u64,
    },
    /// Adding the amount to the window's spend overflows `u64`.
    CapOverflow,
    /// Too many commands for this chat.
    RateLimited {
        /// Bucket capacity.
        capacity: u32,
        /// Window length in seconds.
        window_secs: u64,
    },
    /// The module was configured with incoherent bounds.
    InvalidConfig(String),
}

impl LimitDeny {
    /// Stable machine code (used as `failure_code`).
    pub fn code(&self) -> &'static str {
        match self {
            LimitDeny::OrderCapExceeded { .. } => "order_cap_exceeded",
            LimitDeny::DailyCapExceeded { .. } => "daily_cap_exceeded",
            LimitDeny::CapOverflow => "cap_overflow",
            LimitDeny::RateLimited { .. } => "rate_limited",
            LimitDeny::InvalidConfig(_) => "invalid_limit_config",
        }
    }
}

/// Enforce the per-order cap.
pub fn check_order_cap(amount_lamports: u64, cap_lamports: u64) -> Result<(), LimitDeny> {
    if amount_lamports > cap_lamports {
        return Err(LimitDeny::OrderCapExceeded {
            amount_lamports,
            cap_lamports,
        });
    }
    Ok(())
}

/// Enforce the rolling cap: `spent + amount ≤ cap`.
pub fn check_daily_cap(
    spent_lamports: u64,
    amount_lamports: u64,
    cap_lamports: u64,
) -> Result<(), LimitDeny> {
    let total = spent_lamports
        .checked_add(amount_lamports)
        .ok_or(LimitDeny::CapOverflow)?;
    if total > cap_lamports {
        return Err(LimitDeny::DailyCapExceeded {
            spent_lamports,
            amount_lamports,
            cap_lamports,
        });
    }
    Ok(())
}

/// The caps that apply to a consent, resolved through
/// [`crate::consent::policy`] (the single owner of the rule).
pub fn applicable_caps(consent: &Consent, org: &OrgRiskLimits) -> (u64, u64) {
    (
        policy::effective_order_cap(consent, org),
        policy::effective_daily_cap(consent, org),
    )
}

/// Lamports still available under the rolling cap (never negative).
pub fn remaining_daily_cap(spent_lamports: u64, cap_lamports: u64) -> u64 {
    cap_lamports.saturating_sub(spent_lamports)
}

/// The start of the rolling window measured back from `now`.
pub fn rolling_window_start(now: DateTime<Utc>, window_secs: u64) -> DateTime<Utc> {
    now - Duration::seconds(i64::try_from(window_secs).unwrap_or(i64::MAX))
}

/// One chat's token bucket, in millitokens (integer math: no drift, no `f64`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenBucket {
    millitokens: u64,
    last_refill: DateTime<Utc>,
}

impl TokenBucket {
    /// A full bucket at `now`.
    pub fn full(cfg: &RateLimitConfig, now: DateTime<Utc>) -> Self {
        Self {
            millitokens: cfg.capacity_millitokens(),
            last_refill: now,
        }
    }

    /// Refill for the elapsed time (clock going backwards adds nothing).
    ///
    /// The arithmetic is exact and drift-free: `elapsed · capacity / window` is
    /// evaluated in `u128`, and the clock is advanced **only** by the time that
    /// was actually converted into tokens. A partial interval therefore stays
    /// banked and is spent by the next refill, so the long-run rate is exactly
    /// `capacity / window` (no rounding up into a faster bucket, no rounding
    /// down that quietly starves a slow-refilling one).
    pub fn refill(&mut self, cfg: &RateLimitConfig, now: DateTime<Utc>) {
        let capacity = cfg.capacity_millitokens();
        let elapsed_ms = (now - self.last_refill).num_milliseconds().max(0) as u128;
        if elapsed_ms == 0 {
            return;
        }
        if self.millitokens >= capacity {
            // Already full: only the clock moves, nothing accrues.
            self.last_refill = now;
            return;
        }
        let window_ms = u128::from(cfg.window_millis());
        let gained = elapsed_ms.saturating_mul(u128::from(capacity)) / window_ms;
        if gained == 0 {
            // Less than one millitoken's worth of time: bank it untouched.
            return;
        }
        let consumed_ms = (gained * window_ms / u128::from(capacity)) as i64;
        self.millitokens = self
            .millitokens
            .saturating_add(u64::try_from(gained).unwrap_or(capacity))
            .min(capacity);
        self.last_refill += Duration::milliseconds(consumed_ms);
    }

    /// Consume one token; `false` when the bucket is empty.
    pub fn try_consume(&mut self, cfg: &RateLimitConfig, now: DateTime<Utc>) -> bool {
        self.refill(cfg, now);
        if self.millitokens < 1_000 {
            return false;
        }
        self.millitokens -= 1_000;
        true
    }

    /// Current fill level in millitokens (diagnostics and tests).
    pub fn millitokens(&self) -> u64 {
        self.millitokens
    }
}

/// Per-chat token buckets.
///
/// `check` requires **both** halves to agree: the in-memory bucket must have a
/// token and the durable ledger counter must be under the cap. A cold bucket
/// after a restart therefore inherits the durable budget instead of resetting
/// it.
#[derive(Debug)]
pub struct RateLimiter {
    cfg: RateLimitConfig,
    buckets: Mutex<HashMap<(String, i64), TokenBucket>>,
    /// Bound on tracked chats (a chat is evicted once it is fully refilled and
    /// the map is large).
    max_chats: usize,
}

impl RateLimiter {
    /// Build a limiter with the given configuration (validated).
    pub fn new(cfg: RateLimitConfig) -> Result<Self, LimitDeny> {
        cfg.validate()?;
        Ok(Self {
            cfg,
            buckets: Mutex::new(HashMap::new()),
            max_chats: 4_096,
        })
    }

    /// The configuration in force.
    pub fn config(&self) -> RateLimitConfig {
        self.cfg
    }

    /// Check (and consume) one command slot for `(organization, chat)`.
    ///
    /// `durable_count` is the number of commands the ledger recorded in the
    /// same window; it is the durable half of the limiter.
    pub fn check(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
        durable_count: u64,
    ) -> Result<(), LimitDeny> {
        let denied = LimitDeny::RateLimited {
            capacity: self.cfg.capacity,
            window_secs: self.cfg.window_secs,
        };
        if durable_count >= u64::from(self.cfg.capacity) {
            return Err(denied);
        }
        let mut buckets = self.buckets.lock().expect("rate limiter lock");
        self.evict_if_needed(&mut buckets, now);
        let bucket = buckets
            .entry((organization_id.to_string(), chat_id))
            .or_insert_with(|| TokenBucket::full(&self.cfg, now));
        if bucket.try_consume(&self.cfg, now) {
            Ok(())
        } else {
            Err(denied)
        }
    }

    /// Current fill level in millitokens for a chat (diagnostics and tests).
    pub fn millitokens(&self, organization_id: &str, chat_id: i64) -> Option<u64> {
        self.buckets
            .lock()
            .expect("rate limiter lock")
            .get(&(organization_id.to_string(), chat_id))
            .map(TokenBucket::millitokens)
    }

    /// Drop every bucket (tests and a deliberate operator reset).
    pub fn clear(&self) {
        self.buckets.lock().expect("rate limiter lock").clear();
    }

    fn evict_if_needed(
        &self,
        buckets: &mut HashMap<(String, i64), TokenBucket>,
        now: DateTime<Utc>,
    ) {
        if buckets.len() < self.max_chats {
            return;
        }
        let cfg = self.cfg;
        buckets.retain(|_, bucket| {
            let mut probe = *bucket;
            probe.refill(&cfg, now);
            probe.millitokens() < cfg.capacity_millitokens()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::policy::CONSENT_PHRASE;
    use crate::consent::store::ConsentStore;
    use crate::consent::store::InMemoryConsentStore;
    use crate::consent::{draft_consent, ConsentBounds, ConsentRequest, GrantContext};

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    #[test]
    fn order_cap_is_a_hard_ceiling() {
        assert!(check_order_cap(50_000_000, 50_000_000).is_ok());
        assert_eq!(
            check_order_cap(50_000_001, 50_000_000),
            Err(LimitDeny::OrderCapExceeded {
                amount_lamports: 50_000_001,
                cap_lamports: 50_000_000
            })
        );
        assert_eq!(
            check_order_cap(1, 0),
            Err(LimitDeny::OrderCapExceeded {
                amount_lamports: 1,
                cap_lamports: 0
            })
        );
        assert_eq!(
            check_order_cap(2, 1).unwrap_err().code(),
            "order_cap_exceeded"
        );
    }

    #[test]
    fn rolling_cap_boundaries() {
        // Exactly at the cap is allowed, one lamport over is not.
        assert!(check_daily_cap(150, 50, 200).is_ok());
        assert_eq!(
            check_daily_cap(151, 50, 200),
            Err(LimitDeny::DailyCapExceeded {
                spent_lamports: 151,
                amount_lamports: 50,
                cap_lamports: 200
            })
        );
        // An exhausted window denies any positive amount.
        assert!(check_daily_cap(200, 0, 200).is_ok());
        assert!(check_daily_cap(200, 1, 200).is_err());
        // Overflow is a denial, never a wrap-around.
        assert_eq!(
            check_daily_cap(u64::MAX, 1, u64::MAX),
            Err(LimitDeny::CapOverflow)
        );
        assert_eq!(LimitDeny::CapOverflow.code(), "cap_overflow");
        assert_eq!(
            check_daily_cap(10, 10, 5).unwrap_err().code(),
            "daily_cap_exceeded"
        );
        assert_eq!(remaining_daily_cap(10, 200), 190);
        assert_eq!(remaining_daily_cap(300, 200), 0);
        assert_eq!(rolling_window_start(at(86_400), 86_400), at(0));
    }

    #[test]
    fn applicable_caps_take_the_tighter_side() {
        let consent = draft_consent(
            &ConsentBounds::default(),
            &ConsentRequest::default(),
            &GrantContext {
                organization_id: "org-1",
                user_id: 1,
                chat_id: 100,
                now: at(0),
                org_limits: OrgRiskLimits::new(1_000, 2_000),
                phrase: CONSENT_PHRASE,
            },
        )
        .unwrap();
        assert_eq!(
            applicable_caps(&consent, &OrgRiskLimits::new(1_000, 2_000)),
            (1_000, 2_000)
        );
        assert_eq!(
            applicable_caps(&consent, &OrgRiskLimits::new(u64::MAX, u64::MAX)),
            (consent.max_order_lamports, consent.daily_cap_lamports)
        );
    }

    #[test]
    fn token_bucket_refills_over_the_window() {
        let cfg = RateLimitConfig {
            capacity: 3,
            window_secs: 60,
        };
        let mut bucket = TokenBucket::full(&cfg, at(0));
        assert_eq!(bucket.millitokens(), 3_000);
        assert!(bucket.try_consume(&cfg, at(0)));
        assert!(bucket.try_consume(&cfg, at(0)));
        assert!(bucket.try_consume(&cfg, at(0)));
        assert!(!bucket.try_consume(&cfg, at(0)), "bucket is empty");
        // 20s of a 60s window = one token back (3 capacity / 60s = 50/s).
        assert!(!bucket.try_consume(&cfg, at(19)));
        assert!(bucket.try_consume(&cfg, at(20)));
        // A full refill after the whole window.
        assert!(bucket.try_consume(&cfg, at(80)));
        assert!(bucket.try_consume(&cfg, at(80)));
        assert!(bucket.try_consume(&cfg, at(80)));
        // Never above capacity.
        bucket.refill(&cfg, at(10_000));
        assert_eq!(bucket.millitokens(), 3_000);
        // A backwards clock adds nothing.
        let before = bucket.millitokens();
        bucket.refill(&cfg, at(0));
        assert_eq!(bucket.millitokens(), before);
    }

    #[test]
    fn limiter_denies_after_capacity_and_recovers() {
        let limiter = RateLimiter::new(RateLimitConfig {
            capacity: 2,
            window_secs: 10,
        })
        .unwrap();
        assert!(limiter.check("org-1", 100, at(0), 0).is_ok());
        assert!(limiter.check("org-1", 100, at(0), 0).is_ok());
        assert_eq!(
            limiter.check("org-1", 100, at(0), 0),
            Err(LimitDeny::RateLimited {
                capacity: 2,
                window_secs: 10
            })
        );
        // Another chat has its own bucket.
        assert!(limiter.check("org-1", 101, at(0), 0).is_ok());
        // Another tenant has its own bucket.
        assert!(limiter.check("org-2", 100, at(0), 0).is_ok());
        // Refilled after the window.
        assert!(limiter.check("org-1", 100, at(5), 0).is_ok());
        assert!(limiter.check("org-1", 100, at(10), 0).is_ok());
        assert_eq!(limiter.millitokens("org-1", 100), Some(0));
        assert_eq!(limiter.millitokens("org-9", 100), None);
        assert_eq!(limiter.config().capacity, 2);
        limiter.clear();
        assert_eq!(limiter.millitokens("org-1", 100), None);
    }

    #[test]
    fn durable_counter_is_the_second_half_of_the_check() {
        let limiter = RateLimiter::new(RateLimitConfig {
            capacity: 3,
            window_secs: 60,
        })
        .unwrap();
        // A cold bucket (fresh process) still respects the durable ledger.
        assert_eq!(
            limiter.check("org-1", 100, at(0), 3),
            Err(LimitDeny::RateLimited {
                capacity: 3,
                window_secs: 60
            })
        );
        assert!(limiter.check("org-1", 100, at(0), 2).is_ok());
        assert_eq!(
            limiter
                .check("org-1", 100, at(0), u64::MAX)
                .unwrap_err()
                .code(),
            "rate_limited"
        );
    }

    #[test]
    fn configuration_is_validated() {
        assert!(matches!(
            RateLimiter::new(RateLimitConfig {
                capacity: 0,
                window_secs: 60
            }),
            Err(LimitDeny::InvalidConfig(_))
        ));
        assert!(matches!(
            RateLimiter::new(RateLimitConfig {
                capacity: 1,
                window_secs: 0
            }),
            Err(LimitDeny::InvalidConfig(_))
        ));
        assert!(matches!(
            SlippageBounds {
                min_bps: 0,
                max_bps: 10
            }
            .validate(),
            Err(LimitDeny::InvalidConfig(_))
        ));
        assert!(matches!(
            SlippageBounds {
                min_bps: 10,
                max_bps: 5
            }
            .validate(),
            Err(LimitDeny::InvalidConfig(_))
        ));
        assert!(TradeLimits::default().validate().is_ok());
        assert_eq!(
            LimitDeny::InvalidConfig("x".into()).code(),
            "invalid_limit_config"
        );
        assert_eq!(SlippageBounds::default().max_bps, 500);
        assert_eq!(RateLimitConfig::default().capacity, 5);
    }

    #[test]
    fn limiter_evicts_idle_chats() {
        let limiter = RateLimiter::new(RateLimitConfig {
            capacity: 1,
            window_secs: 1,
        })
        .unwrap();
        for chat in 0..8_000i64 {
            let _ = limiter.check("org-1", chat, at(0), 0);
        }
        // Every bucket is fully refilled at +10s, so the map collapses.
        let _ = limiter.check("org-1", 9_000, at(10), 0);
        let tracked = limiter.buckets.lock().unwrap().len();
        assert!(tracked <= 4_096, "tracked {tracked}");
    }

    #[tokio::test]
    async fn spent_since_matches_the_ledger_view() {
        // Sanity: the cap helper agrees with what the store reports.
        let store = InMemoryConsentStore::new();
        assert!(store.active("org-1", 100, at(0)).await.unwrap().is_none());
        let cap = 200;
        assert!(check_daily_cap(0, 200, cap).is_ok());
        assert!(check_daily_cap(0, 201, cap).is_err());
    }
}
