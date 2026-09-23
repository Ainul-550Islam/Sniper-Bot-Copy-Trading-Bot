//! SaaS control-plane store.
//!
//! Production startup uses [`SaasStore::with_database`], which makes the
//! PostgreSQL projection in migration 0018 authoritative for every read and
//! writes it before updating the local mirror. This gives restarts and
//! multiple replicas the same users, organizations, memberships, sessions,
//! API keys, plans, subscriptions, entitlements, usage events, and
//! provisioning jobs. [`SaasStore::new`] deliberately remains an in-memory
//! implementation for unit tests and database-disabled fixtures.
//!
//! This is deliberately NOT a second source of truth for trading. Orders,
//! fills, positions, risk, the ledger, HA leases, and feed cursors stay
//! exactly where TASK 1–6 put them.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

use bot_core::billing::{
    default_catalogue, entitlements_from_plan, Entitlement, EntitlementSet, Plan, PlanCode, PlanId,
    Subscription, UsageEvent, UsageMetric,
};
use bot_core::db::Database;
use bot_core::error::{BotError, BotResult};
use bot_core::membership::{Membership, MembershipRole, PermissionSet};
use bot_core::provisioning::ProvisioningJob;
use bot_core::session::{SessionId, SessionRecord};
use bot_core::tenant::{Organization, OrganizationId, User, UserId};

