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

use bot_core::billing::payment::{PaymentId, PaymentTransaction};
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
    PostgresSaasRepo, API_KEY, ENTITLEMENT, JOB, MEMBERSHIP, ORGANIZATION, PAYMENT, PLAN, SESSION,
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
    payments: HashMap<PaymentId, PaymentTransaction>,
    payments_by_idem: HashMap<(OrganizationId, String), PaymentId>,
}

/// The control-plane store.
pub struct SaasStore {
    inner: RwLock<Inner>,
    repo: Option<Arc<PostgresSaasRepo>>,
    /// Whether identity writes (registration, login, invitations) may use the
    /// in-memory mirror without PostgreSQL. Off by default: a deployment
    /// without a database must answer 503 rather than report a success that
    /// vanishes on restart. Opt-in only via `SAAS_ALLOW_IN_MEMORY_IDENTITY=1`
    /// (local development) or [`SaasStore::new_in_memory_identity`] (tests).
    identity_in_memory: bool,
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
            identity_in_memory: false,
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
            store.identity_in_memory =
                std::env::var("SAAS_ALLOW_IN_MEMORY_IDENTITY").as_deref() == Ok("1");
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

    /// An in-memory store that accepts identity writes without PostgreSQL.
    /// For tests and explicit local development only; production deployments
    /// without a database refuse identity writes (see `identity_in_memory`).
    pub fn new_in_memory_identity() -> Self {
        let mut store = Self::new();
        store.identity_in_memory = true;
        store
    }

