//! Redis coordination store (BUILD PLAN §4-iii).
//!
//! Scope — SHORT-LIVED coordination only, per the durability rule:
//!   * dedup windows (L2 behind the in-memory L1)
//!   * distributed locks (leader election between replicas)
//!   * rate-limit counters
//!   * ephemeral caches
//!
//! **Durable financial state (orders, positions, trades, audit) NEVER lives
//! in Redis** — that is PostgreSQL's job ([`crate::db`]). If Redis dies, the
//! worst outcome is a duplicated dedup check falling back to L1/Postgres or
//! a lock being re-acquired after its TTL; no money state is lost.
//!
//! Every operation is timeout-bounded and metered (`bot_redis_operations_*`)
//! so a stalled Redis shows up as degraded health, not a hung bot.

use std::time::Duration;

use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client, Script};
use tracing::{debug, info, warn};

use crate::config::RedisConfig;
use crate::error::{BotError, BotResult};
use crate::obs::metrics;

/// Compare-and-delete lock release (atomic in Redis).
static RELEASE_LOCK: once_cell::sync::Lazy<Script> = once_cell::sync::Lazy::new(|| {
    Script::new(
        r#"if redis.call("GET", KEYS[1]) == ARGV[1] then
               return redis.call("DEL", KEYS[1])
           else
               return 0
           end"#,
    )
});

#[derive(Clone)]
pub struct RedisKv {
    manager: ConnectionManager,
    op_timeout: Duration,
}

impl RedisKv {
    /// Connect (ConnectionManager auto-reconnects with backoff afterwards).
    pub async fn connect(cfg: &RedisConfig, url: &str) -> BotResult<Self> {
        let client =
            Client::open(url).map_err(|e| BotError::db(format!("invalid redis url: {e}")))?;
        let manager = tokio::time::timeout(
            Duration::from_millis(cfg.connect_timeout_ms.max(100)),
            ConnectionManager::new(client),
        )
        .await
        .map_err(|_| BotError::Timeout("redis connect timed out".into()))?
        .map_err(|e| BotError::db(format!("redis connect failed: {e}")))?;
        let kv = RedisKv {
            manager,
            op_timeout: Duration::from_millis(cfg.operation_timeout_ms.max(50)),
        };
        kv.ping().await?;
        info!("redis connected");
        Ok(kv)
    }

    pub async fn ping(&self) -> BotResult<()> {
        self.op("ping", |mut c| async move {
            let _: String = redis::cmd("PING").query_async(&mut c).await?;
            Ok(())
        })
        .await
    }

    /// `SET key val NX PX ttl` — true when THIS call created the key
    /// (first arrival). The dedup L2 primitive.
    pub async fn set_nx_ttl(&self, key: &str, val: &str, ttl: Duration) -> BotResult<bool> {
        let ttl_ms = ttl.as_millis().max(1) as u64;
        let key = key.to_string();
        let val = val.to_string();
        self.op("set_nx", move |mut c| async move {
            let set: Option<String> = redis::cmd("SET")
                .arg(&key)
                .arg(&val)
                .arg("NX")
                .arg("PX")
                .arg(ttl_ms)
                .query_async(&mut c)
                .await?;
            Ok(set.is_some())
        })
        .await
    }

    pub async fn get(&self, key: &str) -> BotResult<Option<String>> {
        let key = key.to_string();
        self.op("get", move |mut c| async move {
            let v: Option<String> = c.get(&key).await?;
            Ok(v)
        })
        .await
    }

    pub async fn del(&self, key: &str) -> BotResult<()> {
        let key = key.to_string();
        self.op("del", move |mut c| async move {
            let _: i64 = c.del(&key).await?;
            Ok(())
        })
        .await
    }

    /// INCR + EXPIRE atomically-ish (pipeline). Returns the new counter
    /// value. Used by the rate limiter and alert throttles.
    pub async fn incr_expire(&self, key: &str, ttl: Duration) -> BotResult<u64> {
        let key = key.to_string();
        let secs = ttl.as_secs().max(1) as i64;
        self.op("incr", move |mut c| async move {
            // The pipeline returns one reply PER command: (INCR result,
            // EXPIRE result). Destructuring only the first was a latent
            // type error ("Array response of wrong dimension") that no
            // environment had ever executed until now.
            let (n, _expired): (u64, i64) = redis::pipe()
                .atomic()
                .incr(&key, 1u64)
                .expire(&key, secs)
                .query_async(&mut c)
                .await?;
            Ok(n)
        })
        .await
    }

    /// Try to take a distributed lock: `SET key token NX PX ttl`.
    /// Holders must refresh or finish before the TTL; release with
    /// [`RedisKv::release_lock`] using the same token.
    pub async fn acquire_lock(&self, key: &str, token: &str, ttl: Duration) -> BotResult<bool> {
        self.set_nx_ttl(key, token, ttl).await
    }

