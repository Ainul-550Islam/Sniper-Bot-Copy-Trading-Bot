//! Tenant binding registry (STEP 3 file 16).
//!
//! The durable record of which wallets and signers belong to which
//! tenant. The binding guard resolves a request's wallet LABEL and
//! signer KEY REF through this registry and rejects anything that is
//! not an ACTIVE binding of the requesting tenant — the ownership
//! check that makes cross-tenant execution impossible even when a
//! caller names another tenant's wallet.
//!
//! PG storage lives in `tenant_bindings` (migration 0025); the
//! in-memory implementation backs tests and detached deployments.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;

use bot_core::error::{BotError, BotResult};
use bot_core::tenant::{OrganizationId, SignerProvider};

/// One wallet binding row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletBinding {
    /// The owning tenant.
    pub organization_id: OrganizationId,
    /// The tenant-facing label ("main", "arb-1", …).
    pub label: String,
    /// The public on-chain address this label resolves to.
    pub address: String,
    /// Active for executions?
    pub active: bool,
    /// When the binding was created.
    pub created_at: DateTime<Utc>,
}

/// One signer binding row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignerBindingRow {
    /// The owning tenant.
    pub organization_id: OrganizationId,
    /// The provider kind.
    pub provider: SignerProvider,
    /// The provider-side key reference (public identifier only).
    pub key_ref: String,
    /// Active for executions?
    pub active: bool,
    /// When the binding was created.
    pub created_at: DateTime<Utc>,
}

/// The durable binding contract.
#[async_trait]
pub trait TenantBindingRegistry: Send + Sync {
    /// Upsert a wallet binding (idempotent on org+label).
    async fn put_wallet(&self, binding: WalletBinding) -> BotResult<()>;

    /// Look up one wallet binding by tenant + label.
    async fn wallet(
        &self,
        organization_id: OrganizationId,
        label: &str,
    ) -> BotResult<Option<WalletBinding>>;

    /// Every wallet binding of a tenant (active and inactive).
    async fn wallets(&self, organization_id: OrganizationId) -> BotResult<Vec<WalletBinding>>;

    /// Upsert a signer binding (idempotent on org+provider+key_ref).
    async fn put_signer(&self, binding: SignerBindingRow) -> BotResult<()>;

    /// Look up one signer binding by tenant + provider + key ref.
    async fn signer(
        &self,
        organization_id: OrganizationId,
        provider: SignerProvider,
        key_ref: &str,
    ) -> BotResult<Option<SignerBindingRow>>;

    /// Every signer binding of a tenant.
    async fn signers(&self, organization_id: OrganizationId) -> BotResult<Vec<SignerBindingRow>>;
}

/// PostgreSQL implementation over `tenant_bindings` (migration 0025).
pub struct PgTenantBindingRegistry {
    db: Arc<bot_core::db::Database>,
}

impl PgTenantBindingRegistry {
    /// Bind to the shared database handle.
    pub fn new(db: Arc<bot_core::db::Database>) -> Self {
        PgTenantBindingRegistry { db }
    }
}

