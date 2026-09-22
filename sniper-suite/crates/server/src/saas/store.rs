//! In-process SaaS control-plane store.
//!
//! TASK 7A defines the durable schema (migration 0017) and the repository
//! traits in `bot_core`. This module provides the implementation the server
//! runs against today: an in-memory store with exactly the same semantics —
//! unique email, unique slug, unique API-key secret hash, tenant-scoped
//! reads — so the whole control plane is testable without a database and a
//! single-node deployment works out of the box.
//!
//! It is deliberately NOT a second source of truth for trading: it holds
//! users, organizations, memberships, sessions, API keys, plans,
//! subscriptions, entitlements, usage and provisioning jobs. Orders, fills,
//! positions, risk, the ledger, HA leases and feed cursors stay exactly
//! where TASK 1–6 put them.
//!
//! # Restart behaviour
//!
//! A control-plane restart re-seeds the catalogue and reloads whatever the
//! durable layer holds. The API-key path is written so that a key created
//! at runtime is reconstructed from its durable record
//! ([`SaasStore::reload_api_keys`]), which is the fix for the previous
//! in-memory-only key behaviour.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use bot_core::billing::{
    default_catalogue, entitlements_from_plan, Entitlement, EntitlementSet, Plan, PlanCode, PlanId,
    Subscription, UsageEvent, UsageMetric,
};
use bot_core::error::{BotError, BotResult};
use bot_core::membership::{Membership, MembershipRole, PermissionSet};
use bot_core::provisioning::ProvisioningJob;
use bot_core::session::{SessionId, SessionRecord};
use bot_core::tenant::{Organization, OrganizationId, User, UserId};

use super::api_keys::SaasApiKey;

/// Everything the control plane persists.
#[derive(Default)]
struct Inner {
    users: HashMap<UserId, User>,
    users_by_email: HashMap<String, UserId>,
    organizations: HashMap<OrganizationId, Organization>,
    orgs_by_slug: HashMap<String, OrganizationId>,
    memberships: HashMap<(OrganizationId, UserId), Membership>,
    sessions: HashMap<SessionId, SessionRecord>,
    sessions_by_hash: HashMap<String, SessionId>,
    api_keys: HashMap<String, SaasApiKey>,
    plans: HashMap<PlanId, Plan>,
    plans_by_code: HashMap<PlanCode, PlanId>,
    subscriptions: HashMap<OrganizationId, Subscription>,
    entitlements: HashMap<OrganizationId, Vec<Entitlement>>,
    usage: Vec<UsageEvent>,
    usage_seen: std::collections::HashSet<(OrganizationId, String)>,
    jobs: HashMap<String, ProvisioningJob>,
}

/// The control-plane store.
pub struct SaasStore {
    inner: RwLock<Inner>,
}

impl Default for SaasStore {
    fn default() -> Self {
        SaasStore::new()
    }
}

impl SaasStore {
    /// An empty store with the default plan catalogue seeded.
    pub fn new() -> Self {
        let store = SaasStore {
            inner: RwLock::new(Inner::default()),
        };
        let now = Utc::now();
        {
            let mut inner = store.inner.blocking_lock_fallback();
            for plan in default_catalogue(now) {
                inner.plans_by_code.insert(plan.code, plan.id);
                inner.plans.insert(plan.id, plan);
            }
        }
        store
    }

    /// Shared handle.
    pub fn shared() -> Arc<SaasStore> {
        Arc::new(SaasStore::new())
    }

    // ------------------------------------------------------------- users --

