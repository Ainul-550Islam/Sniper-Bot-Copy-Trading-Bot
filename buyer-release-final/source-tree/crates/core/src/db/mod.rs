//! PostgreSQL persistence (BUILD PLAN §4-iii).
//!
//! One [`Database`] handle wraps an `sqlx` `PgPool` and is shared by every
//! repository in [`repo`]. Design rules:
//!
//! * **Optional by default.** `[database].enabled = false` keeps the exact
//!   pre-database behaviour (in-memory state + JSONL journal). Nothing in the
//!   app may *require* the database unless `[database].required = true`.
//! * **Fail fast, degrade loud.** Every operation has a client-side timeout;
//!   connection/statement timeouts are configured at the pool. Outages are
//!   classified (`DbOutcome`) and metered, never silently swallowed.
//! * **Retry-safe writes.** All repository writes are upserts
//!   (`ON CONFLICT … DO UPDATE`) or guarded inserts, so a retried operation
//!   after an ambiguous failure cannot duplicate state.
//! * **Migrations are embedded** (`sqlx::migrate!`) and versioned in
//!   `crates/core/migrations/`; `auto_migrate` runs them on connect.
//!
//! Secrets: the connection URL contains credentials and is only ever read
//! from the env var named by `database.url_env`; it is never logged (errors
//! are mapped to messages without the URL).

pub mod claims;
pub mod repo;

use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use tracing::{debug, info, warn};

use crate::config::DatabaseConfig;
use crate::error::{BotError, BotResult};
use crate::obs::metrics;

/// Embed every `crates/core/migrations/*.sql` at compile time.
pub const MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// A connected PostgreSQL handle plus the policy that governs it.
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
    query_timeout: Duration,
}

impl Database {
    /// Connect using `[database]` config. `url` is the resolved connection
    /// string (read from the configured env var by the caller).
    pub async fn connect(cfg: &DatabaseConfig, url: &str) -> BotResult<Self> {
        let options: PgConnectOptions = url
            .parse::<PgConnectOptions>()
            .map_err(|e| BotError::db(format!("invalid database url: {e}")))?
            // Server-side statement timeout: nothing may run away.
            .options([("statement_timeout", &cfg.statement_timeout_ms.to_string())])
            .application_name("sniper-suite");

        let pool = PgPoolOptions::new()
            .max_connections(cfg.max_connections)
            .min_connections(cfg.min_connections)
            .acquire_timeout(Duration::from_millis(cfg.acquire_timeout_ms))
            .connect_with(options)
            .await
            .map_err(|e| BotError::db(format!("database connect failed: {e}")))?;

        let db = Database {
            pool,
            query_timeout: Duration::from_millis(cfg.query_timeout_ms.max(100)),
        };

        // Prove the connection works before declaring success.
        db.ping().await?;
        info!(max_connections = cfg.max_connections, "postgres connected");
        Ok(db)
    }

    /// Run the embedded migrations (idempotent). Not routed through `timed`
    /// (MigrateError is not a sqlx::Error); migrations may legitimately take
    /// longer than a query timeout, so no client-side cap is applied.
    pub async fn migrate(&self) -> BotResult<()> {
        MIGRATOR
            .run(&self.pool)
            .await
            .map_err(|e| BotError::db(format!("migrations failed: {e}")))?;
        info!("database migrations applied");
        Ok(())
    }

    /// Cheap liveness probe (`SELECT 1`).
    pub async fn ping(&self) -> BotResult<()> {
        self.timed("ping", sqlx::query("SELECT 1").execute(&self.pool))
            .await
            .map_err(|e| BotError::db(format!("ping failed: {e}")))?;
        Ok(())
    }

    /// Pool telemetry for the state sampler.
    pub async fn pool_stats(&self) -> PoolStats {
        // `size()` = current pool size; idle/active come from the internals
        // exposed via size + the connection manager's counters.
        let size = self.pool.size();
        let idle = self.pool.num_idle() as u32;
        PoolStats {
            size,
            idle,
            active: size.saturating_sub(idle),
            max: self.pool.options().get_max_connections(),
        }
    }

    /// Number of applied migrations (operational visibility).
    pub async fn migration_count(&self) -> BotResult<i64> {
        let row = self
            .timed(
                "migration_count",
                sqlx::query("SELECT COUNT(*) AS n FROM _sqlx_migrations").fetch_one(self.pool()),
            )
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        Ok(row.get::<i64, _>("n"))
    }

