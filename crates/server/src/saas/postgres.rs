//! PostgreSQL adapter for the SaaS control-plane domain records.
//!
//! Every backed read goes to PostgreSQL, so two server replicas observe the
//! same identities, revocations, entitlements, usage keys, and provisioning
//! jobs. The in-process maps remain only the no-database implementation and a
//! post-write mirror; they are never an authority when this adapter exists.

use std::sync::Arc;

use bot_core::billing::{Entitlement, Subscription};
use bot_core::db::Database;
use bot_core::error::{BotError, BotResult};
use bot_core::membership::Membership;
use bot_core::session::SessionRecord;
use bot_core::tenant::{Organization, OrganizationId, User, UserId};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sqlx::Row;

/// Record kinds. Closed here so SQL labels cannot be caller-controlled.
pub const USER: &str = "user";
pub const ORGANIZATION: &str = "organization";
pub const MEMBERSHIP: &str = "membership";
pub const SESSION: &str = "session";
pub const API_KEY: &str = "api_key";
pub const PLAN: &str = "plan";
pub const SUBSCRIPTION: &str = "subscription";
pub const ENTITLEMENT: &str = "entitlement";
pub const USAGE: &str = "usage";
pub const JOB: &str = "provisioning_job";
pub const PAYMENT: &str = "payment_transaction";

/// Shared PostgreSQL repository.
pub struct PostgresSaasRepo {
    db: Arc<Database>,
}