    /// Release ONLY if we still hold it (compare-and-delete via Lua).
    pub async fn release_lock(&self, key: &str, token: &str) -> BotResult<bool> {
        let key = key.to_string();
        let token = token.to_string();
        self.op("release_lock", move |mut c| async move {
            let deleted: i64 = RELEASE_LOCK
                .key(&key)
                .arg(&token)
                .invoke_async(&mut c)
                .await?;
            Ok(deleted == 1)
        })
        .await
    }

    /// Remaining TTL in seconds (-2 missing / -1 no expiry), for diagnostics.
    pub async fn ttl_secs(&self, key: &str) -> BotResult<i64> {
        let key = key.to_string();
        self.op("ttl", move |mut c| async move {
            let t: i64 = c.ttl(&key).await?;
            Ok(t)
        })
        .await
    }

    /// Uniform wrapper: clone the connection, bound the operation, meter it.
    /// Crate-visible so [`crate::redis_ownership`] reuses the same timeout +
    /// metering discipline for its Lua scripts.
    pub(crate) async fn op<T, F, Fut>(&self, name: &'static str, f: F) -> BotResult<T>
    where
        F: FnOnce(ConnectionManager) -> Fut,
        Fut: std::future::Future<Output = redis::RedisResult<T>>,
    {
        let reg = metrics::global();
        let started = std::time::Instant::now();
        let conn = self.manager.clone();
        let result = tokio::time::timeout(self.op_timeout, f(conn)).await;
        match result {
            Ok(Ok(v)) => {
                reg.histogram(
                    "bot_redis_operation_duration_ms",
                    "Redis operation latency in milliseconds.",
                    &[],
                    metrics::LATENCY_BUCKETS_MS,
                )
                .observe(started.elapsed().as_millis() as u64);
                reg.counter(
                    "bot_redis_operations_total",
                    "Redis operations by op and outcome.",
                    &[("op", name), ("outcome", "ok")],
                )
                .inc();
                Ok(v)
            }
            Ok(Err(e)) => {
                reg.counter(
                    "bot_redis_operations_total",
                    "Redis operations by op and outcome.",
                    &[("op", name), ("outcome", "error")],
                )
                .inc();
                debug!(op = name, error = %e, "redis operation failed");
                Err(BotError::db(format!("redis {name}: {e}")))
            }
            Err(_) => {
                reg.counter(
                    "bot_redis_operations_total",
                    "Redis operations by op and outcome.",
                    &[("op", name), ("outcome", "timeout")],
                )
                .inc();
                warn!(
                    op = name,
                    timeout_ms = self.op_timeout.as_millis() as u64,
                    "redis operation timed out"
                );
                Err(BotError::Timeout(format!("redis {name} timed out")))
            }
        }
    }
}

/// Resolve the Redis URL from the configured env var.
pub fn resolve_url(cfg: &RedisConfig) -> Option<String> {
    std::env::var(&cfg.url_env)
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// Startup helper mirroring [`crate::db::open`]: disabled → `Ok(None)`;
/// `required = true` → failures are fatal; otherwise degrade loudly.
pub async fn open(cfg: &RedisConfig) -> BotResult<Option<RedisKv>> {
    if !cfg.enabled {
        debug!("redis disabled by config");
        return Ok(None);
    }
    let Some(url) = resolve_url(cfg) else {
        let msg = format!("redis enabled but env var {} is not set", cfg.url_env);
        if cfg.required {
            return Err(BotError::config(msg));
        }
        warn!(%msg, "continuing without redis");
        return Ok(None);
    };
    match RedisKv::connect(cfg, &url).await {
        Ok(kv) => Ok(Some(kv)),
        Err(e) => {
            if cfg.required {
                return Err(e);
            }
            warn!(error = %e, "redis unavailable — continuing without it");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// URL resolution honours the configured env-var name and treats
    /// empty/whitespace as absent.
    #[test]
    fn resolve_url_reads_configured_env_var() {
        let cfg = RedisConfig {
            url_env: "TEST_REDIS_URL_RESOLVE".into(),
            ..Default::default()
        };
        assert!(resolve_url(&cfg).is_none());
        std::env::set_var("TEST_REDIS_URL_RESOLVE", "   ");
        assert!(resolve_url(&cfg).is_none());
        std::env::set_var("TEST_REDIS_URL_RESOLVE", "redis://127.0.0.1:6379");
        assert_eq!(resolve_url(&cfg).as_deref(), Some("redis://127.0.0.1:6379"));
        std::env::remove_var("TEST_REDIS_URL_RESOLVE");
    }

    /// Disabled config never connects (no env, no network) and open() is a
    /// pure no-op.
    #[tokio::test]
    async fn open_disabled_is_none() {
        let cfg = RedisConfig::default(); // enabled = false
        assert!(open(&cfg).await.unwrap().is_none());
    }

    /// Enabled-but-unset env with required = false degrades to None.
    #[tokio::test]
    async fn open_missing_url_degrades_unless_required() {
        let mut cfg = RedisConfig {
            enabled: true,
            url_env: "TEST_REDIS_URL_DEFINITELY_UNSET_XYZ".into(),
            ..Default::default()
        };
        assert!(open(&cfg).await.unwrap().is_none());
        cfg.required = true;
        assert!(open(&cfg).await.is_err());
    }
}
