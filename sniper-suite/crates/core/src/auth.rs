//! Role-based access control + rate limiting for the control plane
//! (BUILD PLAN §4-xi).
//!
//! * Keys are configured as **env-var references** (`[[auth.keys]]`); the
//!   plaintext is read once at startup and thereafter only ever handled as
//!   a SHA-256 hash. Nothing logs or serialises the plaintext.
//! * The legacy single key (`[api].api_key_env` / `secrets.api_key`) keeps
//!   working and maps to the **owner** role, so existing deployments are
//!   unaffected.
//! * Roles: `owner` (everything, incl. key rotation and live-mode changes)
//!   ⊃ `operator` (all runtime controls: kill/resume/mode/modules) ⊃
//!   `readonly` (authenticated reads only).
//! * The rate limiter is a per-principal token bucket (falls back to a
//!   per-IP bucket for unauthenticated traffic). Hand-rolled on purpose:
//!   deterministic, zero-dependency, testable without a clock mock.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::info;

use crate::config::Config;

/// Access role, strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Readonly = 0,
    Operator = 1,
    Owner = 2,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Readonly => "readonly",
            Role::Operator => "operator",
            Role::Owner => "owner",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "readonly" | "read" | "viewer" => Role::Readonly,
            "operator" | "op" => Role::Operator,
            "owner" | "admin" => Role::Owner,
            _ => return None,
        })
    }

    /// True when `self` is at least `required`.
    pub fn satisfies(&self, required: Role) -> bool {
        (*self as u8) >= (required as u8)
    }
}

/// An authenticated caller. `key_hash` is SHA-256 hex — safe to log.
#[derive(Debug, Clone, Serialize)]
pub struct Principal {
    pub label: String,
    pub role: Role,
    pub key_hash: String,
}

/// Hash a plaintext key (SHA-256 hex). Used for lookups, storage and logs.
pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// Key registry + role checks. Clone is cheap (inner `Arc`).
#[derive(Clone)]
pub struct Authenticator {
    keys: Arc<RwLock<HashMap<String, Principal>>>,
}

