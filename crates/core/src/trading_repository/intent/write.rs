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
    ///
    /// **A duplicate id must describe the SAME intent.** `DO NOTHING` used to
    /// accept ANY row that already owned the id, so reusing an id for a
    /// different module/symbol/wallet/side/qty silently aliased the new intent
    /// onto the old row and the caller believed its (different) order was
    /// journalled. The conflict is now payload-aware: the row is written only
    /// when the payload is identical (`IS NOT DISTINCT FROM` on every business
    /// column), and an exact retry still succeeds without touching
    /// `created_at`/`status`. A differing payload is
    /// [`RepositoryError::Conflict`]`("intent_id")` — terminal, never retried
    /// under the same id (generate a new intent id instead).
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
        // `DO UPDATE … WHERE payload differs → RETURNING 1` writes nothing and
        // returns no row when the payload is identical, so: no row ⇒ exact
        // retry (idempotent success); `Conflict` from a DO NOTHING-style clash
        // is impossible now, so a missing row is the ONLY success. A differing
        // payload produces no row either — which is why the statement also
        // reports whether the id existed at all, in one round trip.
        let (existed, accepted): (bool, bool) = self
            .db
            .timed(
                "tenant_intent_record",
                sqlx::query_as(
                    r#"WITH attempted AS (
                         INSERT INTO execution_intents
                           (organization_id, intent_id, module, symbol, wallet, side, qty)
                         VALUES ($1, $2, $3, $4, $5, $6, $7)
                         ON CONFLICT (organization_id, intent_id) DO UPDATE
                            SET intent_id = execution_intents.intent_id
                          WHERE ROW(execution_intents.module, execution_intents.symbol,
                                    execution_intents.wallet, execution_intents.side,
                                    execution_intents.qty)
                            IS NOT DISTINCT FROM
                                ROW(EXCLUDED.module, EXCLUDED.symbol, EXCLUDED.wallet,
                                    EXCLUDED.side, EXCLUDED.qty)
                         RETURNING 1
                       ), existing AS (
                         SELECT 1 FROM execution_intents
                          WHERE organization_id = $1 AND intent_id = $2
                       )
                       SELECT EXISTS (SELECT 1 FROM existing),
                              EXISTS (SELECT 1 FROM attempted)"#,
                )
                .bind(write.organization_id().as_uuid())
                .bind(intent_id)
                .bind(module)
                .bind(symbol)
                .bind(wallet)
                .bind(side)
                .bind(qty)
                .fetch_one(self.db.pool()),
            )
            .await?;
        if accepted {
            return Ok(());
        }
        if existed {
            // Same id, different payload: refused. Exact retries returned
            // `accepted` above, so this is exclusively the mismatch case.
            return Err(RepositoryError::Conflict("intent_id"));
        }
        // Neither inserted nor visible: the row was written by a concurrent
        // caller of the SAME tenant and is not visible in this snapshot.
        Err(RepositoryError::Conflict("intent_id"))
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
        classify_miss_probe(row.map(|row| row.is_some()))
    }
}

fn classify_miss_probe(probe: Result<bool, crate::db::TimedDbError>) -> RepositoryError {
    match probe {
        Ok(true) => RepositoryError::StaleWrite("intent status"),
        Ok(false) => RepositoryError::NotFound("intent"),
        Err(error) => RepositoryError::from(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miss_probe_timeout_is_storage_error_not_not_found() {
        let err = classify_miss_probe(Err(crate::db::TimedDbError::Timeout));
        assert!(matches!(err, RepositoryError::Storage(_)));
        assert!(err.is_retryable());
        assert!(matches!(
            classify_miss_probe(Ok(false)),
            RepositoryError::NotFound("intent")
        ));
        assert!(matches!(
            classify_miss_probe(Ok(true)),
            RepositoryError::StaleWrite("intent status")
        ));
    }
}