impl PostgresSaasRepo {
    /// Attach to an already migrated application database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn encode<T: Serialize>(record: &T) -> BotResult<serde_json::Value> {
        serde_json::to_value(record)
            .map_err(|e| BotError::db(format!("serialize SaaS record: {e}")))
    }

    fn decode<T: DeserializeOwned>(value: serde_json::Value) -> BotResult<T> {
        serde_json::from_value(value).map_err(|e| BotError::db(format!("decode SaaS record: {e}")))
    }

    /// Keep the legacy relational identity rows in sync with the canonical
    /// serialized SaaS records. Several durable tables (including MFA
    /// devices and tenant policies) have relational FKs to these rows, so
    /// the projection must be committed atomically with the runtime record.
    async fn sync_identity_projection(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        kind: &'static str,
        document: &serde_json::Value,
    ) -> BotResult<()> {
        match kind {
            USER => {
                let user: User = Self::decode(document.clone())?;
                sqlx::query(
                    r#"INSERT INTO users
                           (id, email, email_verified, display_name, password_hash, status,
                            platform_admin, created_at, updated_at, last_login_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                       ON CONFLICT (id) DO UPDATE SET
                           email=EXCLUDED.email,
                           email_verified=EXCLUDED.email_verified,
                           display_name=EXCLUDED.display_name,
                           password_hash=EXCLUDED.password_hash,
                           status=EXCLUDED.status,
                           platform_admin=EXCLUDED.platform_admin,
                           updated_at=EXCLUDED.updated_at,
                           last_login_at=EXCLUDED.last_login_at"#,
                )
                .bind(user.id.as_uuid())
                .bind(&user.email)
                .bind(user.email_verified)
                .bind(&user.display_name)
                .bind(&user.password_hash)
                .bind(user.status.as_str())
                .bind(user.platform_admin)
                .bind(user.created_at)
                .bind(user.updated_at)
                .bind(user.last_login_at)
                .execute(&mut **transaction)
                .await
                .map_err(|error| BotError::db(format!("project SaaS user to users table: {error}")))?;
            }
            ORGANIZATION => {
                let organization: Organization = Self::decode(document.clone())?;
                sqlx::query(
                    r#"INSERT INTO organizations
                           (id, slug, name, status, created_by, created_at, updated_at,
                            suspended_at, suspend_reason)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
                       ON CONFLICT (id) DO UPDATE SET
                           slug=EXCLUDED.slug,
                           name=EXCLUDED.name,
                           status=EXCLUDED.status,
                           created_by=EXCLUDED.created_by,
                           updated_at=EXCLUDED.updated_at,
                           suspended_at=EXCLUDED.suspended_at,
                           suspend_reason=EXCLUDED.suspend_reason"#,
                )
                .bind(organization.id.as_uuid())
                .bind(&organization.slug)
                .bind(&organization.name)
                .bind(organization.status.as_str())
                .bind(organization.created_by.map(|user| user.as_uuid()))
                .bind(organization.created_at)
                .bind(organization.updated_at)
                .bind(organization.suspended_at)
                .bind(&organization.suspend_reason)
                .execute(&mut **transaction)
                .await
                .map_err(|error| {
                    BotError::db(format!("project SaaS organization to organizations table: {error}"))
                })?;
            }
            MEMBERSHIP => {
                let membership: Membership = Self::decode(document.clone())?;
                sqlx::query(
                    r#"INSERT INTO organization_members
                           (id, organization_id, user_id, role, status, invited_by,
                            created_at, updated_at)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                       ON CONFLICT (organization_id, user_id) DO UPDATE SET
                           id=EXCLUDED.id,
                           role=EXCLUDED.role,
                           status=EXCLUDED.status,
                           invited_by=EXCLUDED.invited_by,
                           updated_at=EXCLUDED.updated_at"#,
                )
                .bind(membership.id.as_uuid())
                .bind(membership.organization_id.as_uuid())
                .bind(membership.user_id.as_uuid())
                .bind(membership.role.as_str())
                .bind(membership.status.as_str())
                .bind(membership.invited_by.map(|user| user.as_uuid()))
                .bind(membership.created_at)
                .bind(membership.updated_at)
                .execute(&mut **transaction)
                .await
                .map_err(|error| {
                    BotError::db(format!("project SaaS membership to organization_members table: {error}"))
                })?;
            }
            SESSION => {
                let session: SessionRecord = Self::decode(document.clone())?;
                sqlx::query(
                    r#"INSERT INTO sessions
                           (id, user_id, organization_id, token_hash, token_prefix, user_agent,
                            ip, created_at, last_seen_at, expires_at, revoked_at, revoke_reason)
                       VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
                       ON CONFLICT (id) DO UPDATE SET
                           organization_id=EXCLUDED.organization_id,
                           last_seen_at=EXCLUDED.last_seen_at,
                           expires_at=EXCLUDED.expires_at,
                           revoked_at=EXCLUDED.revoked_at,
                           revoke_reason=EXCLUDED.revoke_reason"#,
                )
                .bind(session.id.as_uuid())
                .bind(session.user_id.as_uuid())
                .bind(session.organization_id.map(|organization| organization.as_uuid()))
                .bind(&session.token_hash)
                .bind(&session.token_prefix)
                .bind(&session.user_agent)
                .bind(&session.ip)
                .bind(session.created_at)
                .bind(session.last_seen_at)
                .bind(session.expires_at)
                .bind(session.revoked_at)
                .bind(&session.revoke_reason)
                .execute(&mut **transaction)
                .await
                .map_err(|error| BotError::db(format!("project SaaS session to sessions table: {error}")))?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Insert once. `false` means the id or unique lookup key already exists.
    pub async fn insert<T: Serialize>(
        &self,
        kind: &'static str,
        id: &str,
        organization_id: Option<OrganizationId>,
        user_id: Option<UserId>,
        lookup_key: Option<&str>,
        record: &T,
    ) -> BotResult<bool> {
        let document = Self::encode(record)?;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|error| BotError::db(format!("begin SaaS {kind} insert: {error}")))?;
        let result = sqlx::query(
            r#"INSERT INTO saas_runtime_records
                 (kind,id,organization_id,user_id,lookup_key,record)
               VALUES ($1,$2,$3,$4,$5,$6)
               ON CONFLICT DO NOTHING"#,
        )
        .bind(kind)
        .bind(id)
        .bind(organization_id.map(|v| v.as_uuid()))
        .bind(user_id.map(|v| v.as_uuid()))
        .bind(lookup_key)
        .bind(&document)
        .execute(&mut *transaction)
        .await
        .map_err(|error| BotError::db(format!("insert SaaS {kind}: {error}")))?;
        let inserted = result.rows_affected() == 1;
        if inserted {
            Self::sync_identity_projection(&mut transaction, kind, &document).await?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| BotError::db(format!("commit SaaS {kind} insert: {error}")))?;
        Ok(inserted)
    }

    /// Replace one existing record, preserving its original creation time.
    pub async fn update<T: Serialize>(
        &self,
        kind: &'static str,
        id: &str,
        organization_id: Option<OrganizationId>,
        user_id: Option<UserId>,
        lookup_key: Option<&str>,
        record: &T,
    ) -> BotResult<bool> {
        let document = Self::encode(record)?;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|error| BotError::db(format!("begin SaaS {kind} update: {error}")))?;
        let result = sqlx::query(
            r#"UPDATE saas_runtime_records
               SET organization_id=$3,user_id=$4,lookup_key=$5,record=$6,updated_at=now()
               WHERE kind=$1 AND id=$2"#,
        )
        .bind(kind)
        .bind(id)
        .bind(organization_id.map(|v| v.as_uuid()))
        .bind(user_id.map(|v| v.as_uuid()))
        .bind(lookup_key)
        .bind(&document)
        .execute(&mut *transaction)
        .await
        .map_err(|error| BotError::db(format!("update SaaS {kind}: {error}")))?;
        let updated = result.rows_affected() == 1;
        if updated {
            Self::sync_identity_projection(&mut transaction, kind, &document).await?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| BotError::db(format!("commit SaaS {kind} update: {error}")))?;
        Ok(updated)
    }

    async fn revoke_user_sessions_in_transaction(
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        user_id: UserId,
        reason: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> BotResult<usize> {
        let row = sqlx::query(
            r#"WITH revoked AS (
                   UPDATE saas_runtime_records
                      SET record = jsonb_set(
                                      jsonb_set(record, '{revoked_at}', to_jsonb($3::timestamptz), true),
                                      '{revoke_reason}', to_jsonb($2::text), true
                                  ),
                          updated_at = $3
                    WHERE kind = 'session'
                      AND user_id = $1
                      AND (record->>'revoked_at' IS NULL OR record->>'revoked_at' = '')
                    RETURNING id, record
               ), projected AS (
                   INSERT INTO sessions (
                       id, user_id, organization_id, token_hash, token_prefix,
                       user_agent, ip, created_at, last_seen_at, expires_at,
                       revoked_at, revoke_reason
                   )
                   SELECT id::uuid,
                          (record->>'user_id')::uuid,
                          NULLIF(record->>'organization_id', '')::uuid,
                          record->>'token_hash',
                          COALESCE(record->>'token_prefix', ''),
                          COALESCE(record->>'user_agent', ''),
                          COALESCE(record->>'ip', ''),
                          (record->>'created_at')::timestamptz,
                          (record->>'last_seen_at')::timestamptz,
                          (record->>'expires_at')::timestamptz,
                          NULLIF(record->>'revoked_at', '')::timestamptz,
                          COALESCE(record->>'revoke_reason', '')
                     FROM revoked
                   ON CONFLICT (id) DO UPDATE SET
                       organization_id = EXCLUDED.organization_id,
                       last_seen_at = EXCLUDED.last_seen_at,
                       expires_at = EXCLUDED.expires_at,
                       revoked_at = EXCLUDED.revoked_at,
                       revoke_reason = EXCLUDED.revoke_reason
                   RETURNING id
               )
               SELECT (SELECT COUNT(*) FROM revoked)::bigint AS runtime_count,
                      (SELECT COUNT(*) FROM projected)::bigint AS projected_count"#,
        )
        .bind(user_id.as_uuid())
        .bind(reason)
        .bind(now)
        .fetch_one(&mut **transaction)
        .await
        .map_err(|error| BotError::db(format!("revoke user sessions: {error}")))?;
        let runtime_count = row
            .try_get::<i64, _>("runtime_count")
            .map_err(|error| BotError::db(format!("decode revoked session count: {error}")))?;
        let projected_count = row
            .try_get::<i64, _>("projected_count")
            .map_err(|error| BotError::db(format!("decode projected session count: {error}")))?;
        if runtime_count != projected_count {
            return Err(BotError::db(format!(
                "session projection mismatch after revocation: runtime={runtime_count}, projected={projected_count}"
            )));
        }
        usize::try_from(runtime_count)
            .map_err(|_| BotError::db("revoked session count is outside usize range"))
    }

    /// Revoke all user sessions and update their normalized projections in
    /// one transaction. A partial revocation is never reported as success.
    pub async fn revoke_user_sessions(
        &self,
        user_id: UserId,
        reason: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> BotResult<usize> {
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|error| BotError::db(format!("begin user-session revocation: {error}")))?;
        let count = Self::revoke_user_sessions_in_transaction(
            &mut transaction,
            user_id,
            reason,
            now,
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|error| BotError::db(format!("commit user-session revocation: {error}")))?;
        Ok(count)
    }

    /// Atomically change a user's password and revoke every existing session.
    pub async fn update_user_and_revoke_sessions(
        &self,
        user: &User,
        reason: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> BotResult<usize> {
        let document = Self::encode(user)?;
        let email = User::normalize_email(&user.email);
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|error| BotError::db(format!("begin password change: {error}")))?;
        let updated = sqlx::query(
            "UPDATE saas_runtime_records SET record = $3, lookup_key = $4, user_id = $5, updated_at = $6 WHERE kind = $1 AND id = $2",
        )
        .bind(USER)
        .bind(user.id.to_string())
        .bind(&document)
        .bind(&email)
        .bind(user.id.as_uuid())
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| BotError::db(format!("update password record: {error}")))?;
        if updated.rows_affected() != 1 {
            return Err(BotError::NotFound(format!("user {}", user.id)));
        }
        Self::sync_identity_projection(&mut transaction, USER, &document).await?;
        let count = Self::revoke_user_sessions_in_transaction(
            &mut transaction,
            user.id,
            reason,
            now,
        )
        .await?;
        transaction
            .commit()
            .await
            .map_err(|error| BotError::db(format!("commit password change and session revocation: {error}")))?;
        Ok(count)
    }

    /// Insert or replace by `(kind,id)`. Unique lookup-key conflicts with a
    /// different id still fail closed instead of stealing another identity.
    pub async fn upsert<T: Serialize>(
        &self,
        kind: &'static str,
        id: &str,
        organization_id: Option<OrganizationId>,
        user_id: Option<UserId>,
        lookup_key: Option<&str>,
        record: &T,
    ) -> BotResult<()> {
        let document = Self::encode(record)?;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|error| BotError::db(format!("begin SaaS {kind} upsert: {error}")))?;
        sqlx::query(
            r#"INSERT INTO saas_runtime_records
                 (kind,id,organization_id,user_id,lookup_key,record)
               VALUES ($1,$2,$3,$4,$5,$6)
               ON CONFLICT (kind,id) DO UPDATE SET
                 organization_id=EXCLUDED.organization_id,
                 user_id=EXCLUDED.user_id,
                 lookup_key=EXCLUDED.lookup_key,
                 record=EXCLUDED.record,
                 updated_at=now()"#,
        )
        .bind(kind)
        .bind(id)
        .bind(organization_id.map(|v| v.as_uuid()))
        .bind(user_id.map(|v| v.as_uuid()))
        .bind(lookup_key)
        .bind(&document)
        .execute(&mut *transaction)
        .await
        .map_err(|error| BotError::db(format!("upsert SaaS {kind}: {error}")))?;
        Self::sync_identity_projection(&mut transaction, kind, &document).await?;
        transaction
            .commit()
            .await
            .map_err(|error| BotError::db(format!("commit SaaS {kind} upsert: {error}")))?;
        Ok(())
    }

    /// Atomically replace a tenant's subscription and plan-derived
    /// entitlements. Readers can never observe half of a plan assignment.
    pub async fn assign_plan(
        &self,
        subscription: &Subscription,
        entitlements: &[Entitlement],
    ) -> BotResult<()> {
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|e| BotError::db(format!("begin SaaS plan assignment: {e}")))?;
        sqlx::query(
            r#"INSERT INTO saas_runtime_records
                 (kind,id,organization_id,lookup_key,record)
               VALUES ($1,$2,$3,$4,$5)
               ON CONFLICT (kind,id) DO UPDATE SET
                 organization_id=EXCLUDED.organization_id,
                 lookup_key=EXCLUDED.lookup_key,
                 record=EXCLUDED.record,
                 updated_at=now()"#,
        )
        .bind(SUBSCRIPTION)
        .bind(subscription.id.to_string())
        .bind(subscription.organization_id.as_uuid())
        .bind(subscription.organization_id.to_string())
        .bind(Self::encode(subscription)?)
        .execute(&mut *tx)
        .await
        .map_err(|e| BotError::db(format!("write SaaS subscription: {e}")))?;
        sqlx::query("DELETE FROM saas_runtime_records WHERE kind=$1 AND organization_id=$2")
            .bind(ENTITLEMENT)
            .bind(subscription.organization_id.as_uuid())
            .execute(&mut *tx)
            .await
            .map_err(|e| BotError::db(format!("replace SaaS entitlements: {e}")))?;
        for entitlement in entitlements {
            let lookup = format!(
                "{}:{}:{}",
                entitlement.organization_id,
                entitlement.feature,
                entitlement.source.as_str()
            );
            sqlx::query(
                r#"INSERT INTO saas_runtime_records
                     (kind,id,organization_id,lookup_key,record)
                   VALUES ($1,$2,$3,$4,$5)"#,
            )
            .bind(ENTITLEMENT)
            .bind(entitlement.id.to_string())
            .bind(entitlement.organization_id.as_uuid())
            .bind(lookup)
            .bind(Self::encode(entitlement)?)
            .execute(&mut *tx)
            .await
            .map_err(|e| BotError::db(format!("write SaaS entitlement: {e}")))?;
        }
        tx.commit()
            .await
            .map_err(|e| BotError::db(format!("commit SaaS plan assignment: {e}")))?;
        Ok(())
    }

    /// Fetch by stable id.
    pub async fn by_id<T: DeserializeOwned>(
        &self,
        kind: &'static str,
        id: &str,
    ) -> BotResult<Option<T>> {
        let row = sqlx::query("SELECT record FROM saas_runtime_records WHERE kind=$1 AND id=$2")
            .bind(kind)
            .bind(id)
            .fetch_optional(self.db.pool())
            .await
            .map_err(|e| BotError::db(format!("read SaaS {kind}: {e}")))?;
        row.map(|r| Self::decode(r.get("record"))).transpose()
    }

    /// Fetch by the kind-specific unique lookup key.
    pub async fn by_lookup<T: DeserializeOwned>(
        &self,
        kind: &'static str,
        key: &str,
    ) -> BotResult<Option<T>> {
        let row =
            sqlx::query("SELECT record FROM saas_runtime_records WHERE kind=$1 AND lookup_key=$2")
                .bind(kind)
                .bind(key)
                .fetch_optional(self.db.pool())
                .await
                .map_err(|e| BotError::db(format!("lookup SaaS {kind}: {e}")))?;
        row.map(|r| Self::decode(r.get("record"))).transpose()
    }

    /// All records of a kind.
    pub async fn all<T: DeserializeOwned>(&self, kind: &'static str) -> BotResult<Vec<T>> {
        let rows = sqlx::query(
            "SELECT record FROM saas_runtime_records WHERE kind=$1 ORDER BY created_at,id",
        )
        .bind(kind)
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("list SaaS {kind}: {e}")))?;
        rows.into_iter()
            .map(|r| Self::decode(r.get("record")))
            .collect()
    }

    /// Tenant-scoped records of one kind.
    pub async fn by_organization<T: DeserializeOwned>(
        &self,
        kind: &'static str,
        organization_id: OrganizationId,
    ) -> BotResult<Vec<T>> {
        let rows = sqlx::query(
            r#"SELECT record FROM saas_runtime_records
               WHERE kind=$1 AND organization_id=$2 ORDER BY created_at,id"#,
        )
        .bind(kind)
        .bind(organization_id.as_uuid())
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("list tenant SaaS {kind}: {e}")))?;
        rows.into_iter()
            .map(|r| Self::decode(r.get("record")))
            .collect()
    }

    /// User-scoped records of one kind.
    pub async fn by_user<T: DeserializeOwned>(
        &self,
        kind: &'static str,
        user_id: UserId,
    ) -> BotResult<Vec<T>> {
        let rows = sqlx::query(
            r#"SELECT record FROM saas_runtime_records
               WHERE kind=$1 AND user_id=$2 ORDER BY created_at,id"#,
        )
        .bind(kind)
        .bind(user_id.as_uuid())
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("list user SaaS {kind}: {e}")))?;
        rows.into_iter()
            .map(|r| Self::decode(r.get("record")))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::config::DatabaseConfig;
    use bot_core::membership::{Membership, MembershipRole};
    use bot_core::session::{generate_token, SessionRecord};
    use bot_core::tenant::{Organization, User, UserStatus};
    use chrono::{Duration, Utc};
    use serde_json::json;
    use uuid::Uuid;

    /// Exercises every SaaS runtime record class through two independent
    /// repository handles. It self-skips unless POSTGRES_URL is supplied.
    #[tokio::test]
    async fn restart_and_two_replicas_share_all_runtime_records() {
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
        let replica_a = PostgresSaasRepo::new(db.clone());
        let replica_b = PostgresSaasRepo::new(db.clone());
        let org = OrganizationId::new();
        let user = UserId::new();
        let run = Uuid::new_v4().to_string();
        let now = Utc::now();
        let user_record = User {
            id: user,
            email: format!("{run}@example.invalid"),
            email_verified: true,
            display_name: "PostgreSQL repository probe".into(),
            password_hash: bot_core::session::token::hash_password_with(
                "fixture-password-not-used-for-authentication",
                b"saas-pg-probe-salt",
                1,
            ),
            status: UserStatus::Active,
            platform_admin: false,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        };
        let organization_record = Organization::new(
            org,
            format!("saas-probe-{run}"),
            "PostgreSQL repository probe",
            Some(user),
            now,
        );
        let membership_record = Membership::new(
            org,
            user,
            MembershipRole::OrgOwner,
            Some(user),
            now,
        );
        let session_token = generate_token("ses");
        let session_record = SessionRecord::new(
            user,
            Some(org),
            session_token.hash,
            session_token.prefix,
            Duration::hours(1),
            now,
        );

        // These four records are also projected into normalized tables. Build
        // real domain values and insert them in FK order rather than relying on
        // `{}` documents or non-UUID synthetic primary keys.
        for kind in [
            USER,
            ORGANIZATION,
            MEMBERSHIP,
            SESSION,
            API_KEY,
            PLAN,
            SUBSCRIPTION,
            ENTITLEMENT,
            USAGE,
            JOB,
        ] {
            let (id, lookup, record) = match kind {
                USER => (
                    user.to_string(),
                    user_record.email.clone(),
                    serde_json::to_value(&user_record).expect("serialize user fixture"),
                ),
                ORGANIZATION => (
                    org.to_string(),
                    organization_record.slug.clone(),
                    serde_json::to_value(&organization_record)
                        .expect("serialize organization fixture"),
                ),
                MEMBERSHIP => (
                    membership_record.id.to_string(),
                    format!("{run}-{kind}-lookup"),
                    serde_json::to_value(&membership_record)
                        .expect("serialize membership fixture"),
                ),
                SESSION => (
                    session_record.id.to_string(),
                    format!("{run}-{kind}-lookup"),
                    serde_json::to_value(&session_record).expect("serialize session fixture"),
                ),
                _ => (
                    format!("{run}-{kind}"),
                    format!("{run}-{kind}-lookup"),
                    json!({"run": run, "kind": kind, "state": "active"}),
                ),
            };
            assert!(replica_a
                .insert(kind, &id, Some(org), Some(user), Some(&lookup), &record)
                .await
                .expect("insert"));
            let read: serde_json::Value = replica_b
                .by_id(kind, &id)
                .await
                .expect("replica read")
                .expect("record exists");
            assert_eq!(read, record);
            let tenant_records: Vec<serde_json::Value> = replica_b
                .by_organization(kind, org)
                .await
                .expect("tenant list");
            assert!(tenant_records.contains(&record));
            let user_records: Vec<serde_json::Value> =
                replica_b.by_user(kind, user).await.expect("user list");
            assert!(user_records.contains(&record));
        }

        // Replica B sees replica A's revocation immediately, with no local
        // hydration step and no process-memory authority.
        let key_id = format!("{run}-{API_KEY}");
        let key_lookup = format!("{run}-{API_KEY}-lookup");
        let revoked = json!({"run": run, "kind": API_KEY, "state": "revoked"});
        assert!(replica_a
            .update(
                API_KEY,
                &key_id,
                Some(org),
                Some(user),
                Some(&key_lookup),
                &revoked,
            )
            .await
            .expect("revoke"));
        let observed: serde_json::Value = replica_b
            .by_lookup(API_KEY, &key_lookup)
            .await
            .expect("lookup")
            .expect("key exists");
        assert_eq!(observed, revoked);

        // Tenant-qualified lookup uniqueness is the cross-replica
        // idempotency primitive for usage and provisioning requests.
        for kind in [USAGE, JOB] {
            let lookup = format!("{run}-{kind}-idempotency");
            let first = json!({"writer": "a"});
            let second = json!({"writer": "b"});
            assert!(replica_a
                .insert(
                    kind,
                    &format!("{run}-{kind}-first"),
                    Some(org),
                    Some(user),
                    Some(&lookup),
                    &first,
                )
                .await
                .expect("first idempotent insert"));
            assert!(!replica_b
                .insert(
                    kind,
                    &format!("{run}-{kind}-second"),
                    Some(org),
                    Some(user),
                    Some(&lookup),
                    &second,
                )
                .await
                .expect("duplicate idempotent insert"));
            let winner: serde_json::Value = replica_b
                .by_lookup(kind, &lookup)
                .await
                .expect("winner lookup")
                .expect("winner exists");
            assert_eq!(winner, first);
        }

        // Clean up runtime rows and normalized projections in one transaction.
        // Every ID is generated for this run, so a shared integration database
        // is left unchanged. The relational tables do not cascade from the
        // canonical JSON runtime table.
        let mut cleanup = db.pool().begin().await.expect("begin probe cleanup");
        sqlx::query(
            "DELETE FROM saas_runtime_records WHERE (organization_id = $1 AND user_id = $2) OR id LIKE $3 OR lookup_key LIKE $3",
        )
            .bind(org.as_uuid())
            .bind(user.as_uuid())
            .bind(format!("{run}-%"))
            .execute(&mut *cleanup)
            .await
            .expect("delete runtime probe rows");
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_record.id.as_uuid())
            .execute(&mut *cleanup)
            .await
            .expect("delete projected session");
        sqlx::query("DELETE FROM organization_members WHERE id = $1")
            .bind(membership_record.id.as_uuid())
            .execute(&mut *cleanup)
            .await
            .expect("delete projected membership");
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(org.as_uuid())
            .execute(&mut *cleanup)
            .await
            .expect("delete projected organization");
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user.as_uuid())
            .execute(&mut *cleanup)
            .await
            .expect("delete projected user");
        cleanup.commit().await.expect("commit probe cleanup");

        db.close().await;
    }
}