    /// Insert a user; the email must be free.
    pub async fn create_user(&self, user: &User) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        let email = User::normalize_email(&user.email);
        if inner.users_by_email.contains_key(&email) {
            return Err(BotError::invalid("an account with that email already exists"));
        }
        inner.users_by_email.insert(email, user.id);
        inner.users.insert(user.id, user.clone());
        Ok(())
    }

    /// One user by id.
    pub async fn user(&self, id: UserId) -> Option<User> {
        self.inner.read().await.users.get(&id).cloned()
    }

    /// One user by email.
    pub async fn user_by_email(&self, email: &str) -> Option<User> {
        let inner = self.inner.read().await;
        let id = inner.users_by_email.get(&User::normalize_email(email))?;
        inner.users.get(id).cloned()
    }

    /// Persist a changed user.
    pub async fn update_user(&self, user: &User) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        if !inner.users.contains_key(&user.id) {
            return Err(BotError::NotFound(format!("user {}", user.id)));
        }
        inner.users.insert(user.id, user.clone());
        Ok(())
    }

    // ----------------------------------------------------- organizations --

    /// Insert an organization; the slug must be free.
    pub async fn create_organization(&self, org: &Organization) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        let slug = org.slug.trim().to_ascii_lowercase();
        if inner.orgs_by_slug.contains_key(&slug) {
            return Err(BotError::invalid("that organization slug is taken"));
        }
        inner.orgs_by_slug.insert(slug, org.id);
        inner.organizations.insert(org.id, org.clone());
        Ok(())
    }

    /// One organization by id.
    pub async fn organization(&self, id: OrganizationId) -> Option<Organization> {
        self.inner.read().await.organizations.get(&id).cloned()
    }

    /// One organization by slug.
    pub async fn organization_by_slug(&self, slug: &str) -> Option<Organization> {
        let inner = self.inner.read().await;
        let id = inner.orgs_by_slug.get(&slug.trim().to_ascii_lowercase())?;
        inner.organizations.get(id).cloned()
    }

    /// Persist a changed organization.
    pub async fn update_organization(&self, org: &Organization) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        if !inner.organizations.contains_key(&org.id) {
            return Err(BotError::NotFound(format!("organization {}", org.id)));
        }
        inner.organizations.insert(org.id, org.clone());
        Ok(())
    }

    // ------------------------------------------------------- memberships --

    /// Insert a membership; the pair must be free.
    pub async fn create_membership(&self, m: &Membership) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        let key = (m.organization_id, m.user_id);
        if inner.memberships.contains_key(&key) {
            return Err(BotError::invalid("that user is already a member"));
        }
        inner.memberships.insert(key, m.clone());
        Ok(())
    }

    /// The membership of one user in one organization.
    pub async fn membership(
        &self,
        organization_id: OrganizationId,
        user_id: UserId,
    ) -> Option<Membership> {
        self.inner
            .read()
            .await
            .memberships
            .get(&(organization_id, user_id))
            .cloned()
    }

    /// Every member of one organization (tenant-scoped by construction).
    pub async fn members(&self, organization_id: OrganizationId) -> Vec<Membership> {
        let mut v: Vec<Membership> = self
            .inner
            .read()
            .await
            .memberships
            .values()
            .filter(|m| m.organization_id == organization_id)
            .cloned()
            .collect();
        v.sort_by_key(|m| m.created_at);
        v
    }

    /// Every organization one user belongs to.
    pub async fn memberships_of_user(&self, user_id: UserId) -> Vec<Membership> {
        let mut v: Vec<Membership> = self
            .inner
            .read()
            .await
            .memberships
            .values()
            .filter(|m| m.user_id == user_id)
            .cloned()
            .collect();
        v.sort_by_key(|m| m.created_at);
        v
    }

    /// Persist a changed membership.
    pub async fn update_membership(&self, m: &Membership) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        let key = (m.organization_id, m.user_id);
        if !inner.memberships.contains_key(&key) {
            return Err(BotError::NotFound("membership".into()));
        }
        inner.memberships.insert(key, m.clone());
        Ok(())
    }

    // ---------------------------------------------------------- sessions --

    /// Persist a new session.
    pub async fn create_session(&self, s: &SessionRecord) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        inner.sessions_by_hash.insert(s.token_hash.clone(), s.id);
        inner.sessions.insert(s.id, s.clone());
        Ok(())
    }

    /// Look a session up by token hash (the plaintext never reaches here).
    pub async fn session_by_hash(&self, token_hash: &str) -> Option<SessionRecord> {
        let inner = self.inner.read().await;
        let id = inner.sessions_by_hash.get(token_hash)?;
        inner.sessions.get(id).cloned()
    }

    /// Persist a changed session.
    pub async fn update_session(&self, s: &SessionRecord) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        inner.sessions.insert(s.id, s.clone());
        Ok(())
    }

    /// Every session of one user, newest first (the "active devices" list
    /// and the logout path).
    pub async fn sessions_of_user(&self, user_id: UserId) -> Vec<SessionRecord> {
        let mut v: Vec<SessionRecord> = self
            .inner
            .read()
            .await
            .sessions
            .values()
            .filter(|s| s.user_id == user_id)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    /// Revoke every session of one user; returns how many changed.
    pub async fn revoke_user_sessions(
        &self,
        user_id: UserId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> usize {
        let mut inner = self.inner.write().await;
        let ids: Vec<SessionId> = inner
            .sessions
            .values()
            .filter(|s| s.user_id == user_id && s.revoked_at.is_none())
            .map(|s| s.id)
            .collect();
        let mut n = 0;
        for id in ids {
            if let Some(s) = inner.sessions.get_mut(&id) {
                if s.revoke(reason, now) {
                    n += 1;
                }
            }
        }
        n
    }

    // --------------------------------------------------------- api keys --

    /// Persist a new API key record (hash only).
    pub async fn create_api_key(&self, key: &SaasApiKey) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        if inner.api_keys.contains_key(&key.secret_hash) {
            return Err(BotError::invalid("that key already exists"));
        }
        inner.api_keys.insert(key.secret_hash.clone(), key.clone());
        Ok(())
    }

    /// Look a key up by the hash of the presented secret.
    pub async fn api_key_by_hash(&self, secret_hash: &str) -> Option<SaasApiKey> {
        self.inner.read().await.api_keys.get(secret_hash).cloned()
    }

    /// Every key of one organization, newest first. Tenant-scoped.
    pub async fn api_keys_of(&self, organization_id: OrganizationId) -> Vec<SaasApiKey> {
        let mut v: Vec<SaasApiKey> = self
            .inner
            .read()
            .await
            .api_keys
            .values()
            .filter(|k| k.organization_id == organization_id)
            .cloned()
            .collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    /// Persist a changed key (revocation, last-used).
    pub async fn update_api_key(&self, key: &SaasApiKey) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        if !inner.api_keys.contains_key(&key.secret_hash) {
            return Err(BotError::NotFound(format!("api key {}", key.key_prefix)));
        }
        inner.api_keys.insert(key.secret_hash.clone(), key.clone());
        Ok(())
    }

    /// Reload API keys from durable records at startup.
    ///
    /// This is what makes a runtime-created key survive a restart: the
    /// server hands the rows it read from `saas_api_keys` back to the
    /// in-process store, and authentication works again without the
    /// plaintext ever existing on disk.
    pub async fn reload_api_keys(&self, keys: Vec<SaasApiKey>) -> usize {
        let mut inner = self.inner.write().await;
        let mut n = 0;
        for k in keys {
            inner.api_keys.insert(k.secret_hash.clone(), k);
            n += 1;
        }
        n
    }

    // ---------------------------------------------------------- billing --

    /// One plan by code.
    pub async fn plan_by_code(&self, code: PlanCode) -> Option<Plan> {
        let inner = self.inner.read().await;
        let id = inner.plans_by_code.get(&code)?;
        inner.plans.get(id).cloned()
    }

    /// One plan by id.
    pub async fn plan(&self, id: PlanId) -> Option<Plan> {
        self.inner.read().await.plans.get(&id).cloned()
    }

    /// The whole catalogue, weakest tier first.
    pub async fn plans(&self) -> Vec<Plan> {
        let mut v: Vec<Plan> = self.inner.read().await.plans.values().cloned().collect();
        v.sort_by_key(|p| p.code);
        v
    }

    /// Assign a plan: create the subscription and its entitlement rows.
    pub async fn assign_plan(
        &self,
        organization_id: OrganizationId,
        code: PlanCode,
        now: DateTime<Utc>,
    ) -> BotResult<Subscription> {
        let plan = self
            .plan_by_code(code)
            .await
            .ok_or_else(|| BotError::NotFound(format!("plan {code}")))?;
        let sub = Subscription::manual(organization_id, plan.id, now);
        let rows = entitlements_from_plan(organization_id, &plan, now);
        let mut inner = self.inner.write().await;
        inner.subscriptions.insert(organization_id, sub.clone());
        inner.entitlements.insert(organization_id, rows);
        Ok(sub)
    }

    /// The tenant's subscription.
    pub async fn subscription_of(&self, organization_id: OrganizationId) -> Option<Subscription> {
        self.inner
            .read()
            .await
            .subscriptions
            .get(&organization_id)
            .cloned()
    }

    /// Persist a changed subscription.
    pub async fn update_subscription(&self, sub: &Subscription) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        inner.subscriptions.insert(sub.organization_id, sub.clone());
        Ok(())
    }

    /// Upsert one entitlement row (operator override, trial).
    pub async fn upsert_entitlement(&self, ent: &Entitlement) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        let rows = inner.entitlements.entry(ent.organization_id).or_default();
        if let Some(existing) = rows
            .iter_mut()
            .find(|r| r.feature == ent.feature && r.source == ent.source)
        {
            *existing = ent.clone();
        } else {
            rows.push(ent.clone());
        }
        Ok(())
    }

    /// The tenant's effective entitlements at `now`.
    pub async fn entitlements_of(
        &self,
        organization_id: OrganizationId,
        now: DateTime<Utc>,
    ) -> EntitlementSet {
        let inner = self.inner.read().await;
        let sub = inner.subscriptions.get(&organization_id);
        let plan = sub.and_then(|s| inner.plans.get(&s.plan_id));
        let rows = inner
            .entitlements
            .get(&organization_id)
            .cloned()
            .unwrap_or_default();
        EntitlementSet::resolve(plan, sub, &rows, now)
    }

    // ------------------------------------------------------------ usage --

    /// Record one usage event. `false` = the `(tenant, key)` pair was
    /// already counted.
    pub async fn record_usage(&self, event: &UsageEvent) -> BotResult<bool> {
        event.validate().map_err(BotError::invalid)?;
        let mut inner = self.inner.write().await;
        let key = (event.organization_id, event.idempotency_key.clone());
        if !inner.usage_seen.insert(key) {
            return Ok(false);
        }
        inner.usage.push(event.clone());
        Ok(true)
    }

    /// Total of one metric for one tenant in one `YYYY-MM` period.
    pub async fn usage_total(
        &self,
        organization_id: OrganizationId,
        metric: UsageMetric,
        period: &str,
    ) -> f64 {
        self.inner
            .read()
            .await
            .usage
            .iter()
            .filter(|e| {
                e.organization_id == organization_id && e.metric == metric && e.period() == period
            })
            .map(|e| e.quantity)
            .sum()
    }

    // ----------------------------------------------------- provisioning --

    /// Insert a job, or return the existing one with the same request key.
    pub async fn upsert_job(&self, job: &ProvisioningJob) -> ProvisioningJob {
        let mut inner = self.inner.write().await;
        inner
            .jobs
            .entry(job.request_key.clone())
            .or_insert_with(|| job.clone())
            .clone()
    }

    /// One job by request key.
    pub async fn job_by_request_key(&self, request_key: &str) -> Option<ProvisioningJob> {
        self.inner.read().await.jobs.get(request_key).cloned()
    }

    /// Persist a changed job.
    pub async fn update_job(&self, job: &ProvisioningJob) -> BotResult<()> {
        let mut inner = self.inner.write().await;
        inner.jobs.insert(job.request_key.clone(), job.clone());
        Ok(())
    }

    /// Jobs a worker may resume, oldest first.
    pub async fn resumable_jobs(&self, limit: usize) -> Vec<ProvisioningJob> {
        let mut v: Vec<ProvisioningJob> = self
            .inner
            .read()
            .await
            .jobs
            .values()
            .filter(|j| j.state.is_resumable())
            .cloned()
            .collect();
        v.sort_by_key(|j| j.created_at);
        v.truncate(limit);
        v
    }

    /// The role a tenant API key's scopes narrow to, resolved once so the
    /// middleware does not re-parse strings per request.
    pub fn scopes_of(key: &SaasApiKey) -> Option<PermissionSet> {
        if key.scopes.is_empty() {
            None
        } else {
            Some(PermissionSet::parse_list(&key.scopes))
        }
    }

    /// The role a legacy deployment credential maps to.
    pub fn legacy_role(role: bot_core::auth::Role) -> MembershipRole {
        MembershipRole::from_legacy(role)
    }
}

/// `RwLock` does not offer a blocking lock outside an async context in all
/// runtimes; the constructor needs one to seed the catalogue before any
/// task exists. This tiny helper keeps that contained.
trait BlockingLockFallback<T> {
    fn blocking_lock_fallback(&self) -> tokio::sync::RwLockWriteGuard<'_, T>;
}

impl<T> BlockingLockFallback<T> for RwLock<T> {
    fn blocking_lock_fallback(&self) -> tokio::sync::RwLockWriteGuard<'_, T> {
        // The store is brand new here: nobody else can hold the lock, so
        // `try_write` always succeeds and we never block a runtime thread.
        self.try_write()
            .expect("a freshly constructed store is never contended")
    }
}
