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

pub mod accounting;
pub mod claims;
pub mod copy;
pub mod deployment_org;
pub mod execution;
pub mod ha;
pub mod mod_tenant_exports;
pub mod polymarket;
pub mod repo;
pub mod tenant_idempotency;
pub mod tenant_lock;
pub mod tenant_pagination;
pub mod tenant_query;
pub mod tenant_row;
pub mod tenant_tx;

use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Row};
use tracing::{debug, info, warn};

use crate::config::DatabaseConfig;
use crate::error::{BotError, BotResult};
use crate::obs::metrics;

/// Embed every `crates/core/migrations/*.sql` at compile time.
pub const MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

fn pool_builder(cfg: &DatabaseConfig) -> PgPoolOptions {
    PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .min_connections(cfg.min_connections)
        .acquire_timeout(Duration::from_millis(cfg.acquire_timeout_ms))
}

fn pg_connect_options(
    cfg: &DatabaseConfig,
    url: &str,
    application_name: &'static str,
) -> BotResult<PgConnectOptions> {
    let timeout = cfg.statement_timeout_ms.to_string();
    url.parse::<PgConnectOptions>()
        .map_err(|error| BotError::db(format!("invalid database URL: {error}")))
        .map(|options| {
            options
                .options([("statement_timeout", timeout.as_str())])
                .application_name(application_name)
        })
}

fn required_database_url(env_name: &str) -> BotResult<String> {
    std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            BotError::config(format!(
                "RLS requires the {} environment variable",
                env_name
            ))
        })
}

fn validate_runtime_role_names(cfg: &DatabaseConfig) -> BotResult<()> {
    let valid = |name: &str| {
        !name.trim().is_empty()
            && name.len() <= 63
            && !name.contains('\0')
            && !matches!(name, "bot_app" | "bot_system")
    };
    if !valid(&cfg.app_login_role)
        || !valid(&cfg.system_login_role)
        || cfg.app_login_role == cfg.system_login_role
    {
        return Err(BotError::config(
            "database app_login_role and system_login_role must be distinct valid PostgreSQL login roles",
        ));
    }
    Ok(())
}

fn quote_role_identifier(role: &str) -> String {
    format!("\"{}\"", role.replace('"', "\"\""))
}

async fn grant_runtime_roles(
    migration_pool: &PgPool,
    app_login_role: &str,
    system_login_role: &str,
) -> BotResult<()> {
    let app_login_role = quote_role_identifier(app_login_role);
    let system_login_role = quote_role_identifier(system_login_role);
    sqlx::query(&format!("GRANT bot_app TO {app_login_role}"))
        .execute(migration_pool)
        .await
        .map_err(|error| BotError::db(format!("grant bot_app to configured app login: {error}")))?;
    sqlx::query(&format!("GRANT bot_system TO {system_login_role}"))
        .execute(migration_pool)
        .await
        .map_err(|error| {
            BotError::db(format!(
                "grant bot_system to configured system login: {error}"
            ))
        })?;
    Ok(())
}

