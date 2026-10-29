//! Transaction-local PostgreSQL row-level-security context.
//!
//! This module is the sole owner of `app.org_id`. Request tenant identity is
//! held in a Tokio task-local, copied onto checked-out application-pool
//! sessions by the database pool hooks, and reset before a connection returns
//! to the pool. `TenantScope` also installs transaction-local role and GUC
//! state for operations that explicitly own a transaction.

use std::cell::Cell;
use std::future::Future;

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

tokio::task_local! {
    static REQUEST_ORGANIZATION_ID: Cell<Option<Uuid>>;
}

/// Run an HTTP request future with an initially empty tenant context. The
/// authenticated middleware binds the organization once authorization passes.
pub async fn scope_request_context<F: Future>(future: F) -> F::Output {
    REQUEST_ORGANIZATION_ID.scope(Cell::new(None), future).await
}

/// Bind the authenticated tenant to the current request task. Rebinding to a
/// different organization is rejected; outside a request scope this is a
/// no-op so non-HTTP worker paths remain fail-closed on the app pool.
pub fn bind_request_tenant(organization_id: Uuid) -> Result<(), TenantContextConflict> {
    match REQUEST_ORGANIZATION_ID.try_with(|slot| match slot.get() {
        Some(existing) if existing != organization_id => Err(TenantContextConflict {
            existing,
            requested: organization_id,
        }),
        Some(_) => Ok(()),
        None => {
            slot.set(Some(organization_id));
            Ok(())
        }
    }) {
        Ok(result) => result,
        Err(_) => Ok(()),
    }
}

/// Run a trusted non-HTTP worker future with its tenant already bound.
///
/// This is the worker counterpart of [`scope_request_context`] +
/// [`bind_request_tenant`]: long-lived tasks that act for exactly one
/// organization (the deployment-scoped Telegram trade console is the
/// Batch 07 caller) have no HTTP middleware to bind the tenant for them,
/// and Tokio task-locals do not propagate into `tokio::spawn`, so the
/// scope must wrap the spawned future itself. The scope starts with the
/// tenant already bound, so a later `bind_request_tenant` to a different
/// organization inside the future is rejected exactly as on the HTTP path.
pub async fn scope_tenant_context<F: Future>(organization_id: Uuid, future: F) -> F::Output {
    REQUEST_ORGANIZATION_ID
        .scope(Cell::new(Some(organization_id)), future)
        .await
}

/// Current authenticated tenant used by the request-pool checkout hook.
pub fn current_request_tenant() -> Option<Uuid> {
    REQUEST_ORGANIZATION_ID.try_with(Cell::get).ok().flatten()
}

/// A request attempted to bind more than one tenant to one task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantContextConflict {
    pub existing: Uuid,
    pub requested: Uuid,
}

/// An open transaction with one tenant's RLS context installed.
pub struct TenantScope<'a> {
    transaction: Transaction<'a, Postgres>,
    organization_id: Uuid,
}

impl<'a> TenantScope<'a> {
    /// Begin a transaction and set the tenant context transaction-locally.
    pub async fn begin(pool: &'a sqlx::PgPool, organization_id: Uuid) -> Result<Self, sqlx::Error> {
        let transaction = pool.begin().await?;
        Self::from_transaction(transaction, organization_id).await
    }

    /// Install the tenant GUC into a transaction opened by a caller that must
    /// perform transaction-local setup (for example, `SET LOCAL ROLE`).
    pub async fn from_transaction(
        mut transaction: Transaction<'a, Postgres>,
        organization_id: Uuid,
    ) -> Result<Self, sqlx::Error> {
        install_local_role(&mut transaction, "bot_app").await?;
        install_org_id(&mut transaction, organization_id).await?;
        Ok(Self {
            transaction,
            organization_id,
        })
    }

    /// The organization this transaction is scoped to.
    pub fn organization_id(&self) -> Uuid {
        self.organization_id
    }

