//! Tenant transaction wrapper (STEP 3 file 45).
//!
//! [`TenantTx`] is the only way the NEW tenant repository code opens a
//! trading transaction: it REQUIRES a [`TenantQueryScope`], so a
//! "global" trading transaction cannot be constructed by accident. The
//! legacy deployment-global repositories keep their own paths untouched —
//! this wrapper is additive.
//!
//! Semantics:
//!
//! * `begin` opens a `sqlx` transaction (statement/lock semantics
//!   identical to the existing repositories);
//! * the scope is carried inside, so every statement in the transaction
//!   can bind the tenant without passing it around;
//! * commit/rollback are explicit; dropping the wrapper rolls back —
//!   the same discipline `sqlx` itself applies.

use sqlx::{Postgres, Transaction};

use crate::error::{BotError, BotResult};

use super::tenant_query::TenantQueryScope;
use super::Database;

/// A tenant-scoped transaction.
pub struct TenantTx<'a> {
    tx: Transaction<'a, Postgres>,
    scope: TenantQueryScope,
}

impl<'a> TenantTx<'a> {
    /// Begin a transaction that is scoped to one tenant. The scope is
    /// REQUIRED — there is no `TenantTx::begin_global`.
    pub async fn begin(db: &'a Database, scope: TenantQueryScope) -> BotResult<TenantTx<'a>> {
        let tx = db
            .pool()
            .begin()
            .await
            .map_err(|e| BotError::db(format!("tenant tx begin failed: {e}")))?;
        Ok(TenantTx { tx, scope })
    }

    /// The tenant this transaction executes under.
    pub fn scope(&self) -> &TenantQueryScope {
        &self.scope
    }

    /// The acting organization.
    pub fn organization_id(&self) -> crate::tenant::OrganizationId {
        self.scope.organization_id()
    }

    /// The underlying connection, for executing scoped statements.
    pub fn connection(&mut self) -> &mut PgConnectionAsSqlx {
        self.tx.as_mut()
    }

    /// Commit the transaction.
    pub async fn commit(self) -> BotResult<()> {
        self.tx
            .commit()
            .await
            .map_err(|e| BotError::db(format!("tenant tx commit failed: {e}")))
    }

    /// Roll the transaction back explicitly (dropping it does the same).
    pub async fn rollback(self) -> BotResult<()> {
        self.tx
            .rollback()
            .await
            .map_err(|e| BotError::db(format!("tenant tx rollback failed: {e}")))
    }
}

/// Alias keeping the public signature honest: the connection type of a
/// `sqlx` PostgreSQL transaction is `sqlx::PgConnection`.
pub type PgConnectionAsSqlx = sqlx::PgConnection;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::OrganizationId;

    fn pg() -> Option<&'static Database> {
        // Bound in the integration suites (POSTGRES_URL); unit tests only
        // exercise the type-level contract here.
        None
    }

    #[test]
    fn scope_is_required_and_carried() {
        // The constructor signature itself enforces the scope; this test
        // documents the contract and keeps the type constructible in
        // doctests. A Database is required for begin(); see
        // tenant_idempotency_isolation.rs for the live exercise.
        let org = OrganizationId::new();
        let scope = TenantQueryScope::new(org);
        assert_eq!(scope.organization_id(), org);
        assert!(pg().is_none());
    }
}
