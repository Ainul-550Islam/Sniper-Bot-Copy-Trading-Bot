//! Tenant-scoped intent journal writes (PROMPT 3/10 §E32).

use std::sync::Arc;

use crate::db::Database;
use crate::trading_repository::repository_error::RepositoryError;
use crate::trading_repository::write_scope::TenantWriteScope;

/// Write-side tenant intent repository (`execution_intents`).
pub struct TenantIntentWrite {
    db: Arc<Database>,
}

impl TenantIntentWrite {
    pub fn new(db: Arc<Database>) -> Self {
        TenantIntentWrite { db }
    }

    /// Journal an intent BEFORE broadcast, attributed to the acting
    /// tenant. Idempotent on the 0031 composite arbiter
    /// `(organization_id, intent_id)`: a retry of THIS tenant's intent
    /// never rewrites the original timestamp; ANOTHER tenant's same
    /// intent_id journals independently.
    #[allow(clippy::too_many_arguments)]
    pub async fn record(
        &self,
        write: &TenantWriteScope,
        intent_id: &str,
        module: &str,
        symbol: &str,
        wallet: &str,
        side: &str,
        qty: &str,
    ) -> Result<(), RepositoryError> {
        if intent_id.trim().is_empty() {
            return Err(RepositoryError::Validation("intent_id"));
        }
        self.db
            .timed(
                "tenant_intent_record",
                sqlx::query(
                    r#"INSERT INTO execution_intents
                           (organization_id, intent_id, module, symbol, wallet, side, qty)
                       VALUES ($1, $2, $3, $4, $5, $6, $7)
                       ON CONFLICT (organization_id, intent_id) DO NOTHING"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(intent_id)
                .bind(module)
                .bind(symbol)
                .bind(wallet)
                .bind(side)
                .bind(qty)
                .execute(self.db.pool()),
            )
            .await?;
        Ok(())
    }

    /// The broadcast produced `signature`: THIS tenant's intent is no
    /// longer ambiguous. Only pending intents transition (terminal rows
    /// are immutable). Zero rows (absent/foreign/already terminal) →
    /// `StaleWrite`/`NotFound`, never the other tenant's row.
    pub async fn link(
        &self,
        write: &TenantWriteScope,
        intent_id: &str,
        signature: &str,
    ) -> Result<(), RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_intent_link",
                sqlx::query(
                    r#"UPDATE execution_intents
                          SET status = 'submitted', signature = $3, updated_at = now()
                        WHERE organization_id = $1 AND intent_id = $2
                          AND status = 'pending'"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(intent_id)
                .bind(signature)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            Err(self.miss_error(write, intent_id).await)
        }
    }

    /// The attempt provably never broadcast (terminal error, no
    /// signature) — THIS tenant's intent becomes `abandoned`.
    pub async fn abandon(
        &self,
        write: &TenantWriteScope,
        intent_id: &str,
    ) -> Result<(), RepositoryError> {
        let res = self
            .db
            .timed(
                "tenant_intent_abandon",
                sqlx::query(
                    r#"UPDATE execution_intents
                          SET status = 'abandoned', updated_at = now()
                        WHERE organization_id = $1 AND intent_id = $2
                          AND status = 'pending'"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(intent_id)
                .execute(self.db.pool()),
            )
            .await?;
        if res.rows_affected() == 1 {
            Ok(())
        } else {
            Err(self.miss_error(write, intent_id).await)
        }
    }

    /// Distinguish "not ours / absent" from "ours but moved on"
    /// without ever leaking a foreign row's existence or state.
    async fn miss_error(&self, write: &TenantWriteScope, intent_id: &str) -> RepositoryError {
        let row = self
            .db
            .timed(
                "tenant_intent_miss_probe",
                sqlx::query(
                    r#"SELECT 1 FROM execution_intents
                        WHERE organization_id = $1 AND intent_id = $2"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(intent_id)
                .fetch_optional(self.db.pool()),
            )
            .await;
        match row {
            Ok(Some(_)) => RepositoryError::StaleWrite("intent status"),
            _ => RepositoryError::NotFound("intent"),
        }
    }
}
