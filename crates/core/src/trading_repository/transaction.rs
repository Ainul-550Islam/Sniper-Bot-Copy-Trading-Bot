//! Tenant-aware transaction boundary (PROMPT 3/10 §A6).
//!
//! [`TradingTransaction`] is the ONLY way the tenant trading
//! repositories open a multi-statement transaction. It wraps the STEP 3
//! [`TenantTx`] (which REQUIRES a [`TenantQueryScope`] — there is no
//! global trading transaction) and adds:
//!
//! * the read scope carried for the whole transaction (every statement
//!   can bind the tenant without passing it around);
//! * an optional WRITE scope, set by [`TradingTransaction::for_write`]
//!   before the first mutation — an attribution record (actor/origin)
//!   the repositories read for their audit columns;
//! * the tenant advisory-lock entry point (`recovery sweep`,
//!   `recon sweep` …) namespaced per (tenant, purpose), so two tenants
//!   doing the same logical job never contend while two workers of one
//!   tenant still serialize.
//!
//! Commit and rollback are explicit; dropping the wrapper rolls back
//! (sqlx discipline, unchanged).

use crate::db::tenant_tx::TenantTx;
use crate::tenant::OrganizationId;

use super::query_scope::TradingQueryScope;
use super::repository_error::RepositoryError;
use super::write_scope::TenantWriteScope;

/// A tenant-scoped trading transaction.
pub struct TradingTransaction<'a> {
    tx: TenantTx<'a>,
    write: Option<TenantWriteScope>,
}

impl<'a> TradingTransaction<'a> {
    /// Begin a transaction scoped to one tenant's reads. There is no
    /// `begin_global`.
    pub async fn begin(
        db: &'a crate::db::Database,
        scope: TradingQueryScope,
    ) -> Result<Self, RepositoryError> {
        let tx = TenantTx::begin(db, *scope.tenant_scope())
            .await
            .map_err(|e| RepositoryError::Storage(e.to_string()))?;
        Ok(TradingTransaction { tx, write: None })
    }

    /// Attach the write scope (actor/origin) the mutations in this
    /// transaction are attributed to. Must be set before the first
    /// mutation; returns the previous scope if one was already set.
    pub fn for_write(&mut self, write: TenantWriteScope) -> Option<TenantWriteScope> {
        self.write.replace(write)
    }

    /// The acting organization.
    pub fn organization_id(&self) -> OrganizationId {
        self.tx.organization_id()
    }

    /// The write scope, when the transaction mutates.
    pub fn write_scope(&self) -> Option<&TenantWriteScope> {
        self.write.as_ref()
    }

    /// The actor of the transaction (blank when read-only).
    pub fn actor(&self) -> &str {
        self.write.as_ref().map(|w| w.actor()).unwrap_or("")
    }

    /// The underlying connection for executing scoped statements.
    pub fn connection(&mut self) -> &mut sqlx::PgConnection {
        self.tx.connection()
    }

    /// Acquire the tenant-namespaced transaction-scoped advisory lock
    /// for `purpose` (recovery sweep, recon sweep, …). Blocks until
    /// held; released automatically at commit/rollback.
    pub async fn tenant_lock(&mut self, purpose: &str) -> Result<(), RepositoryError> {
        let scope = *self.tx.scope();
        crate::db::tenant_lock::tenant_advisory_xact_lock(self.connection(), &scope, purpose)
            .await
            .map_err(|e| RepositoryError::Storage(e.to_string()))
    }

    /// Try to acquire the tenant advisory lock without blocking.
    /// `Ok(false)` = another worker of the SAME tenant holds it.
    pub async fn try_tenant_lock(&mut self, purpose: &str) -> Result<bool, RepositoryError> {
        let scope = *self.tx.scope();
        crate::db::tenant_lock::try_tenant_advisory_xact_lock(self.connection(), &scope, purpose)
            .await
            .map_err(|e| RepositoryError::Storage(e.to_string()))
    }

    /// Commit the transaction.
    pub async fn commit(self) -> Result<(), RepositoryError> {
        self.tx
            .commit()
            .await
            .map_err(|e| RepositoryError::Storage(e.to_string()))
    }

    /// Roll the transaction back explicitly.
    pub async fn rollback(self) -> Result<(), RepositoryError> {
        self.tx
            .rollback()
            .await
            .map_err(|e| RepositoryError::Storage(e.to_string()))
    }
}

// Re-exported for callers that compose raw tenant transactions.
pub use crate::db::tenant_tx::PgConnectionAsSqlx;

impl<'a> std::fmt::Debug for TradingTransaction<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TradingTransaction")
            .field("organization_id", &self.tx.organization_id())
            .field("has_write_scope", &self.write.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tenant::OrganizationId;
    use crate::trading_repository::WriteOrigin;

    #[test]
    fn scope_is_carried_and_write_scope_is_optional() {
        // Type-level contract: the constructor requires the scope (see
        // TradingTransaction::begin signature); the live begin/commit
        // exercise runs in the PostgreSQL isolation suites.
        let org = OrganizationId::new();
        let scope = TradingQueryScope::new(org);
        assert_eq!(scope.organization_id(), org);
        let write = TenantWriteScope::new(org, "user:1", WriteOrigin::Http).unwrap();
        assert_eq!(write.organization_id(), org);
    }
}