    /// Borrow the underlying transaction for SQLx query execution.
    pub fn transaction(&mut self) -> &mut Transaction<'a, Postgres> {
        &mut self.transaction
    }

    /// Commit the scoped transaction.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.transaction.commit().await
    }

    /// Roll back the scoped transaction.
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.transaction.rollback().await
    }
}

/// An explicit transaction for trusted identity, webhook, migration, and
/// lifecycle operations that must run with the `bot_system` database role.
/// The connection principal must be authenticated as, or be a member of,
/// `bot_system`; ordinary tenant-pool principals must never receive that grant.
pub struct SystemScope<'a> {
    transaction: Transaction<'a, Postgres>,
}

impl<'a> SystemScope<'a> {
    /// Begin a system transaction without installing a tenant GUC.
    pub async fn begin(pool: &'a sqlx::PgPool) -> Result<Self, sqlx::Error> {
        let mut transaction = pool.begin().await?;
        install_local_role(&mut transaction, "bot_system").await?;
        clear_org_id(&mut transaction).await?;
        Ok(Self { transaction })
    }

    /// Borrow the underlying transaction for SQLx query execution.
    pub fn transaction(&mut self) -> &mut Transaction<'a, Postgres> {
        &mut self.transaction
    }

    /// Commit the system transaction.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        self.transaction.commit().await
    }

    /// Roll back the system transaction.
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        self.transaction.rollback().await
    }
}

async fn install_local_role(
    transaction: &mut Transaction<'_, Postgres>,
    role: &'static str,
) -> Result<(), sqlx::Error> {
    let statement = match role {
        "bot_app" => "SET LOCAL ROLE bot_app",
        "bot_system" => "SET LOCAL ROLE bot_system",
        _ => unreachable!("RLS roles are selected only from static constants"),
    };
    sqlx::query(statement).execute(&mut **transaction).await?;
    Ok(())
}

async fn clear_org_id(transaction: &mut Transaction<'_, Postgres>) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.org_id', '', true)")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn install_org_id(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT set_config('app.org_id', $1, true)")
        .bind(organization_id.to_string())
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_context_is_task_local_and_rejects_tenant_switches() {
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        scope_request_context(async {
            assert_eq!(current_request_tenant(), None);
            bind_request_tenant(tenant_a).unwrap();
            assert_eq!(current_request_tenant(), Some(tenant_a));
            assert_eq!(
                bind_request_tenant(tenant_b),
                Err(TenantContextConflict {
                    existing: tenant_a,
                    requested: tenant_b,
                })
            );
        })
        .await;
        assert_eq!(current_request_tenant(), None);
    }

    #[tokio::test]
    async fn independent_request_scopes_do_not_share_tenants() {
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        let ((), ()) = tokio::join!(
            scope_request_context(async {
                bind_request_tenant(tenant_a).unwrap();
                assert_eq!(current_request_tenant(), Some(tenant_a));
            }),
            scope_request_context(async {
                bind_request_tenant(tenant_b).unwrap();
                assert_eq!(current_request_tenant(), Some(tenant_b));
            })
        );
    }

    #[tokio::test]
    async fn tenant_context_scope_starts_bound_and_rejects_switches() {
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        scope_tenant_context(tenant_a, async {
            assert_eq!(current_request_tenant(), Some(tenant_a));
            bind_request_tenant(tenant_a).unwrap();
            assert_eq!(
                bind_request_tenant(tenant_b),
                Err(TenantContextConflict {
                    existing: tenant_a,
                    requested: tenant_b,
                })
            );
        })
        .await;
        assert_eq!(current_request_tenant(), None);
    }

    #[test]
    fn tenant_scope_uses_canonical_uuid_text() {
        let id = Uuid::parse_str("2e09c64e-b627-4f03-9c18-f1040d9bc567").unwrap();
        assert_eq!(id.to_string(), "2e09c64e-b627-4f03-9c18-f1040d9bc567");
    }
}