    /// Close the pool, waiting for in-flight statements. Called during the
    /// graceful-shutdown persistence phase.
    pub async fn close(&self) {
        self.pool.close().await;
        info!("postgres pool closed");
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Run a future under the client-side query timeout, metering the
    /// outcome. Every repository call funnels through here so timeouts and
    /// failure classification are uniform.
    pub(crate) async fn timed<T>(
        &self,
        op: &'static str,
        fut: impl std::future::Future<Output = Result<T, sqlx::Error>>,
    ) -> Result<T, TimedDbError> {
        let reg = metrics::global();
        let started = std::time::Instant::now();
        match tokio::time::timeout(self.query_timeout, fut).await {
            Ok(Ok(v)) => {
                reg.histogram(
                    "bot_db_operation_duration_ms",
                    "Database operation latency in milliseconds.",
                    &[],
                    metrics::LATENCY_BUCKETS_MS,
                )
                .observe(started.elapsed().as_millis() as u64);
                reg.counter(
                    "bot_db_operations_total",
                    "Database operations by op and outcome.",
                    &[("op", op), ("outcome", "ok")],
                )
                .inc();
                Ok(v)
            }
            Ok(Err(e)) => {
                reg.counter(
                    "bot_db_operations_total",
                    "Database operations by op and outcome.",
                    &[("op", op), ("outcome", "error")],
                )
                .inc();
                warn!(op, error = %e, "database operation failed");
                Err(TimedDbError::Error(e))
            }
            Err(_) => {
                reg.counter(
                    "bot_db_operations_total",
                    "Database operations by op and outcome.",
                    &[("op", op), ("outcome", "timeout")],
                )
                .inc();
                warn!(
                    op,
                    timeout_ms = self.query_timeout.as_millis() as u64,
                    "database operation timed out"
                );
                Err(TimedDbError::Timeout)
            }
        }
    }
}

/// Uniform error type for repository operations: distinguishes a real
/// database error from a client-side timeout so callers can classify
/// retryability.
#[derive(Debug)]
pub enum TimedDbError {
    Error(sqlx::Error),
    Timeout,
}

impl std::fmt::Display for TimedDbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimedDbError::Error(e) => write!(f, "{e}"),
            TimedDbError::Timeout => write!(f, "query timeout"),
        }
    }
}

impl From<TimedDbError> for BotError {
    fn from(e: TimedDbError) -> Self {
        match e {
            TimedDbError::Error(e) => BotError::db(e.to_string()),
            TimedDbError::Timeout => BotError::Timeout("database query timeout".into()),
        }
    }
}

/// Pool telemetry snapshot.
#[derive(Debug, Clone, Copy, Default)]
pub struct PoolStats {
    pub size: u32,
    pub idle: u32,
    pub active: u32,
    pub max: u32,
}

/// Resolve the connection URL from the configured env var. Returns `None`
/// when the var is unset/empty (the database then stays off unless
/// `required`, which the caller must enforce).
pub fn resolve_url(cfg: &DatabaseConfig) -> Option<String> {
    std::env::var(&cfg.url_env)
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// Startup helper: connect (+ migrate) per config. Returns `Ok(None)` when
/// the database is disabled. `required = true` turns any failure into `Err`;
/// otherwise failures degrade to `Ok(None)` with a loud warning.
pub async fn open(cfg: &DatabaseConfig) -> BotResult<Option<Database>> {
    if !cfg.enabled {
        debug!("database disabled by config");
        return Ok(None);
    }
    let Some(url) = resolve_url(cfg) else {
        let msg = format!("database enabled but env var {} is not set", cfg.url_env);
        if cfg.required {
            return Err(BotError::config(msg));
        }
        warn!(%msg, "continuing without the database");
        return Ok(None);
    };
    match Database::connect(cfg, &url).await {
        Ok(db) => {
            if cfg.auto_migrate {
                if let Err(e) = db.migrate().await {
                    if cfg.required {
                        return Err(e);
                    }
                    warn!(error = %e, "migration failed — continuing without the database");
                    return Ok(None);
                }
            }
            Ok(Some(db))
        }
        Err(e) => {
            if cfg.required {
                return Err(e);
            }
            warn!(error = %e, "database unavailable — continuing without it");
            Ok(None)
        }
    }
}