async fn configure_app_connection(
    connection: &mut sqlx::postgres::PgConnection,
) -> Result<(), sqlx::Error> {
    sqlx::query("SET ROLE bot_app")
        .execute(&mut *connection)
        .await?;
    let organization_id = crate::tenant::rls_context::current_request_tenant()
        .map(|id| id.to_string())
        .unwrap_or_default();
    sqlx::query("SELECT set_config('app.org_id', $1, false)")
        .bind(organization_id)
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn configure_system_connection(
    connection: &mut sqlx::postgres::PgConnection,
) -> Result<(), sqlx::Error> {
    sqlx::query("SET ROLE bot_system")
        .execute(&mut *connection)
        .await?;
    sqlx::query("SELECT set_config('app.org_id', '', false)")
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn reset_connection_context(
    connection: &mut sqlx::postgres::PgConnection,
) -> Result<bool, sqlx::Error> {
    sqlx::query("SELECT set_config('app.org_id', '', false)")
        .execute(&mut *connection)
        .await?;
    sqlx::query("RESET ROLE").execute(&mut *connection).await?;
    Ok(true)
}

/// A connected PostgreSQL handle plus the policy that governs it.
#[derive(Clone)]
pub struct Database {
    /// Request/data pool. In RLS mode every checkout assumes `bot_app` and
    /// carries the task-local authenticated tenant, defaulting to no tenant.
    pool: PgPool,
    /// Privileged identity/control-plane pool; never used for trading data.
    system_pool: Option<PgPool>,
    /// Schema-owner pool used only for migrations and migration metadata.
    migration_pool: Option<PgPool>,
    rls_enabled: bool,
    query_timeout: Duration,
}

impl Database {
    /// Connect using `[database]` config. `url` is the resolved connection
    /// string (read from the configured env var by the caller).
    pub async fn connect(cfg: &DatabaseConfig, url: &str) -> BotResult<Self> {
        let (system_pool, migration_pool, pool) = if cfg.rls_enabled {
            let system_url = required_database_url(&cfg.system_url_env)?;
            validate_runtime_role_names(cfg)?;
            let migration_pool = if cfg.auto_migrate {
                let migration_url = required_database_url(&cfg.migration_url_env)?;
                let migration_options =
                    pg_connect_options(cfg, &migration_url, "sniper-suite-migrations")?;
                let migration_pool = pool_builder(cfg)
                    .connect_with(migration_options)
                    .await
                    .map_err(|e| {
                        BotError::db(format!(
                            "migration database pool connect failed for env {}: {e}",
                            cfg.migration_url_env
                        ))
                    })?;
                MIGRATOR
                    .run(&migration_pool)
                    .await
                    .map_err(|e| BotError::db(format!("RLS bootstrap migrations failed: {e}")))?;
                grant_runtime_roles(&migration_pool, &cfg.app_login_role, &cfg.system_login_role)
                    .await?;
                Some(migration_pool)
            } else {
                None
            };

            let system_options = pg_connect_options(cfg, &system_url, "sniper-suite-system")?;
            if system_options.get_username() != cfg.system_login_role {
                return Err(BotError::config(
                    "POSTGRES_SYSTEM_URL username must match database.system_login_role",
                ));
            }
            let app_options = pg_connect_options(cfg, url, "sniper-suite-app")?;
            if app_options.get_username() != cfg.app_login_role {
                return Err(BotError::config(
                    "application database URL username must match database.app_login_role",
                ));
            }
            let system_pool = pool_builder(cfg)
                .after_connect(|connection, _| {
                    Box::pin(async move { configure_system_connection(connection).await })
                })
                .before_acquire(|connection, _| {
                    Box::pin(async move {
                        configure_system_connection(connection).await?;
                        Ok(true)
                    })
                })
                .after_release(|connection, _| {
                    Box::pin(async move {
                        reset_connection_context(connection).await?;
                        Ok(true)
                    })
                })
                .connect_with(system_options)
                .await
                .map_err(|e| {
                    BotError::db(format!(
                        "system database pool connect failed for env {}: {e}",
                        cfg.system_url_env
                    ))
                })?;

            let app_pool = pool_builder(cfg)
                .after_connect(|connection, _| {
                    Box::pin(async move { configure_app_connection(connection).await })
                })
                .before_acquire(|connection, _| {
                    Box::pin(async move {
                        configure_app_connection(connection).await?;
                        Ok(true)
                    })
                })
                .after_release(|connection, _| {
                    Box::pin(async move {
                        reset_connection_context(connection).await?;
                        Ok(true)
                    })
                })
                .connect_with(app_options)
                .await
                .map_err(|e| BotError::db(format!("application database connect failed: {e}")))?;
            (Some(system_pool), migration_pool, app_pool)
        } else {
            let options = pg_connect_options(cfg, url, "sniper-suite-legacy")?;
            let pool = pool_builder(cfg)
                .connect_with(options)
                .await
                .map_err(|e| BotError::db(format!("database connect failed: {e}")))?;
            (None, None, pool)
        };

        let db = Database {
            pool,
            system_pool,
            migration_pool,
            rls_enabled: cfg.rls_enabled,
            query_timeout: Duration::from_millis(cfg.query_timeout_ms.max(100)),
        };
        db.ping().await?;
        info!(
            max_connections = cfg.max_connections,
            rls_enabled = cfg.rls_enabled,
            system_pool = db.system_pool.is_some(),
            "postgres connected"
        );
        Ok(db)
    }

    /// Run the embedded migrations (idempotent). Not routed through `timed`
    /// (MigrateError is not a sqlx::Error); migrations may legitimately take
    /// longer than a query timeout, so no client-side cap is applied.
    pub async fn migrate(&self) -> BotResult<()> {
        let migration_pool = match &self.migration_pool {
            Some(pool) => pool,
            None if !self.rls_enabled => &self.pool,
            None => {
                return Err(BotError::config(
                    "RLS mode requires a separately configured migration pool",
                ));
            }
        };
        MIGRATOR
            .run(migration_pool)
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
                sqlx::query("SELECT COUNT(*) AS n FROM _sqlx_migrations").fetch_one(
                    self.migration_pool
                        .as_ref()
                        .or(self.system_pool.as_ref())
                        .unwrap_or(&self.pool),
                ),
            )
            .await
            .map_err(|e| BotError::db(e.to_string()))?;
        Ok(row.get::<i64, _>("n"))
    }

    /// Close the pool, waiting for in-flight statements. Called during the
    /// graceful-shutdown persistence phase.
    pub async fn close(&self) {
        self.pool.close().await;
        if let Some(pool) = &self.system_pool {
            pool.close().await;
        }
        if let Some(pool) = &self.migration_pool {
            pool.close().await;
        }
        info!("postgres pools closed");
    }

    /// The non-bypass request/data pool. In RLS mode this role defaults to
    /// an empty tenant context and is bound to the authenticated request
    /// context on every connection checkout.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// The separate privileged identity/control-plane pool, when RLS mode
    /// is enabled. Never use it for tenant trading queries.
    pub fn system_pool(&self) -> Option<&PgPool> {
        self.system_pool.as_ref()
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
    if !cfg.rls_enabled {
        return Err(BotError::config(
            "database.enabled requires database.rls_enabled=true; legacy unscoped pools are not allowed at runtime",
        ));
    }
    required_database_url(&cfg.system_url_env)?;
    if cfg.auto_migrate {
        required_database_url(&cfg.migration_url_env)?;
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
        Ok(db) => Ok(Some(db)),
        Err(error) => {
            if cfg.required {
                return Err(error);
            }
            warn!(error = %error, "database unavailable — continuing without it");
            Ok(None)
        }
    }
}