    /// Whether identity writes and logins may proceed: PostgreSQL is attached,
    /// or in-memory identity was explicitly enabled.
    pub fn allows_identity_writes(&self) -> bool {
        self.is_durable() || self.identity_in_memory
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
        }
        let mut inner = self.inner.write().await;
        if self.repo.is_none() && inner.users_by_email.contains_key(&email) {
            return Err(BotError::invalid(
                "an account with that email already exists",
            ));
        }
        inner.users_by_email.insert(email, user.id);
        inner.users.insert(user.id, user.clone());
        Ok(())
    }

    /// One user by id. Durable read failures are returned rather than
    /// converted into a missing identity.
    pub async fn user(&self, id: UserId) -> BotResult<Option<User>> {
        if let Some(repo) = &self.repo {
            return repo.by_id(USER, &id.to_string()).await;
        }
        Ok(self.inner.read().await.users.get(&id).cloned())
    }

    /// One user by email. Durable read failures are returned rather than
    /// converted into a missing identity.
    pub async fn user_by_email(&self, email: &str) -> BotResult<Option<User>> {
        let email = User::normalize_email(email);
        if let Some(repo) = &self.repo {
            return repo.by_lookup(USER, &email).await;
        }
        let inner = self.inner.read().await;
        let Some(id) = inner.users_by_email.get(&email) else {
            return Ok(None);
        };
        Ok(inner.users.get(id).cloned())
    }

    /// Persist only a profile display-name update, preserving the latest
    /// password, status, email-verification and other identity fields.
    pub async fn update_user(&self, user: &User) -> BotResult<User> {
        if let Some(repo) = &self.repo {
            let current = repo
                .update_user_display_name(user.id, &user.display_name, user.updated_at)
                .await?;
            let mut inner = self.inner.write().await;
            inner.users.insert(current.id, current.clone());
            return Ok(current);
        }
        let mut inner = self.inner.write().await;
        let Some(current) = inner.users.get_mut(&user.id) else {
            return Err(BotError::NotFound(format!("user {}", user.id)));
        };
        current.display_name = user.display_name.clone();
        current.updated_at = user.updated_at;
        Ok(current.clone())
    }

    /// Change a password and revoke all of that user's sessions atomically
    /// when PostgreSQL is authoritative. The in-memory path applies both
    /// mutations under one write lock.
    pub async fn update_user_and_revoke_sessions(
        &self,
        user: &User,
        expected_password_hash: &str,
        display_name: Option<&str>,
        reason: &str,
        now: DateTime<Utc>,
    ) -> BotResult<Option<(User, usize)>> {
        if let Some(repo) = &self.repo {
            let result = repo
                .update_user_and_revoke_sessions(
                    user,
                    expected_password_hash,
                    display_name,
                    reason,
                    now,
                )
                .await?;
            if let Some((current, _)) = &result {
                self.inner
                    .write()
                    .await
                    .users
                    .insert(current.id, current.clone());
            }
            return Ok(result);
        }

        let mut inner = self.inner.write().await;
        let Some(current) = inner.users.get_mut(&user.id) else {
            return Err(BotError::NotFound(format!("user {}", user.id)));
        };
        if current.password_hash != expected_password_hash || !current.can_authenticate() {
            return Ok(None);
        }
        current.password_hash = user.password_hash.clone();
        if let Some(display_name) = display_name {
            current.display_name = display_name.to_string();
        }
        current.updated_at = now;
        let current = current.clone();
        let mut revoked = 0usize;
        for session in inner
            .sessions
            .values_mut()
            .filter(|session| session.user_id == user.id)
        {
            if session.revoke(reason, now) {
                revoked += 1;
            }
        }
        Ok(Some((current, revoked)))
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

    /// One organization by id. Durable read failures are returned rather
    /// than converted into a missing tenant.
    pub async fn organization(&self, id: OrganizationId) -> BotResult<Option<Organization>> {
        if let Some(repo) = &self.repo {
            return repo.by_id(ORGANIZATION, &id.to_string()).await;
        }
        Ok(self.inner.read().await.organizations.get(&id).cloned())
    }

    /// One organization by slug. Durable read failures are returned rather
    /// than converted into a missing tenant.
    pub async fn organization_by_slug(&self, slug: &str) -> BotResult<Option<Organization>> {
        let slug = slug.trim().to_ascii_lowercase();
        if let Some(repo) = &self.repo {
            return repo.by_lookup(ORGANIZATION, &slug).await;
        }
        let inner = self.inner.read().await;
        let Some(id) = inner.orgs_by_slug.get(&slug) else {
            return Ok(None);
        };
        Ok(inner.organizations.get(id).cloned())
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

    /// The membership of one user in one organization. Durable read
    /// failures are returned rather than converted into no membership.
    pub async fn membership(
        &self,
        organization_id: OrganizationId,
        user_id: UserId,
    ) -> BotResult<Option<Membership>> {
        if let Some(repo) = &self.repo {
            let lookup = format!("{organization_id}:{user_id}");
            return repo.by_lookup(MEMBERSHIP, &lookup).await;
        }
        Ok(self
            .inner
            .read()
            .await
            .memberships
            .get(&(organization_id, user_id))
            .cloned())
    }

    /// Every member of one organization (tenant-scoped by construction).
    /// A durable read failure is returned to the caller; it is never
    /// represented as an empty tenant.
    pub async fn members(&self, organization_id: OrganizationId) -> BotResult<Vec<Membership>> {
        let mut v: Vec<Membership> = if let Some(repo) = &self.repo {
            repo.by_organization(MEMBERSHIP, organization_id).await?
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
        Ok(v)
    }

    /// Every organization one user belongs to.
    pub async fn memberships_of_user(&self, user_id: UserId) -> BotResult<Vec<Membership>> {
        let mut v: Vec<Membership> = if let Some(repo) = &self.repo {
            repo.by_user(MEMBERSHIP, user_id).await?
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
        Ok(v)
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

    /// Revalidate the password and persist an authenticated session and
    /// `last_login_at` atomically. PostgreSQL serializes this with password
    /// reset/change on the canonical user row; the in-memory adapter holds
    /// the same write lock for the whole state transition.
    pub async fn create_authenticated_session(
        &self,
        expected_password_hash: &str,
        session: &SessionRecord,
        now: DateTime<Utc>,
    ) -> BotResult<Option<User>> {
        if let Some(repo) = &self.repo {
            let Some(user) = repo
                .create_authenticated_session(expected_password_hash, session, now)
                .await?
            else {
                return Ok(None);
            };
            let mut inner = self.inner.write().await;
            inner.users_by_email.insert(user.email.clone(), user.id);
            inner.users.insert(user.id, user.clone());
            inner
                .sessions_by_hash
                .insert(session.token_hash.clone(), session.id);
            inner.sessions.insert(session.id, session.clone());
            return Ok(Some(user));
        }

        let mut inner = self.inner.write().await;
        if inner.sessions.contains_key(&session.id)
            || inner.sessions_by_hash.contains_key(&session.token_hash)
        {
            return Err(BotError::invalid("that session already exists"));
        }
        let Some(user) = inner.users.get_mut(&session.user_id) else {
            return Ok(None);
        };
        if user.password_hash != expected_password_hash || !user.can_authenticate() {
            return Ok(None);
        }
        user.last_login_at = Some(now);
        user.updated_at = now;
        let user = user.clone();
        inner
            .sessions_by_hash
            .insert(session.token_hash.clone(), session.id);
        inner.sessions.insert(session.id, session.clone());
        Ok(Some(user))
    }

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

    /// One session by id. Durable read failures are returned as errors, never
    /// converted into `None`.
    pub async fn session(&self, id: SessionId) -> BotResult<Option<SessionRecord>> {
        if let Some(repo) = &self.repo {
            return repo.by_id(SESSION, &id.to_string()).await;
        }
        Ok(self.inner.read().await.sessions.get(&id).cloned())
    }

    /// Look a session up by token hash (the plaintext never reaches here).
    /// Durable read failures are returned rather than converted into an
    /// unknown session.
    pub async fn session_by_hash(&self, token_hash: &str) -> BotResult<Option<SessionRecord>> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(SESSION, token_hash).await;
        }
        let inner = self.inner.read().await;
        let Some(id) = inner.sessions_by_hash.get(token_hash) else {
            return Ok(None);
        };
        Ok(inner.sessions.get(id).cloned())
    }

    /// Record a session use without replacing its authorization state. Returns
    /// false when the session was concurrently revoked, expired, or mismatched.
    pub async fn touch_session_last_seen(
        &self,
        session_id: SessionId,
        token_hash: &str,
        now: DateTime<Utc>,
    ) -> BotResult<bool> {
        if let Some(repo) = &self.repo {
            if !repo
                .touch_session_last_seen(&session_id.to_string(), token_hash, now)
                .await?
            {
                return Ok(false);
            }
            let mut inner = self.inner.write().await;
            if let Some(session) = inner.sessions.get_mut(&session_id) {
                if session.token_hash == token_hash {
                    session.last_seen_at = session.last_seen_at.max(now);
                }
            }
            return Ok(true);
        }

        let mut inner = self.inner.write().await;
        let Some(session) = inner.sessions.get_mut(&session_id) else {
            return Ok(false);
        };
        if session.token_hash != token_hash || !session.is_usable(now) {
            return Ok(false);
        }
        session.last_seen_at = session.last_seen_at.max(now);
        Ok(true)
    }

    /// Update only MFA proof/promotion fields on a live session.
    pub async fn set_session_mfa_state(
        &self,
        session_id: SessionId,
        token_hash: &str,
        policy_updated_at: Option<DateTime<Utc>>,
        enrollment_only: bool,
        now: DateTime<Utc>,
    ) -> BotResult<bool> {
        if let Some(repo) = &self.repo {
            if !repo
                .set_session_mfa_state(
                    &session_id.to_string(),
                    token_hash,
                    policy_updated_at,
                    enrollment_only,
                    now,
                )
                .await?
            {
                return Ok(false);
            }
            let mut inner = self.inner.write().await;
            if let Some(session) = inner.sessions.get_mut(&session_id) {
                if session.token_hash == token_hash {
                    session.mfa_policy_updated_at = policy_updated_at;
                    session.mfa_enrollment_only = enrollment_only;
                }
            }
            return Ok(true);
        }

        let mut inner = self.inner.write().await;
        let Some(session) = inner.sessions.get_mut(&session_id) else {
            return Ok(false);
        };
        if session.token_hash != token_hash || !session.is_usable(now) {
            return Ok(false);
        }
        session.mfa_policy_updated_at = policy_updated_at;
        session.mfa_enrollment_only = enrollment_only;
        Ok(true)
    }

    /// Revoke a session without writing back a stale full-record snapshot.
    pub async fn revoke_session(
        &self,
        session_id: SessionId,
        token_hash: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> BotResult<bool> {
        if let Some(repo) = &self.repo {
            if !repo
                .revoke_session(&session_id.to_string(), token_hash, reason, now)
                .await?
            {
                return Ok(false);
            }
            let mut inner = self.inner.write().await;
            if let Some(session) = inner.sessions.get_mut(&session_id) {
                if session.token_hash == token_hash {
                    session.revoke(reason, now);
                }
            }
            return Ok(true);
        }

        let mut inner = self.inner.write().await;
        let Some(session) = inner.sessions.get_mut(&session_id) else {
            return Ok(false);
        };
        if session.token_hash != token_hash {
            return Ok(false);
        }
        Ok(session.revoke(reason, now))
    }

    /// Every session of one user, newest first (the "active devices" list
    /// and the logout path). A durable read failure is returned rather than
    /// being interpreted as "the user has no sessions".
    pub async fn sessions_of_user(&self, user_id: UserId) -> BotResult<Vec<SessionRecord>> {
        let mut v: Vec<SessionRecord> = if let Some(repo) = &self.repo {
            repo.by_user(SESSION, user_id).await?
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
        Ok(v)
    }

    /// Revoke every session of one user; returns how many changed. A failed
    /// durable update aborts the operation instead of returning a partial
    /// success count.
    pub async fn revoke_user_sessions(
        &self,
        user_id: UserId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> BotResult<usize> {
        let sessions = self.sessions_of_user(user_id).await?;
        let mut n = 0;
        for session in sessions {
            if self
                .revoke_session(session.id, &session.token_hash, reason, now)
                .await?
            {
                n += 1;
            }
        }
        Ok(n)
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

    /// Look a key up by the hash of the presented secret. Durable read
    /// failures are returned rather than converted into an unknown key.
    pub async fn api_key_by_hash(&self, secret_hash: &str) -> BotResult<Option<SaasApiKey>> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(API_KEY, secret_hash).await;
        }
        Ok(self.inner.read().await.api_keys.get(secret_hash).cloned())
    }

    /// Every key of one organization, newest first. Tenant-scoped. A
    /// durable read failure is returned rather than an empty key list.
    pub async fn api_keys_of(&self, organization_id: OrganizationId) -> BotResult<Vec<SaasApiKey>> {
        let mut v: Vec<SaasApiKey> = if let Some(repo) = &self.repo {
            repo.by_organization(API_KEY, organization_id).await?
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
        Ok(v)
    }

    /// Revoke an API key once, preserving concurrent last-use telemetry and
    /// the first reason. Returns false for a missing, mismatched, or already
    /// revoked key.
    pub async fn revoke_api_key(
        &self,
        key_id: uuid::Uuid,
        secret_hash: &str,
        reason: &str,
        now: DateTime<Utc>,
    ) -> BotResult<bool> {
        if let Some(repo) = &self.repo {
            if !repo
                .revoke_api_key(&key_id.to_string(), secret_hash, reason, now)
                .await?
            {
                return Ok(false);
            }
            let mut inner = self.inner.write().await;
            if let Some(key) = inner.api_keys.get_mut(secret_hash) {
                if key.id == key_id {
                    key.revoke(reason, now);
                }
            }
            return Ok(true);
        }

        let mut inner = self.inner.write().await;
        let Some(key) = inner.api_keys.get_mut(secret_hash) else {
            return Ok(false);
        };
        if key.id != key_id {
            return Ok(false);
        }
        Ok(key.revoke(reason, now))
    }

    /// Record successful authentication without replacing any authorization
    /// fields from the snapshot used to authenticate. Returns false if the key
    /// was revoked, expired, or no longer matches the resolved identity.
    pub async fn touch_api_key_last_used(
        &self,
        key_id: uuid::Uuid,
        secret_hash: &str,
        now: DateTime<Utc>,
    ) -> BotResult<bool> {
        if let Some(repo) = &self.repo {
            if !repo
                .touch_api_key_last_used(&key_id.to_string(), secret_hash, now)
                .await?
            {
                return Ok(false);
            }
            let mut inner = self.inner.write().await;
            if let Some(key) = inner.api_keys.get_mut(secret_hash) {
                if key.id == key_id {
                    key.last_used_at = Some(key.last_used_at.map_or(now, |old| old.max(now)));
                }
            }
            return Ok(true);
        }

        let mut inner = self.inner.write().await;
        let Some(key) = inner.api_keys.get_mut(secret_hash) else {
            return Ok(false);
        };
        if key.id != key_id || !key.is_usable(now) {
            return Ok(false);
        }
        key.last_used_at = Some(key.last_used_at.map_or(now, |old| old.max(now)));
        Ok(true)
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

    /// One plan by code. Durable read failures are returned rather than
    /// converted into an unknown plan.
    pub async fn plan_by_code(&self, code: PlanCode) -> BotResult<Option<Plan>> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(PLAN, code.as_str()).await;
        }
        let inner = self.inner.read().await;
        let Some(id) = inner.plans_by_code.get(&code) else {
            return Ok(None);
        };
        Ok(inner.plans.get(id).cloned())
    }

    /// One plan by id. Durable read failures are returned rather than
    /// converted into an unknown plan.
    pub async fn plan(&self, id: PlanId) -> BotResult<Option<Plan>> {
        if let Some(repo) = &self.repo {
            return repo.by_id(PLAN, &id.to_string()).await;
        }
        Ok(self.inner.read().await.plans.get(&id).cloned())
    }

    /// The whole catalogue, weakest tier first.
    /// The full plan catalogue.
    ///
    /// With PostgreSQL attached this reads the durable catalogue and **fails
    /// closed**: a decode/database failure must surface as an error, never as
    /// an empty catalogue (an empty catalogue would silently make plan
    /// authority vacuous and misreport real plans as unknown).
    pub async fn plans(&self) -> BotResult<Vec<Plan>> {
        let mut v: Vec<Plan> = if let Some(repo) = &self.repo {
            repo.all(PLAN)
                .await
                .map_err(|e| BotError::db(format!("plan catalogue unavailable: {e}")))?
        } else {
            self.inner.read().await.plans.values().cloned().collect()
        };
        v.sort_by_key(|p| p.code);
        Ok(v)
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
            .await?
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

    /// The tenant's subscription. Durable read failures are returned rather
    /// than converted into no subscription.
    pub async fn subscription_of(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Option<Subscription>> {
        if let Some(repo) = &self.repo {
            return repo
                .by_lookup(SUBSCRIPTION, &organization_id.to_string())
                .await;
        }
        Ok(self
            .inner
            .read()
            .await
            .subscriptions
            .get(&organization_id)
            .cloned())
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

    /// The tenant's effective entitlements at `now`. Every durable lookup
    /// is fallible; an unavailable subscription or entitlement read must not
    /// be interpreted as a tenant with default access.
    pub async fn entitlements_of(
        &self,
        organization_id: OrganizationId,
        now: DateTime<Utc>,
    ) -> BotResult<EntitlementSet> {
        if let Some(repo) = &self.repo {
            let sub: Option<Subscription> = repo
                .by_lookup(SUBSCRIPTION, &organization_id.to_string())
                .await?;
            let plan = match &sub {
                Some(subscription) => repo.by_id(PLAN, &subscription.plan_id.to_string()).await?,
                None => None,
            };
            let rows: Vec<Entitlement> = repo.by_organization(ENTITLEMENT, organization_id).await?;
            return Ok(EntitlementSet::resolve(
                plan.as_ref(),
                sub.as_ref(),
                &rows,
                now,
            ));
        }
        let inner = self.inner.read().await;
        let sub = inner.subscriptions.get(&organization_id);
        let plan = sub.and_then(|s| inner.plans.get(&s.plan_id));
        let rows = inner
            .entitlements
            .get(&organization_id)
            .cloned()
            .unwrap_or_default();
        Ok(EntitlementSet::resolve(plan, sub, &rows, now))
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

    /// Total of one metric for one tenant in one `YYYY-MM` period. Durable
    /// query failures are returned instead of being converted into zero
    /// usage.
    pub async fn usage_total(
        &self,
        organization_id: OrganizationId,
        metric: UsageMetric,
        period: &str,
    ) -> BotResult<f64> {
        let events: Vec<UsageEvent> = if let Some(repo) = &self.repo {
            repo.by_organization(USAGE, organization_id).await?
        } else {
            self.inner.read().await.usage.clone()
        };
        Ok(events
            .iter()
            .filter(|e| {
                e.organization_id == organization_id && e.metric == metric && e.period() == period
            })
            .map(|e| e.quantity)
            .sum())
    }

    // ---------------------------------------------------- payments --

    /// Record a NEW payment transaction. `false` = an transaction with
    /// the same `(organization, idempotency_key)` identity was already
    /// recorded (idempotent re-delivery of a provider event).
    pub async fn record_payment(&self, payment: &PaymentTransaction) -> BotResult<bool> {
        if let Some(repo) = &self.repo {
            let lookup = format!("{}:{}", payment.organization_id, payment.idempotency_key);
            if !repo
                .insert(
                    PAYMENT,
                    &payment.id.to_string(),
                    Some(payment.organization_id),
                    None,
                    Some(&lookup),
                    payment,
                )
                .await?
            {
                return Ok(false);
            }
        } else if self
            .inner
            .read()
            .await
            .payments_by_idem
            .contains_key(&(payment.organization_id, payment.idempotency_key.clone()))
        {
            return Ok(false);
        }
        let mut inner = self.inner.write().await;
        inner.payments_by_idem.insert(
            (payment.organization_id, payment.idempotency_key.clone()),
            payment.id,
        );
        inner.payments.insert(payment.id, payment.clone());
        Ok(true)
    }

    /// Persist a payment transaction whose state changed (status
    /// transitions, provider ids, failure detail). Upsert semantics keep
    /// the memory and durable views in lockstep.
    pub async fn update_payment(&self, payment: &PaymentTransaction) -> BotResult<()> {
        if let Some(repo) = &self.repo {
            let lookup = format!("{}:{}", payment.organization_id, payment.idempotency_key);
            repo.upsert(
                PAYMENT,
                &payment.id.to_string(),
                Some(payment.organization_id),
                None,
                Some(&lookup),
                payment,
            )
            .await?;
        }
        let mut inner = self.inner.write().await;
        inner.payments_by_idem.insert(
            (payment.organization_id, payment.idempotency_key.clone()),
            payment.id,
        );
        inner.payments.insert(payment.id, payment.clone());
        Ok(())
    }

    /// One payment by id, ownership-checked: a foreign organization id
    /// can never read another tenant's transaction. Durable read failures
    /// are returned rather than converted into a missing transaction.
    pub async fn payment_of(
        &self,
        organization_id: OrganizationId,
        id: PaymentId,
    ) -> BotResult<Option<PaymentTransaction>> {
        if let Some(repo) = &self.repo {
            let found: Option<PaymentTransaction> = repo.by_id(PAYMENT, &id.to_string()).await?;
            return Ok(found.filter(|p| p.organization_id == organization_id));
        }
        Ok(self
            .inner
            .read()
            .await
            .payments
            .get(&id)
            .filter(|p| p.organization_id == organization_id)
            .cloned())
    }

    /// All payments for one tenant, oldest first. Durable read failures are
    /// returned rather than an empty payment history.
    pub async fn payments_of(
        &self,
        organization_id: OrganizationId,
    ) -> BotResult<Vec<PaymentTransaction>> {
        let mut out: Vec<PaymentTransaction> = if let Some(repo) = &self.repo {
            repo.by_organization(PAYMENT, organization_id).await?
        } else {
            self.inner
                .read()
                .await
                .payments
                .values()
                .filter(|p| p.organization_id == organization_id)
                .cloned()
                .collect()
        };
        out.sort_by_key(|p| p.created_at);
        Ok(out)
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

    /// One job by request key. Durable read failures are returned rather
    /// than converted into a missing job.
    pub async fn job_by_request_key(
        &self,
        request_key: &str,
    ) -> BotResult<Option<ProvisioningJob>> {
        if let Some(repo) = &self.repo {
            return repo.by_lookup(JOB, request_key).await;
        }
        Ok(self.inner.read().await.jobs.get(request_key).cloned())
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

    /// Jobs a worker may resume, oldest first. A durable read failure is
    /// returned so a worker never silently concludes that no work exists.
    pub async fn resumable_jobs(&self, limit: usize) -> BotResult<Vec<ProvisioningJob>> {
        let mut v: Vec<ProvisioningJob> = if let Some(repo) = &self.repo {
            repo.all(JOB).await?
        } else {
            self.inner.read().await.jobs.values().cloned().collect()
        }
        .into_iter()
        .filter(|j| j.state.is_resumable())
        .collect();
        v.sort_by_key(|j| j.created_at);
        v.truncate(limit);
        Ok(v)
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
            let a = first
                .plan_by_code(code)
                .await
                .expect("first plan lookup")
                .expect("first plan");
            let b = second
                .plan_by_code(code)
                .await
                .expect("second plan lookup")
                .expect("second plan");
            assert_eq!(a.id, b.id);
            assert_eq!(a.limits, b.limits);
        }

        let organization_id = OrganizationId::new();
        let plan = first
            .plan_by_code(PlanCode::Business)
            .await
            .expect("business plan lookup")
            .expect("business plan");
        // PostgreSQL timestamptz stores microseconds, so normalize the
        // fixture clock before asserting exact round-trips from the DB.
        let now = DateTime::<Utc>::from_timestamp_micros(Utc::now().timestamp_micros())
            .expect("current time is representable");
        let subscription = first
            .assign_plan(organization_id, PlanCode::Business, now)
            .await
            .expect("atomic plan assignment");
        assert_eq!(
            second
                .subscription_of(organization_id)
                .await
                .expect("replica subscription lookup")
                .expect("replica subscription")
                .id,
            subscription.id
        );
        assert_eq!(
            second
                .entitlements_of(organization_id, now)
                .await
                .expect("entitlements")
                .len(),
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
            .expect("key lookup survives the restart")
            .expect("key survives the restart");
        assert_eq!(key.id, created.key.id);
        assert_eq!(key.organization_id, organization_id);
        assert!(key.is_usable(now), "valid key is usable after restart");
        assert_eq!(key.rejection(now), None);
        let used_at = now + chrono::Duration::seconds(1);
        assert!(second
            .touch_api_key_last_used(key.id, &key.secret_hash, used_at)
            .await
            .expect("record successful use on second replica"));
        let revoked_at = now + chrono::Duration::seconds(2);
        assert!(first
            .revoke_api_key(key.id, &key.secret_hash, "operator request", revoked_at,)
            .await
            .expect("persist revoke"));
        let seen_by_second = second
            .api_key_by_hash(&created.key.secret_hash)
            .await
            .expect("revoked record lookup")
            .expect("revoked record is still there");
        assert!(!seen_by_second.is_usable(now), "revoked key stops working");
        assert_eq!(seen_by_second.rejection(now), Some("api_key_revoked"));
        assert!(
            !second
                .touch_api_key_last_used(
                    key.id,
                    &key.secret_hash,
                    now + chrono::Duration::seconds(3)
                )
                .await
                .expect("stale authentication touch is a no-op"),
            "a replica's last-used write cannot restore a revoked key"
        );
        let after_stale_touch = second
            .api_key_by_hash(&created.key.secret_hash)
            .await
            .expect("reload after stale touch")
            .expect("revoked record remains stored");
        assert_eq!(after_stale_touch.revoked_at, Some(revoked_at));
        assert_eq!(after_stale_touch.last_used_at, Some(used_at));

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
            .expect("expired record lookup")
            .expect("expired record is there");
        assert!(!seen_expired.is_usable(now), "expired key stops working");
        assert_eq!(seen_expired.rejection(now), Some("api_key_expired"));

        // Session field updates use the same durable replica guarantee: a
        // last-seen/MFA update cannot overwrite a revoke committed elsewhere.
        let user_id = UserId::new();
        let user = User {
            id: user_id,
            email: format!("session-race-{user_id}@example.invalid"),
            email_verified: true,
            display_name: "Session race test".to_string(),
            password_hash: "test-only-hash".to_string(),
            status: bot_core::tenant::UserStatus::Active,
            platform_admin: false,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        };
        first
            .create_user(&user)
            .await
            .expect("insert session owner");
        let token_hash = format!("session-token-hash-{user_id}");
        let session = SessionRecord::new(
            user_id,
            None,
            token_hash,
            "ss_race",
            chrono::Duration::hours(1),
            now,
        );
        first
            .create_session(&session)
            .await
            .expect("insert durable session");
        let touched_at = now + chrono::Duration::seconds(2);
        assert!(second
            .touch_session_last_seen(session.id, &session.token_hash, touched_at)
            .await
            .expect("touch from second replica"));
        assert!(second
            .set_session_mfa_state(
                session.id,
                &session.token_hash,
                Some(touched_at),
                false,
                touched_at,
            )
            .await
            .expect("set MFA state from second replica"));
        let revoked_at = now + chrono::Duration::seconds(3);
        assert!(first
            .revoke_session(
                session.id,
                &session.token_hash,
                "durable race test",
                revoked_at,
            )
            .await
            .expect("revoke from first replica"));
        assert!(!second
            .touch_session_last_seen(
                session.id,
                &session.token_hash,
                now + chrono::Duration::seconds(4),
            )
            .await
            .expect("stale last-seen touch is a no-op"));
        assert!(!second
            .set_session_mfa_state(
                session.id,
                &session.token_hash,
                None,
                true,
                now + chrono::Duration::seconds(4),
            )
            .await
            .expect("stale MFA update is a no-op"));
        let after_session_race = second
            .session(session.id)
            .await
            .expect("reload session after race")
            .expect("revoked session remains stored");
        assert_eq!(after_session_race.revoked_at, Some(revoked_at));
        assert_eq!(after_session_race.revoke_reason, "durable race test");
        assert_eq!(after_session_race.last_seen_at, touched_at);
        assert_eq!(after_session_race.mfa_policy_updated_at, Some(touched_at));
        assert!(!after_session_race.mfa_enrollment_only);

        db.close().await;
    }
}

#[cfg(test)]
mod session_touch_tests {
    use super::*;

    #[tokio::test]
    async fn session_touch_is_monotonic_and_cannot_restore_revocation() {
        let store = SaasStore::new_in_memory_identity();
        let now = Utc::now();
        let live = SessionRecord::new(
            UserId::new(),
            None,
            "live-session-hash",
            "ss_live",
            chrono::Duration::hours(1),
            now,
        );
        store
            .create_session(&live)
            .await
            .expect("create live session");
        assert!(store
            .touch_session_last_seen(
                live.id,
                &live.token_hash,
                now + chrono::Duration::seconds(3)
            )
            .await
            .expect("touch live session"));
        assert!(store
            .touch_session_last_seen(
                live.id,
                &live.token_hash,
                now + chrono::Duration::seconds(2)
            )
            .await
            .expect("older timestamp is an idempotent touch"));
        assert!(store
            .set_session_mfa_state(
                live.id,
                &live.token_hash,
                Some(now + chrono::Duration::seconds(3)),
                false,
                now + chrono::Duration::seconds(3),
            )
            .await
            .expect("update active session MFA state"));
        let live_after_updates = store.session(live.id).await.unwrap().unwrap();
        assert_eq!(
            live_after_updates.last_seen_at,
            now + chrono::Duration::seconds(3)
        );
        assert_eq!(
            live_after_updates.mfa_policy_updated_at,
            Some(now + chrono::Duration::seconds(3))
        );

        let candidate = SessionRecord::new(
            UserId::new(),
            None,
            "revoked-session-hash",
            "ss_revoked",
            chrono::Duration::hours(1),
            now,
        );
        store
            .create_session(&candidate)
            .await
            .expect("create revocable session");
        assert!(store
            .revoke_session(candidate.id, &candidate.token_hash, "operator revoke", now,)
            .await
            .expect("persist revocation"));
        assert!(!store
            .revoke_session(
                candidate.id,
                &candidate.token_hash,
                "later reason must not replace the first",
                now + chrono::Duration::seconds(1),
            )
            .await
            .expect("duplicate revocation is a no-op"));
        assert!(!store
            .set_session_mfa_state(
                candidate.id,
                &candidate.token_hash,
                Some(now),
                false,
                now + chrono::Duration::seconds(1),
            )
            .await
            .expect("revoked session MFA update is a no-op"));
        assert!(!store
            .touch_session_last_seen(
                candidate.id,
                &candidate.token_hash,
                now + chrono::Duration::seconds(1)
            )
            .await
            .expect("revoked session touch is a no-op"));
        let after = store.session(candidate.id).await.unwrap().unwrap();
        assert_eq!(after.revoked_at, Some(now));
        assert_eq!(after.revoke_reason, "operator revoke");
        assert_eq!(after.last_seen_at, now);
    }
}