impl Authenticator {
    /// An empty registry (keys added at runtime via [`Authenticator::add_key`]).
    pub fn empty() -> Self {
        Authenticator {
            keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Build from config: `[[auth.keys]]` entries (key read from each
    /// `key_env`) plus the legacy `[api]` key mapped to owner.
    /// Keys whose env var is unset are skipped with a warning.
    pub fn from_config(cfg: &Config) -> Self {
        let mut map = HashMap::new();

        for entry in &cfg.auth.keys {
            let plaintext = match std::env::var(&entry.key_env) {
                Ok(v) if !v.trim().is_empty() => v,
                _ => {
                    tracing::warn!(
                        label = %entry.label,
                        env = %entry.key_env,
                        "auth key env var not set — key disabled"
                    );
                    continue;
                }
            };
            let Some(role) = Role::parse(&entry.role) else {
                tracing::warn!(label = %entry.label, role = %entry.role, "invalid auth role — key disabled");
                continue;
            };
            let hash = sha256_hex(&plaintext);
            map.insert(
                hash.clone(),
                Principal {
                    label: entry.label.clone(),
                    role,
                    key_hash: hash,
                },
            );
        }

        // Legacy single key => owner (backward compatible).
        if let Some(legacy) = crate::config::resolve_api_key(cfg) {
            let hash = sha256_hex(&legacy);
            map.entry(hash.clone()).or_insert_with(|| Principal {
                label: "legacy-api-key".into(),
                role: Role::Owner,
                key_hash: hash,
            });
        }

        info!(keys = map.len(), "authenticator initialised");
        Authenticator {
            keys: Arc::new(RwLock::new(map)),
        }
    }

    /// Number of registered keys (dashboard/diagnostics).
    pub async fn len(&self) -> usize {
        self.keys.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Look up a presented plaintext key. Constant work per attempt (hash +
    /// map lookup); the plaintext itself is never stored.
    pub async fn authenticate(&self, presented: &str) -> Option<Principal> {
        if presented.trim().is_empty() {
            return None;
        }
        let hash = sha256_hex(presented);
        self.keys.read().await.get(&hash).cloned()
    }

    /// Role gate.
    pub fn authorize(principal: &Principal, required: Role) -> bool {
        principal.role.satisfies(required)
    }

    /// Runtime key rotation: add or replace a key. Returns the hash (safe to
    /// log / store in the `api_keys` table).
    pub async fn add_key(&self, label: &str, plaintext: &str, role: Role) -> String {
        let hash = sha256_hex(plaintext);
        self.keys.write().await.insert(
            hash.clone(),
            Principal {
                label: label.to_string(),
                role,
                key_hash: hash.clone(),
            },
        );
        info!(label, role = role.as_str(), "auth key added");
        hash
    }

    /// Revoke by hash. Returns true when a key was removed.
    pub async fn revoke_hash(&self, hash: &str) -> bool {
        let removed = self.keys.write().await.remove(hash).is_some();
        if removed {
            info!(%hash, "auth key revoked");
        }
        removed
    }

    /// Public view: labels + roles + hashes only (never plaintext).
    pub async fn list(&self) -> Vec<Principal> {
        let mut v: Vec<Principal> = self.keys.read().await.values().cloned().collect();
        v.sort_by(|a, b| a.label.cmp(&b.label));
        v
    }
}

/// One token bucket.
#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    updated: Instant,
    last_seen: Instant,
}

/// Per-principal (or per-IP) token-bucket rate limiter.
///
/// `rpm` requests per minute with burst = rpm (a fresh bucket is full).
/// Refill is continuous. Zero-cost when `rpm == 0` (limiting disabled).
pub struct RateLimiter {
    rpm: u32,
    buckets: RwLock<HashMap<String, Bucket>>,
    max_buckets: usize,
}

/// Verdict of a rate-limit check.
#[derive(Debug, Clone, PartialEq)]
pub enum RateVerdict {
    Allowed,
    /// Denied; retry after this many seconds (rounded up).
    Limited {
        retry_after_secs: u64,
    },
}

impl RateLimiter {
    pub fn new(rpm: u32) -> Arc<Self> {
        Arc::new(RateLimiter {
            rpm,
            buckets: RwLock::new(HashMap::new()),
            max_buckets: 10_000,
        })
    }

    pub fn rpm(&self) -> u32 {
        self.rpm
    }

    /// Consume one token for `key` (principal hash or client IP).
    pub async fn check(&self, key: &str) -> RateVerdict {
        if self.rpm == 0 {
            return RateVerdict::Allowed;
        }
        let capacity = self.rpm as f64;
        let refill_per_sec = capacity / 60.0;
        let now = Instant::now();

        let mut buckets = self.buckets.write().await;
        // Opportunistic sweep: keep the map bounded even under churn.
        if buckets.len() > self.max_buckets {
            buckets.retain(|_, b| now.duration_since(b.last_seen) < Duration::from_secs(600));
        }
        let bucket = buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: capacity,
            updated: now,
            last_seen: now,
        });
        let elapsed = now.duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_per_sec).min(capacity);
        bucket.updated = now;
        bucket.last_seen = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            RateVerdict::Allowed
        } else {
            let need = 1.0 - bucket.tokens;
            RateVerdict::Limited {
                retry_after_secs: (need / refill_per_sec).ceil().max(1.0) as u64,
            }
        }
    }