#[async_trait]
impl TenantBindingRegistry for PgTenantBindingRegistry {
    async fn put_wallet(&self, binding: WalletBinding) -> BotResult<()> {
        sqlx::query(
            "INSERT INTO tenant_bindings (organization_id, kind, label, reference, active, created_at) \
             VALUES ($1, 'wallet', $2, $3, $4, $5) \
             ON CONFLICT (organization_id, kind, label) \
             DO UPDATE SET reference = EXCLUDED.reference, active = EXCLUDED.active",
        )
        .bind(binding.organization_id.as_uuid())
        .bind(&binding.label)
        .bind(&binding.address)
        .bind(binding.active)
        .bind(binding.created_at)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("wallet binding write failed: {e}")))?;
        Ok(())
    }

    async fn wallet(
        &self,
        organization_id: OrganizationId,
        label: &str,
    ) -> BotResult<Option<WalletBinding>> {
        let row = sqlx::query(
            "SELECT label, reference, active, created_at FROM tenant_bindings \
             WHERE organization_id = $1 AND kind = 'wallet' AND label = $2",
        )
        .bind(organization_id.as_uuid())
        .bind(label)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("wallet binding read failed: {e}")))?;
        Ok(row.map(|r| WalletBinding {
            organization_id,
            label: r.get("label"),
            address: r.get("reference"),
            active: r.get("active"),
            created_at: r.get("created_at"),
        }))
    }

    async fn wallets(&self, organization_id: OrganizationId) -> BotResult<Vec<WalletBinding>> {
        let rows = sqlx::query(
            "SELECT label, reference, active, created_at FROM tenant_bindings \
             WHERE organization_id = $1 AND kind = 'wallet' ORDER BY label",
        )
        .bind(organization_id.as_uuid())
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("wallet binding scan failed: {e}")))?;
        Ok(rows
            .iter()
            .map(|r| WalletBinding {
                organization_id,
                label: r.get("label"),
                address: r.get("reference"),
                active: r.get("active"),
                created_at: r.get("created_at"),
            })
            .collect())
    }

    async fn put_signer(&self, binding: SignerBindingRow) -> BotResult<()> {
        sqlx::query(
            "INSERT INTO tenant_bindings (organization_id, kind, label, reference, active, created_at) \
             VALUES ($1, 'signer', $2, $3, $4, $5) \
             ON CONFLICT (organization_id, kind, label) \
             DO UPDATE SET reference = EXCLUDED.reference, active = EXCLUDED.active",
        )
        .bind(binding.organization_id.as_uuid())
        .bind(format!("{}/{}", binding.provider.as_str(), binding.key_ref))
        .bind(&binding.key_ref)
        .bind(binding.active)
        .bind(binding.created_at)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("signer binding write failed: {e}")))?;
        Ok(())
    }

    async fn signer(
        &self,
        organization_id: OrganizationId,
        provider: SignerProvider,
        key_ref: &str,
    ) -> BotResult<Option<SignerBindingRow>> {
        let row = sqlx::query(
            "SELECT label, reference, active, created_at FROM tenant_bindings \
             WHERE organization_id = $1 AND kind = 'signer' AND label = $2",
        )
        .bind(organization_id.as_uuid())
        .bind(format!("{}/{}", provider.as_str(), key_ref))
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("signer binding read failed: {e}")))?;
        Ok(row.map(|r| SignerBindingRow {
            organization_id,
            provider,
            key_ref: r.get("reference"),
            active: r.get("active"),
            created_at: r.get("created_at"),
        }))
    }

    async fn signers(&self, organization_id: OrganizationId) -> BotResult<Vec<SignerBindingRow>> {
        let rows = sqlx::query(
            "SELECT label, reference, active, created_at FROM tenant_bindings \
             WHERE organization_id = $1 AND kind = 'signer' ORDER BY label",
        )
        .bind(organization_id.as_uuid())
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("signer binding scan failed: {e}")))?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                // The stored label is "provider/key_ref" (see put_signer).
                let label: String = r.get("label");
                let (provider, _) = label.split_once('/')?;
                let provider = SignerProvider::parse(provider)?;
                Some(SignerBindingRow {
                    organization_id,
                    provider,
                    key_ref: r.get("reference"),
                    active: r.get("active"),
                    created_at: r.get("created_at"),
                })
            })
            .collect())
    }
}

/// In-memory registry (tests, detached deployments).
#[derive(Default)]
pub struct MemoryTenantBindingRegistry {
    wallets: tokio::sync::RwLock<HashMap<(OrganizationId, String), WalletBinding>>,
    signers: tokio::sync::RwLock<HashMap<(OrganizationId, String), SignerBindingRow>>,
}

impl MemoryTenantBindingRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        MemoryTenantBindingRegistry::default()
    }

    fn signer_key(
        _organization_id: OrganizationId,
        provider: SignerProvider,
        key_ref: &str,
    ) -> String {
        format!("{}/{}", provider.as_str(), key_ref)
    }
}

#[async_trait]
impl TenantBindingRegistry for MemoryTenantBindingRegistry {
    async fn put_wallet(&self, binding: WalletBinding) -> BotResult<()> {
        self.wallets
            .write()
            .await
            .insert((binding.organization_id, binding.label.clone()), binding);
        Ok(())
    }

    async fn wallet(
        &self,
        organization_id: OrganizationId,
        label: &str,
    ) -> BotResult<Option<WalletBinding>> {
        Ok(self
            .wallets
            .read()
            .await
            .get(&(organization_id, label.to_string()))
            .cloned())
    }

    async fn wallets(&self, organization_id: OrganizationId) -> BotResult<Vec<WalletBinding>> {
        let mut out: Vec<_> = self
            .wallets
            .read()
            .await
            .values()
            .filter(|b| b.organization_id == organization_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.label.cmp(&b.label));
        Ok(out)
    }

