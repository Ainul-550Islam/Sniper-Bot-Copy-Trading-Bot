//! Tenant (organization) domain — the SaaS ownership root (TASK 7A file 02).
//!
//! Before TASK 7A every resource belonged implicitly to "the deployment".
//! This module introduces the explicit owner that the control plane needs:
//! a typed [`OrganizationId`] that every SaaS-owned row carries and every
//! authorization decision resolves to.
//!
//! | file | concern |
//! |---|---|
//! | `model.rs` | typed identifiers, [`Organization`], [`User`], lifecycle vocabularies |
//! | `policy.rs` | PURE tenant policy: may a tenant in this state take this action, and does this resource belong to it |
//!
//! Authorization rules do NOT live here. Roles and permissions are
//! [`crate::membership`]; the combined decision (tenant + role + permission
//! + entitlement) is [`crate::authorization`]. This module answers only:
//!
//! "who owns it" and "is the tenant itself allowed to act".
//!
//! # Not a source of truth for trading
//!
//! Orders, fills, positions, risk, the ledger, HA leases and feed cursors
//! remain owned by TASK 1–6 and their tables. Tenancy adds a *who* column
//! and a gate in front; it never becomes the record of *what happened*.

pub mod model;
pub mod module_kind;
pub mod policy;
pub mod runtime_generation;
pub mod runtime_id;
pub mod tenant_entitlement;
pub mod tenant_id;
pub mod tenant_module_state;
pub mod tenant_signer_ref;
pub mod tenant_state;
pub mod tenant_wallet_ref;

pub use model::{
    MembershipId, Organization, OrganizationId, OrganizationStatus, TenantId, TenantIdError, User,
    UserId, UserProfile, UserStatus,
};
pub use policy::{
    can_authenticate, check as check_tenant, check_ownership, check_resource, check_status,
    TenantAction, TenantDenyReason, TenantVerdict,
};

// STEP 3 — the tenant runtime-identity foundation. Each type reuses the
// canonical ids above; none of them duplicates an existing concept.
pub use module_kind::{
    can_trade, feature_key, parse_module, ModuleKind, ALL_MODULES, TRADING_MODULES,
};
pub use runtime_generation::{FenceOrdering, RuntimeGeneration, FIRST_GENERATION};
pub use runtime_id::RuntimeId;
pub use tenant_entitlement::{EntitlementDenyReason, EntitlementVerdict, TenantEntitlementView};
pub use tenant_id::{is_valid_tenant_id, parse_tenant_id, require_valid_tenant_id, TenantIdentity};
pub use tenant_module_state::{
    ModuleDisableReason, ModuleEnablement, ModuleVerdict, TenantModuleSet, TenantModuleState,
};
pub use tenant_signer_ref::{SignerProvider, TenantSignerRef, MAX_KEY_REF_LEN};
pub use tenant_state::TenantStateSnapshot;
pub use tenant_wallet_ref::{TenantWalletRef, MAX_ADDRESS_LEN};

use async_trait::async_trait;

use crate::error::BotResult;

/// Durable tenant storage (implemented over PostgreSQL by the server;
/// in-memory in tests).
///
/// Every lookup that can return another tenant's data takes the acting
/// [`OrganizationId`] explicitly, so a caller cannot "forget" the tenant
/// scope and read across tenants by accident.
#[async_trait]
pub trait TenantStore: Send + Sync {
    /// Insert a new organization. Fails when the slug is taken.
    async fn create_organization(&self, org: &Organization) -> BotResult<()>;

    /// One organization by id.
    async fn organization(&self, id: OrganizationId) -> BotResult<Option<Organization>>;

    /// One organization by slug (login / routing).
    async fn organization_by_slug(&self, slug: &str) -> BotResult<Option<Organization>>;

    /// Persist a changed organization (name, status, suspension fields).
    async fn update_organization(&self, org: &Organization) -> BotResult<()>;

    /// Insert a new user. Fails when the email is taken.
    async fn create_user(&self, user: &User) -> BotResult<()>;

    /// One user by id.
    async fn user(&self, id: UserId) -> BotResult<Option<User>>;

    /// One user by (normalised) email.
    async fn user_by_email(&self, email: &str) -> BotResult<Option<User>>;

    /// Persist a changed user (profile fields, status, last login).
    async fn update_user(&self, user: &User) -> BotResult<()>;
}
