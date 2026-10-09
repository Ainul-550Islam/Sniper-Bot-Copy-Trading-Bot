//! Tenant-scoped idempotency key behavior (PROMPT 3/10 §C20).
//!
//! Facade over the STEP 3 primitive
//! [`crate::db::tenant_idempotency`](crate::db::tenant_idempotency) —
//! now on the 0027 composite arbiter `(organization_id, scope, key)` —
//! adding the pool-level operations the trading data plane needs:
//! consume, response replay and retention, each scoped to the acting
//! tenant.
//!
//! Contract: a key reserved by tenant A is invisible to tenant B;
//! tenant B reserving the SAME `(scope, key)` succeeds — tenant-local
//! business identity (tested in `tests/orders_tenant_isolation.rs`
//! and `tests/executions_tenant_isolation.rs`).

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::db::tenant_idempotency::TenantIdempotencyKey;
use crate::db::Database;
use crate::trading_repository::query_scope::TradingQueryScope;
use crate::trading_repository::repository_error::RepositoryError;

/// Tenant-scoped idempotency repository (`idempotency_keys`).
pub struct TenantIdempotencyRepo {
    db: Arc<Database>,
}

impl TenantIdempotencyRepo {
    pub fn new(db: Arc<Database>) -> Self {
        TenantIdempotencyRepo { db }
    }

    /// Reserve `(scope, key)` for the acting tenant.
    /// `Ok(true)` = newly reserved (proceed); `Ok(false)` = this tenant
    /// already consumed it (duplicate).
    pub async fn try_consume(
        &self,
        scope: &TradingQueryScope,
        idem_scope: &str,
        key: &str,
    ) -> Result<bool, RepositoryError> {
        let idem = TenantIdempotencyKey::new(scope.organization_id(), idem_scope, key)
            .map_err(|_| RepositoryError::Validation("idempotency scope/key bounds"))?;
        let mut conn = self
            .db
            .pool()
            .acquire()
            .await
            .map_err(RepositoryError::from)?;
        crate::db::tenant_idempotency::insert_once(&mut conn, &idem)
            .await
            .map_err(|e| RepositoryError::Storage(e.to_string()))
    }

    /// Record the replayable response for THIS tenant's key. A foreign
    /// key updates zero rows (silent no-op is safe here: the response
    /// belongs to the caller's own key only).
    pub async fn record_response(
        &self,
        scope: &TradingQueryScope,
        idem_scope: &str,
        key: &str,
        response: &serde_json::Value,
    ) -> Result<(), RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_idem_record_response",
                sqlx::query(
                    r#"UPDATE idempotency_keys SET response = $4
                        WHERE organization_id = $1 AND scope = $2 AND key = $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(idem_scope)
                .bind(key)
                .bind(response)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            Err(RepositoryError::NotFound("idempotency key"))
        }
    }

    /// Fetch THIS tenant's recorded response, if any.
    pub async fn response(
        &self,
        scope: &TradingQueryScope,
        idem_scope: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>, RepositoryError> {
        let row = self
            .db
            .timed(
                "tenant_idem_response",
                sqlx::query(
                    r#"SELECT response FROM idempotency_keys
                        WHERE organization_id = $1 AND scope = $2 AND key = $3"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(idem_scope)
                .bind(key)
                .fetch_optional(self.db.pool()),
            )
            .await?;
        Ok(row
            .and_then(|r| r.try_get::<Option<serde_json::Value>, _>("response").ok())
            .flatten())
    }

    /// Is THIS tenant holding the key? (pure lookup; the insert remains
    /// the atomic authority).
    pub async fn held(
        &self,
        scope: &TradingQueryScope,
        idem_scope: &str,
        key: &str,
    ) -> Result<bool, RepositoryError> {
        let idem = TenantIdempotencyKey::new(scope.organization_id(), idem_scope, key)
            .map_err(|_| RepositoryError::Validation("idempotency scope/key bounds"))?;
        let mut conn = self
            .db
            .pool()
            .acquire()
            .await
            .map_err(RepositoryError::from)?;
        crate::db::tenant_idempotency::held_by_tenant(&mut conn, &idem)
            .await
            .map_err(|e| RepositoryError::Storage(e.to_string()))
    }

    /// Retention: drop THIS tenant's keys older than `age` (the tenant-
    /// scoped twin of the legacy cleanup; the deployment-wide job
    /// remains the legacy path).
    pub async fn cleanup_older_than(
        &self,
        scope: &TradingQueryScope,
        age: chrono::Duration,
        now: DateTime<Utc>,
    ) -> Result<u64, RepositoryError> {
        let cutoff = now - age;
        let res = self
            .db
            .timed(
                "tenant_idem_cleanup",
                sqlx::query(
                    r#"DELETE FROM idempotency_keys
                        WHERE organization_id = $1 AND created_at < $2"#,
                )
                .bind(scope.organization_id().as_uuid())
                .bind(cutoff)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(res.rows_affected())
    }
}