use super::api_keys::SaasApiKey;
use super::postgres::{
    PostgresSaasRepo, API_KEY, ENTITLEMENT, JOB, MEMBERSHIP, ORGANIZATION, PLAN, SESSION,
    SUBSCRIPTION, USAGE, USER,
};

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
    repo: Option<Arc<PostgresSaasRepo>>,
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
            repo: None,
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

    /// Build the runtime store. With PostgreSQL attached, every operation
    /// uses the shared repository; without it, the exact in-memory semantics
    /// remain available for local development and unit tests.
    pub async fn with_database(db: Option<Arc<Database>>) -> BotResult<Self> {
        let mut store = Self::new();
        let Some(db) = db else {
            return Ok(store);
        };
        let repo = Arc::new(PostgresSaasRepo::new(db));
        let generated: Vec<Plan> = store.inner.read().await.plans.values().cloned().collect();
        let mut durable_plans = Vec::with_capacity(generated.len());
        for plan in generated {
            if let Some(existing) = repo.by_lookup(PLAN, plan.code.as_str()).await? {
                durable_plans.push(existing);
            } else if repo
                .insert(
                    PLAN,
                    &plan.id.to_string(),
                    None,
                    None,
                    Some(plan.code.as_str()),
                    &plan,
                )
                .await?
            {
                durable_plans.push(plan);
            } else {
                durable_plans.push(
                    repo.by_lookup(PLAN, plan.code.as_str())
                        .await?
                        .ok_or_else(|| BotError::db("plan seed raced without a readable winner"))?,
                );
            }
        }
        {
            let mut inner = store.inner.write().await;
            inner.plans.clear();
            inner.plans_by_code.clear();
            for plan in durable_plans {
                inner.plans_by_code.insert(plan.code, plan.id);
                inner.plans.insert(plan.id, plan);
            }
        }
        store.repo = Some(repo);
        Ok(store)
    }

    /// Whether PostgreSQL is authoritative for this store.
    pub fn is_durable(&self) -> bool {
        self.repo.is_some()
    }

    /// Shared in-memory handle for tests and API router fixtures.
    pub fn shared() -> Arc<SaasStore> {
        Arc::new(SaasStore::new())
    }

    // ------------------------------------------------------------- users --

    /// Insert a user; the email must be free.
    pub async fn create_user(&self, user: &User) -> BotResult<()> {
        let email = User::normalize_email(&user.email);
        if let Some(repo) = &self.repo {
            if !repo
                .insert(
                    USER,
                    &user.id.to_string(),
                    None,
                    Some(user.id),
                    Some(&email),
                    user,
                )
                .await?
            {
                return Err(BotError::invalid(
                    "an account with that email already exists",
                ));
            }
        } else if self.inner.read().await.users_by_email.contains_key(&email) {
            return Err(BotError::invalid(
                "an account with that email already exists",
            ));
        }
        let mut inner = self.inner.write().await;
        inner.users_by_email.insert(email, user.id);
        inner.users.insert(user.id, user.clone());
        Ok(())
    }

    /// One user by id. Database errors fail closed as no identity.
    pub async fn user(&self, id: UserId) -> Option<User> {
        if let Some(repo) = &self.repo {
            return repo.by_id(USER, &id.to_string()).await.ok().flatten();
        }
        self.inner.read().await.users.get(&id).cloned()
    }

    /// One user by email. Database errors fail closed as no identity.
    pub async fn user_by_email(&self, email: &str) -> Option<User> {
        let email = User::normalize_email(email);
        if let Some(repo) = &self.repo {
            return repo.by_lookup(USER, &email).await.ok().flatten();
        }
        let inner = self.inner.read().await;
        let id = inner.users_by_email.get(&email)?;
        inner.users.get(id).cloned()
    }

    /// Persist a changed user.
    pub async fn update_user(&self, user: &User) -> BotResult<()> {
        let email = User::normalize_email(&user.email);
        if let Some(repo) = &self.repo {
            if !repo
                .update(
                    USER,
                    &user.id.to_string(),
                    None,
                    Some(user.id),
                    Some(&email),
                    user,
                )
                .await?
            {
                return Err(BotError::NotFound(format!("user {}", user.id)));
            }
        } else if !self.inner.read().await.users.contains_key(&user.id) {
            return Err(BotError::NotFound(format!("user {}", user.id)));
        }
        let mut inner = self.inner.write().await;
        inner.users_by_email.retain(|_, id| *id != user.id);
        inner.users_by_email.insert(email, user.id);
        inner.users.insert(user.id, user.clone());
        Ok(())
    }

    // ----------------------------------------------------- organizations --

    /// Insert an organization; the slug must be free.
    pub async fn create_organization(&self, org: &Organization) -> BotResult<()> {
        let slug = org.slug.trim().to_ascii_lowercase();
        if let Some(repo) = &self.repo {
            if !repo
                .insert(
                    ORGANIZATION,
                    &org.id.to_string(),
                    Some(org.id),
                    org.created_by,
                    Some(&slug),
                    org,
                )
                .await?
            {
                return Err(BotError::invalid("that organization slug is taken"));
            }
        } else if self.inner.read().await.orgs_by_slug.contains_key(&slug) {
            return Err(BotError::invalid("that organization slug is taken"));
        }
        let mut inner = self.inner.write().await;
        inner.orgs_by_slug.insert(slug, org.id);
        inner.organizations.insert(org.id, org.clone());
        Ok(())
    }

    /// One organization by id.
    pub async fn organization(&self, id: OrganizationId) -> Option<Organization> {
        if let Some(repo) = &self.repo {
            return repo
                .by_id(ORGANIZATION, &id.to_string())
                .await
                .ok()
                .flatten();
        }
        self.inner.read().await.organizations.get(&id).cloned()
    }

    /// One organization by slug.
    pub async fn organization_by_slug(&self, slug: &str) -> Option<Organization> {
        let slug = slug.trim().to_ascii_lowercase();
        if let Some(repo) = &self.repo {
            return repo.by_lookup(ORGANIZATION, &slug).await.ok().flatten();
        }
        let inner = self.inner.read().await;
        let id = inner.orgs_by_slug.get(&slug)?;
        inner.organizations.get(id).cloned()
    }

    /// Persist a changed organization.
    pub async fn update_organization(&self, org: &Organization) -> BotResult<()> {
        let slug = org.slug.trim().to_ascii_lowercase();
        if let Some(repo) = &self.repo {
            if !repo
                .update(
                    ORGANIZATION,
                    &org.id.to_string(),
                    Some(org.id),
                    org.created_by,
                    Some(&slug),
                    org,
                )
                .await?
            {
                return Err(BotError::NotFound(format!("organization {}", org.id)));
            }
        } else if !self.inner.read().await.organizations.contains_key(&org.id) {
            return Err(BotError::NotFound(format!("organization {}", org.id)));
        }
        let mut inner = self.inner.write().await;
        inner.orgs_by_slug.retain(|_, id| *id != org.id);
        inner.orgs_by_slug.insert(slug, org.id);
        inner.organizations.insert(org.id, org.clone());
        Ok(())
    }

    // ------------------------------------------------------- memberships --

    /// Insert a membership; the pair must be free.
    pub async fn create_membership(&self, m: &Membership) -> BotResult<()> {
        let lookup = format!("{}:{}", m.organization_id, m.user_id);
        if let Some(repo) = &self.repo {
            if !repo
                .insert(
                    MEMBERSHIP,
                    &m.id.to_string(),
                    Some(m.organization_id),
                    Some(m.user_id),
                    Some(&lookup),
                    m,
                )
                .await?
            {
                return Err(BotError::invalid("that user is already a member"));
            }
        } else if self
            .inner
            .read()
            .await
            .memberships
            .contains_key(&(m.organization_id, m.user_id))
        {
            return Err(BotError::invalid("that user is already a member"));
        }
        self.inner
            .write()
            .await
            .memberships
            .insert((m.organization_id, m.user_id), m.clone());
        Ok(())
    }

    /// The membership of one user in one organization.
    pub async fn membership(
        &self,
        organization_id: OrganizationId,
        user_id: UserId,
    ) -> Option<Membership> {
        if let Some(repo) = &self.repo {
            let lookup = format!("{organization_id}:{user_id}");
            return repo.by_lookup(MEMBERSHIP, &lookup).await.ok().flatten();
        }
        self.inner
            .read()
            .await
            .memberships
            .get(&(organization_id, user_id))
            .cloned()
    }

    /// Every member of one organization (tenant-scoped by construction).
    pub async fn members(&self, organization_id: OrganizationId) -> Vec<Membership> {
        let mut v: Vec<Membership> = if let Some(repo) = &self.repo {
            repo.by_organization(MEMBERSHIP, organization_id)
                .await
                .unwrap_or_default()
        } else {
            self.inner
                .read()
                .await
                .memberships
                .values()
                .filter(|m| m.organization_id == organization_id)
                .cloned()
                .collect()
        };
        v.sort_by_key(|m| m.created_at);
        v
    }

    /// Every organization one user belongs to.
    pub async fn memberships_of_user(&self, user_id: UserId) -> Vec<Membership> {
        let mut v: Vec<Membership> = if let Some(repo) = &self.repo {
            repo.by_user(MEMBERSHIP, user_id).await.unwrap_or_default()
        } else {
            self.inner
                .read()
                .await
                .memberships
                .values()
                .filter(|m| m.user_id == user_id)
                .cloned()
                .collect()
        };
        v.sort_by_key(|m| m.created_at);
        v
    }

    /// Persist a changed membership.
    pub async fn update_membership(&self, m: &Membership) -> BotResult<()> {
        let lookup = format!("{}:{}", m.organization_id, m.user_id);
        if let Some(repo) = &self.repo {
            if !repo
                .update(
                    MEMBERSHIP,
                    &m.id.to_string(),
                    Some(m.organization_id),
                    Some(m.user_id),
                    Some(&lookup),
                    m,
                )
                .await?
            {
                return Err(BotError::NotFound("membership".into()));
            }
        } else if !self
            .inner
            .read()
            .await
            .memberships
            .contains_key(&(m.organization_id, m.user_id))
        {
            return Err(BotError::NotFound("membership".into()));
        }
        self.inner
            .write()
            .await
            .memberships
            .insert((m.organization_id, m.user_id), m.clone());
        Ok(())
    }

    // ---------------------------------------------------------- sessions --

    /// Persist a new session.
    pub async fn create_session(&self, s: &SessionRecord) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            if !repo
                .insert(
                    SESSION,
                    &s.id.to_string(),
                    s.organization_id,
                    Some(s.user_id),
                    Some(&s.token_hash),
                    s,
                )
                .await?
            {
                return Err(BotError::invalid("that session already exists"));
            }
        }
        let mut inner = self.inner.write().await;
        inner.sessions_by_hash.insert(s.token_hash.clone(), s.id);
        inner.sessions.insert(s.id, s.clone());
        Ok(())
    }

    /// Look a session up by token hash (the plaintext never reaches here).
    pub async fn session_by_hash(&self, token_hash: &str) -> Option<SessionRecord> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(SESSION, token_hash).await.ok().flatten();
        }
        let inner = self.inner.read().await;
        let id = inner.sessions_by_hash.get(token_hash)?;
        inner.sessions.get(id).cloned()
    }

    /// Persist a changed session.
    pub async fn update_session(&self, s: &SessionRecord) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            if !repo
                .update(
                    SESSION,
                    &s.id.to_string(),
                    s.organization_id,
                    Some(s.user_id),
                    Some(&s.token_hash),
                    s,
                )
                .await?
            {
                return Err(BotError::NotFound(format!("session {}", s.id)));
            }
        }
        self.inner.write().await.sessions.insert(s.id, s.clone());
        Ok(())
    }

    /// Every session of one user, newest first (the "active devices" list
    /// and the logout path).
    pub async fn sessions_of_user(&self, user_id: UserId) -> Vec<SessionRecord> {
        let mut v: Vec<SessionRecord> = if let Some(repo) = &self.repo {
            repo.by_user(SESSION, user_id).await.unwrap_or_default()
        } else {
            self.inner
                .read()
                .await
                .sessions
                .values()
                .filter(|s| s.user_id == user_id)
                .cloned()
                .collect()
        };
        v.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        v
    }

    /// Revoke every session of one user; returns how many changed.
    pub async fn revoke_user_sessions(
        &self,
        user_id: UserId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> usize {
        let mut sessions = self.sessions_of_user(user_id).await;
        let mut n = 0;
        for session in &mut sessions {
            if session.revoke(reason, now) && self.update_session(session).await.is_ok() {
                n += 1;
            }
        }
        n
    }

    // --------------------------------------------------------- api keys --

    /// Persist a new API key record (hash only).
    pub async fn create_api_key(&self, key: &SaasApiKey) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            if !repo
                .insert(
                    API_KEY,
                    &key.id.to_string(),
                    Some(key.organization_id),
                    key.created_by,
                    Some(&key.secret_hash),
                    key,
                )
                .await?
            {
                return Err(BotError::invalid("that key already exists"));
            }
        } else if self
            .inner
            .read()
            .await
            .api_keys
            .contains_key(&key.secret_hash)
        {
            return Err(BotError::invalid("that key already exists"));
        }
        self.inner
            .write()
            .await
            .api_keys
            .insert(key.secret_hash.clone(), key.clone());
        Ok(())
    }

    /// Look a key up by the hash of the presented secret.
    pub async fn api_key_by_hash(&self, secret_hash: &str) -> Option<SaasApiKey> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(API_KEY, secret_hash).await.ok().flatten();
        }
        self.inner.read().await.api_keys.get(secret_hash).cloned()
    }

    /// Every key of one organization, newest first. Tenant-scoped.
    pub async fn api_keys_of(&self, organization_id: OrganizationId) -> Vec<SaasApiKey> {
        let mut v: Vec<SaasApiKey> = if let Some(repo) = &self.repo {
            repo.by_organization(API_KEY, organization_id)
                .await
                .unwrap_or_default()
        } else {
            self.inner
                .read()
                .await
                .api_keys
                .values()
                .filter(|k| k.organization_id == organization_id)
                .cloned()
                .collect()
        };
        v.sort_by_key(|row| std::cmp::Reverse(row.created_at));
        v
    }

    /// Persist a changed key (revocation, last-used).
    pub async fn update_api_key(&self, key: &SaasApiKey) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            if !repo
                .update(
                    API_KEY,
                    &key.id.to_string(),
                    Some(key.organization_id),
                    key.created_by,
                    Some(&key.secret_hash),
                    key,
                )
                .await?
            {
                return Err(BotError::NotFound(format!("api key {}", key.key_prefix)));
            }
        } else if !self
            .inner
            .read()
            .await
            .api_keys
            .contains_key(&key.secret_hash)
        {
            return Err(BotError::NotFound(format!("api key {}", key.key_prefix)));
        }
        self.inner
            .write()
            .await
            .api_keys
            .insert(key.secret_hash.clone(), key.clone());
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
        for key in keys {
            if let Some(repo) = &self.repo {
                if repo
                    .upsert(
                        API_KEY,
                        &key.id.to_string(),
                        Some(key.organization_id),
                        key.created_by,
                        Some(&key.secret_hash),
                        &key,
                    )
                    .await
                    .is_err()
                {
                    continue;
                }
            }
            inner.api_keys.insert(key.secret_hash.clone(), key);
            n += 1;
        }
        n
    }

    // ---------------------------------------------------------- billing --

    /// One plan by code.
    pub async fn plan_by_code(&self, code: PlanCode) -> Option<Plan> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(PLAN, code.as_str()).await.ok().flatten();
        }
        let inner = self.inner.read().await;
        let id = inner.plans_by_code.get(&code)?;
        inner.plans.get(id).cloned()
    }

    /// One plan by id.
    pub async fn plan(&self, id: PlanId) -> Option<Plan> {
        if let Some(repo) = &self.repo {
            return repo.by_id(PLAN, &id.to_string()).await.ok().flatten();
        }
        self.inner.read().await.plans.get(&id).cloned()
    }

    /// The whole catalogue, weakest tier first.
    pub async fn plans(&self) -> Vec<Plan> {
        let mut v: Vec<Plan> = if let Some(repo) = &self.repo {
            repo.all(PLAN).await.unwrap_or_default()
        } else {
            self.inner.read().await.plans.values().cloned().collect()
        };
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
        if let Some(repo) = &self.repo {
            repo.assign_plan(&sub, &rows).await?;
        }
        let mut inner = self.inner.write().await;
        inner.subscriptions.insert(organization_id, sub.clone());
        inner.entitlements.insert(organization_id, rows);
        Ok(sub)
    }

    /// The tenant's subscription.
    pub async fn subscription_of(&self, organization_id: OrganizationId) -> Option<Subscription> {
        if let Some(repo) = &self.repo {
            return repo
                .by_lookup(SUBSCRIPTION, &organization_id.to_string())
                .await
                .ok()
                .flatten();
        }
        self.inner
            .read()
            .await
            .subscriptions
            .get(&organization_id)
            .cloned()
    }

    /// Persist a changed subscription.
    pub async fn update_subscription(&self, sub: &Subscription) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            repo.upsert(
                SUBSCRIPTION,
                &sub.id.to_string(),
                Some(sub.organization_id),
                None,
                Some(&sub.organization_id.to_string()),
                sub,
            )
            .await?;
        }
        self.inner
            .write()
            .await
            .subscriptions
            .insert(sub.organization_id, sub.clone());
        Ok(())
    }

    /// Upsert one entitlement row (operator override, trial).
    pub async fn upsert_entitlement(&self, ent: &Entitlement) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            let lookup = format!(
                "{}:{}:{}",
                ent.organization_id,
                ent.feature,
                ent.source.as_str()
            );
            repo.upsert(
                ENTITLEMENT,
                &ent.id.to_string(),
                Some(ent.organization_id),
                None,
                Some(&lookup),
                ent,
            )
            .await?;
        }
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
        if let Some(repo) = &self.repo {
            let sub: Option<Subscription> = repo
                .by_lookup(SUBSCRIPTION, &organization_id.to_string())
                .await
                .ok()
                .flatten();
            let plan = match &sub {
                Some(subscription) => repo
                    .by_id(PLAN, &subscription.plan_id.to_string())
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let rows: Vec<Entitlement> = repo
                .by_organization(ENTITLEMENT, organization_id)
                .await
                .unwrap_or_default();
            return EntitlementSet::resolve(plan.as_ref(), sub.as_ref(), &rows, now);
        }
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
        let key = (event.organization_id, event.idempotency_key.clone());
        if let Some(repo) = &self.repo {
            let lookup = format!("{}:{}", event.organization_id, event.idempotency_key);
            if !repo
                .insert(
                    USAGE,
                    &event.id.to_string(),
                    Some(event.organization_id),
                    None,
                    Some(&lookup),
                    event,
                )
                .await?
            {
                return Ok(false);
            }
        } else if !self.inner.write().await.usage_seen.insert(key.clone()) {
            return Ok(false);
        }
        let mut inner = self.inner.write().await;
        inner.usage_seen.insert(key);
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
        let events: Vec<UsageEvent> = if let Some(repo) = &self.repo {
            repo.by_organization(USAGE, organization_id)
                .await
                .unwrap_or_default()
        } else {
            self.inner.read().await.usage.clone()
        };
        events
            .iter()
            .filter(|e| {
                e.organization_id == organization_id && e.metric == metric && e.period() == period
            })
            .map(|e| e.quantity)
            .sum()
    }

    // ----------------------------------------------------- provisioning --

    /// Insert a job, or return the existing one with the same request key.
    pub async fn upsert_job(&self, job: &ProvisioningJob) -> BotResult<ProvisioningJob> {
        if let Some(repo) = &self.repo {
            if let Some(existing) = repo.by_lookup(JOB, &job.request_key).await? {
                return Ok(existing);
            }
            if !repo
                .insert(
                    JOB,
                    &job.id.to_string(),
                    job.organization_id,
                    job.user_id,
                    Some(&job.request_key),
                    job,
                )
                .await?
            {
                return repo.by_lookup(JOB, &job.request_key).await?.ok_or_else(|| {
                    BotError::db("provisioning job conflict without a readable winner")
                });
            }
        }
        let mut inner = self.inner.write().await;
        Ok(inner
            .jobs
            .entry(job.request_key.clone())
            .or_insert_with(|| job.clone())
            .clone())
    }

    /// One job by request key.
    pub async fn job_by_request_key(&self, request_key: &str) -> Option<ProvisioningJob> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(JOB, request_key).await.ok().flatten();
        }
        self.inner.read().await.jobs.get(request_key).cloned()
    }

    /// Persist a changed job.
    pub async fn update_job(&self, job: &ProvisioningJob) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            repo.upsert(
                JOB,
                &job.id.to_string(),
                job.organization_id,
                job.user_id,
                Some(&job.request_key),
                job,
            )
            .await?;
        }
        self.inner
            .write()
            .await
            .jobs
            .insert(job.request_key.clone(), job.clone());
        Ok(())
    }

    /// Jobs a worker may resume, oldest first.
    pub async fn resumable_jobs(&self, limit: usize) -> Vec<ProvisioningJob> {
        let mut v: Vec<ProvisioningJob> = if let Some(repo) = &self.repo {
            repo.all(JOB).await.unwrap_or_default()
        } else {
            self.inner.read().await.jobs.values().cloned().collect()
        }
        .into_iter()
        .filter(|j| j.state.is_resumable())
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

#[cfg(test)]
mod postgres_tests {
    use super::*;
    use bot_core::config::DatabaseConfig;

    /// A second process must reuse the durable catalogue rather than trying
    /// to seed new random plan ids into the unique plan-code keys.
    #[tokio::test]
    async fn durable_store_restarts_and_replica_catalogues_converge() {
        let Ok(url) = std::env::var("POSTGRES_URL") else {
            eprintln!("skipped: POSTGRES_URL is not set");
            return;
        };
        let cfg = DatabaseConfig {
            enabled: true,
            required: true,
            auto_migrate: true,
            ..DatabaseConfig::default()
        };
        let db = Arc::new(Database::connect(&cfg, &url).await.expect("connect"));
        db.migrate().await.expect("migrate");

        let first = SaasStore::with_database(Some(db.clone()))
            .await
            .expect("first replica");
        let second = SaasStore::with_database(Some(db.clone()))
            .await
            .expect("restart/second replica");
        assert!(first.is_durable());
        assert!(second.is_durable());
        for code in PlanCode::ALL {
            let a = first.plan_by_code(code).await.expect("first plan");
            let b = second.plan_by_code(code).await.expect("second plan");
            assert_eq!(a.id, b.id);
            assert_eq!(a.limits, b.limits);
        }

        let organization_id = OrganizationId::new();
        let plan = first
            .plan_by_code(PlanCode::Business)
            .await
            .expect("business plan");
        let now = Utc::now();
        let subscription = first
            .assign_plan(organization_id, PlanCode::Business, now)
            .await
            .expect("atomic plan assignment");
        assert_eq!(
            second
                .subscription_of(organization_id)
                .await
                .expect("replica subscription")
                .id,
            subscription.id
        );
        assert_eq!(
            second.entitlements_of(organization_id, now).await.len(),
            plan.limits.len()
        );

        // API-key restart guarantee (spec E/F/G): a key created before the
        // restart still authenticates afterwards — hash-only lookup, no
        // plaintext anywhere — a revocation committed by one replica stops
        // the key on the other, and an expired key is rejected with the
        // stable reason.
        let created = super::super::api_keys::build_api_key(
            organization_id,
            "restart-proof",
            MembershipRole::Trader,
            Vec::new(),
            None,
            None,
            now,
        );
        first
            .create_api_key(&created.key)
            .await
            .expect("insert key");
        let key = second
            .api_key_by_hash(&created.key.secret_hash)
            .await
            .expect("key survives the restart");
        assert_eq!(key.id, created.key.id);
        assert_eq!(key.organization_id, organization_id);
        assert!(key.is_usable(now), "valid key is usable after restart");
        assert_eq!(key.rejection(now), None);

        let mut revoked = key.clone();
        assert!(revoked.revoke("operator request", now));
        first
            .update_api_key(&revoked)
            .await
            .expect("persist revoke");
        let seen_by_second = second
            .api_key_by_hash(&created.key.secret_hash)
            .await
            .expect("revoked record is still there");
        assert!(!seen_by_second.is_usable(now), "revoked key stops working");
        assert_eq!(seen_by_second.rejection(now), Some("api_key_revoked"));

        let expired = super::super::api_keys::build_api_key(
            organization_id,
            "expired-proof",
            MembershipRole::Viewer,
            Vec::new(),
            None,
            None,
            now,
        );
        let mut expired_record = expired.key;
        expired_record.expires_at = Some(now - chrono::Duration::seconds(1));
        first
            .create_api_key(&expired_record)
            .await
            .expect("insert expired key");
        let seen_expired = second
            .api_key_by_hash(&expired_record.secret_hash)
            .await
            .expect("expired record is there");
        assert!(!seen_expired.is_usable(now), "expired key stops working");
        assert_eq!(seen_expired.rejection(now), Some("api_key_expired"));

        db.close().await;
    }
}