    /// Current bucket count (gauge/diagnostics).
    pub async fn tracked(&self) -> usize {
        self.buckets.read().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(role: Role) -> Principal {
        Principal {
            label: "t".into(),
            role,
            key_hash: "h".into(),
        }
    }

    #[test]
    fn role_ordering_and_parsing() {
        assert!(Role::Owner.satisfies(Role::Operator));
        assert!(Role::Operator.satisfies(Role::Readonly));
        assert!(!Role::Readonly.satisfies(Role::Operator));
        assert!(!Role::Operator.satisfies(Role::Owner));
        assert_eq!(Role::parse("ADMIN"), Some(Role::Owner));
        assert_eq!(Role::parse("op"), Some(Role::Operator));
        assert_eq!(Role::parse("viewer"), Some(Role::Readonly));
        assert_eq!(Role::parse("root"), None);
    }

    #[test]
    fn authorize_enforces_the_role_hierarchy() {
        assert!(Authenticator::authorize(
            &principal(Role::Owner),
            Role::Owner
        ));
        assert!(Authenticator::authorize(
            &principal(Role::Owner),
            Role::Operator
        ));
        assert!(Authenticator::authorize(
            &principal(Role::Operator),
            Role::Operator
        ));
        assert!(!Authenticator::authorize(
            &principal(Role::Operator),
            Role::Owner
        ));
        assert!(!Authenticator::authorize(
            &principal(Role::Readonly),
            Role::Operator
        ));
        assert!(Authenticator::authorize(
            &principal(Role::Readonly),
            Role::Readonly
        ));
    }

    #[test]
    fn hash_is_sha256_hex_and_stable() {
        let h = sha256_hex("secret");
        assert_eq!(h.len(), 64);
        assert_eq!(h, sha256_hex("secret"));
        assert_ne!(h, sha256_hex("secret2"));
        // Known SHA-256 vector.
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn authenticate_only_matches_exact_keys() {
        let a = Authenticator::empty();
        let hash = a.add_key("ops", "topsecret", Role::Operator).await;
        let p = a.authenticate("topsecret").await.unwrap();
        assert_eq!(p.role, Role::Operator);
        assert_eq!(p.key_hash, hash);
        assert!(a.authenticate("topsecre").await.is_none());
        assert!(a.authenticate("").await.is_none());
        assert!(a.authenticate("  ").await.is_none());
        // List never exposes plaintext.
        let listed = a.list().await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].label, "ops");
    }

    #[tokio::test]
    async fn revoke_removes_access() {
        let a = Authenticator::empty();
        let hash = a.add_key("tmp", "k", Role::Readonly).await;
        assert!(a.authenticate("k").await.is_some());
        assert!(a.revoke_hash(&hash).await);
        assert!(a.authenticate("k").await.is_none());
        assert!(!a.revoke_hash(&hash).await);
    }

    #[tokio::test]
    async fn rate_limiter_allows_burst_then_limits() {
        let rl = RateLimiter::new(60); // 1 token/sec refill, burst 60
        for _ in 0..60 {
            assert_eq!(rl.check("alice").await, RateVerdict::Allowed);
        }
        match rl.check("alice").await {
            RateVerdict::Limited { retry_after_secs } => assert!(retry_after_secs >= 1),
            other => panic!("expected Limited, got {other:?}"),
        }
        // A different principal has its own bucket.
        assert_eq!(rl.check("bob").await, RateVerdict::Allowed);
    }

    #[tokio::test]
    async fn rate_limiter_refills_over_time() {
        let rl = RateLimiter::new(6000); // 100 tokens/sec
        for _ in 0..6000 {
            assert_eq!(rl.check("x").await, RateVerdict::Allowed);
        }
        // The refill is continuous against the wall clock, so under machine
        // load the burst loop itself can span enough real time to trickle a
        // few tokens back in. Drain with a bounded loop until the bucket
        // reports Limited — deterministic because a tight check loop consumes
        // ~1000 tokens per refill-token at this rate, so exhaustion is
        // guaranteed well inside the bound.
        let mut verdict = rl.check("x").await;
        for _ in 0..1000 {
            if matches!(verdict, RateVerdict::Limited { .. }) {
                break;
            }
            verdict = rl.check("x").await;
        }
        assert!(matches!(verdict, RateVerdict::Limited { .. }));
        tokio::time::sleep(Duration::from_millis(60)).await; // ~6 tokens
        assert_eq!(rl.check("x").await, RateVerdict::Allowed);
    }

    #[tokio::test]
    async fn rate_limiter_zero_rpm_disables_limiting() {
        let rl = RateLimiter::new(0);
        for _ in 0..1000 {
            assert_eq!(rl.check("y").await, RateVerdict::Allowed);
        }
        assert_eq!(rl.tracked().await, 0, "disabled limiter tracks nothing");
    }
}