    async fn put_signer(&self, binding: SignerBindingRow) -> BotResult<()> {
        let key = Self::signer_key(binding.organization_id, binding.provider, &binding.key_ref);
        self.signers
            .write()
            .await
            .insert((binding.organization_id, key), binding);
        Ok(())
    }

    async fn signer(
        &self,
        organization_id: OrganizationId,
        provider: SignerProvider,
        key_ref: &str,
    ) -> BotResult<Option<SignerBindingRow>> {
        let key = Self::signer_key(organization_id, provider, key_ref);
        Ok(self
            .signers
            .read()
            .await
            .get(&(organization_id, key))
            .cloned())
    }

    async fn signers(&self, organization_id: OrganizationId) -> BotResult<Vec<SignerBindingRow>> {
        let mut out: Vec<_> = self
            .signers
            .read()
            .await
            .values()
            .filter(|b| b.organization_id == organization_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.key_ref.cmp(&b.key_ref));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallet(label: &str, org: OrganizationId, active: bool) -> WalletBinding {
        WalletBinding {
            organization_id: org,
            label: label.into(),
            address: format!("addr-{label}"),
            active,
            created_at: Utc::now(),
        }
    }

    fn signer(key_ref: &str, org: OrganizationId, active: bool) -> SignerBindingRow {
        SignerBindingRow {
            organization_id: org,
            provider: SignerProvider::Custody,
            key_ref: key_ref.into(),
            active,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn wallets_are_scoped_to_their_tenant() {
        let registry = MemoryTenantBindingRegistry::new();
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        registry.put_wallet(wallet("main", a, true)).await.unwrap();
        registry.put_wallet(wallet("main", b, true)).await.unwrap();

        // Same label, different tenants: distinct bindings, no leakage.
        let for_a = registry.wallet(a, "main").await.unwrap().unwrap();
        assert_eq!(for_a.address, "addr-main");
        assert_eq!(for_a.organization_id, a);
        assert_eq!(registry.wallets(a).await.unwrap().len(), 1);
        assert_eq!(registry.wallets(b).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unknown_labels_and_inactive_bindings_resolve_distinctly() {
        let registry = MemoryTenantBindingRegistry::new();
        let org = OrganizationId::new();
        registry
            .put_wallet(wallet("main", org, false))
            .await
            .unwrap();

        assert!(registry.wallet(org, "missing").await.unwrap().is_none());
        let inactive = registry.wallet(org, "main").await.unwrap().unwrap();
        assert!(!inactive.active, "the binding exists but is inactive");
    }

    #[tokio::test]
    async fn signer_keys_are_provider_scoped() {
        let registry = MemoryTenantBindingRegistry::new();
        let org = OrganizationId::new();
        registry
            .put_signer(signer("key-1", org, true))
            .await
            .unwrap();

        assert!(
            registry
                .signer(org, SignerProvider::External, "key-1")
                .await
                .unwrap()
                .is_none(),
            "the same key ref under a different provider is a different signer"
        );
        assert!(registry
            .signer(org, SignerProvider::Custody, "key-1")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn upserts_are_idempotent_per_label() {
        let registry = MemoryTenantBindingRegistry::new();
        let org = OrganizationId::new();
        registry
            .put_wallet(wallet("main", org, true))
            .await
            .unwrap();
        // Same org+label with a new address: replaces, never duplicates.
        let mut rotated = wallet("main", org, true);
        rotated.address = "addr-rotated".into();
        registry.put_wallet(rotated).await.unwrap();
        let wallets = registry.wallets(org).await.unwrap();
        assert_eq!(wallets.len(), 1);
        assert_eq!(wallets[0].address, "addr-rotated");

        registry
            .put_signer(signer("key-1", org, true))
            .await
            .unwrap();
        let mut deactivated = signer("key-1", org, false);
        deactivated.active = false;
        registry.put_signer(deactivated).await.unwrap();
        let signers = registry.signers(org).await.unwrap();
        assert_eq!(signers.len(), 1);
        assert!(!signers[0].active);
    }
}

/// The live signer-binding check the broadcast guard calls.
///
/// The guard's captured context carries `is_active()` from issue time, so a
/// binding revoked afterwards stays "active" forever in every module that was
/// already running. This adapter asks the DURABLE registry (`tenant_bindings`,
/// migration 0025) at broadcast time.
///
/// Fail-closed by construction: a missing row is a refusal (an unbound signer
/// must never sign), an inactive row is a refusal, and a registry error is a
/// refusal. Only an active row passes.
pub struct RegistrySignerProbe {
    registry: Arc<dyn TenantBindingRegistry>,
}

impl std::fmt::Debug for RegistrySignerProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RegistrySignerProbe")
    }
}

impl RegistrySignerProbe {
    /// Wrap the tenant binding registry.
    pub fn new(registry: Arc<dyn TenantBindingRegistry>) -> Self {
        RegistrySignerProbe { registry }
    }
}

#[async_trait]
impl solana_kit::tenant_broadcast_guard::SignerBindingProbe for RegistrySignerProbe {
    async fn signer_is_active(
        &self,
        organization_id: OrganizationId,
        provider: SignerProvider,
        key_ref: &str,
    ) -> Result<(), String> {
        match self
            .registry
            .signer(organization_id, provider, key_ref)
            .await
        {
            Ok(Some(row)) if row.active => Ok(()),
            Ok(Some(_)) => Err("binding_revoked".to_string()),
            Ok(None) => Err("binding_not_found".to_string()),
            Err(error) => Err(format!("binding lookup failed: {error}")),
        }
    }

    fn probe_name(&self) -> &'static str {
        "binding_registry"
    }
}

#[cfg(test)]
mod signer_probe_tests {
    use super::*;
    use std::sync::Mutex;

    /// A registry whose answer the test controls, plus a call counter.
    struct Scripted {
        row: Mutex<Option<SignerBindingRow>>,
        calls: Mutex<usize>,
        fail: Mutex<bool>,
    }

    #[async_trait]
    impl TenantBindingRegistry for Scripted {
        async fn put_wallet(&self, _b: WalletBinding) -> bot_core::error::BotResult<()> {
            Ok(())
        }
        async fn wallet(
            &self,
            _o: OrganizationId,
            _l: &str,
        ) -> bot_core::error::BotResult<Option<WalletBinding>> {
            Ok(None)
        }
        async fn wallets(
            &self,
            _o: OrganizationId,
        ) -> bot_core::error::BotResult<Vec<WalletBinding>> {
            Ok(vec![])
        }
        async fn put_signer(&self, _b: SignerBindingRow) -> bot_core::error::BotResult<()> {
            Ok(())
        }
        async fn signer(
            &self,
            _o: OrganizationId,
            _p: SignerProvider,
            _k: &str,
        ) -> bot_core::error::BotResult<Option<SignerBindingRow>> {
            *self.calls.lock().unwrap() += 1;
            if *self.fail.lock().unwrap() {
                return Err(bot_core::error::BotError::Http("binding store down".into()));
            }
            Ok(self.row.lock().unwrap().clone())
        }
        async fn signers(
            &self,
            _o: OrganizationId,
        ) -> bot_core::error::BotResult<Vec<SignerBindingRow>> {
            Ok(vec![])
        }
    }

    fn row(active: bool) -> SignerBindingRow {
        SignerBindingRow {
            organization_id: OrganizationId::new(),
            provider: SignerProvider::Local,
            key_ref: "copy-key".into(),
            active,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn only_an_active_binding_passes_and_everything_else_refuses() {
        use solana_kit::tenant_broadcast_guard::SignerBindingProbe;

        let scripted = Arc::new(Scripted {
            row: Mutex::new(Some(row(true))),
            calls: Mutex::new(0),
            fail: Mutex::new(false),
        });
        let probe = RegistrySignerProbe::new(scripted.clone());
        let org = OrganizationId::new();

        probe
            .signer_is_active(org, SignerProvider::Local, "copy-key")
            .await
            .expect("an active binding passes");

        *scripted.row.lock().unwrap() = Some(row(false));
        assert_eq!(
            probe
                .signer_is_active(org, SignerProvider::Local, "copy-key")
                .await
                .unwrap_err(),
            "binding_revoked"
        );

        // A missing binding is refused: unbound never signs.
        *scripted.row.lock().unwrap() = None;
        assert_eq!(
            probe
                .signer_is_active(org, SignerProvider::Local, "copy-key")
                .await
                .unwrap_err(),
            "binding_not_found"
        );

        // A registry failure is refused, not assumed fine.
        *scripted.fail.lock().unwrap() = true;
        let err = probe
            .signer_is_active(org, SignerProvider::Local, "copy-key")
            .await
            .unwrap_err();
        assert!(err.contains("binding lookup failed"), "{err}");

        assert_eq!(*scripted.calls.lock().unwrap(), 4, "every call is a lookup");
        assert_eq!(probe.probe_name(), "binding_registry");
    }
}
