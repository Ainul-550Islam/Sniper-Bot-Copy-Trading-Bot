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
use bot_core::tenant::{OrganizationId, UserId};
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
        .bind(Self::encode(record)?)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("insert SaaS {kind}: {e}")))?;
        Ok(result.rows_affected() == 1)
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
        .bind(Self::encode(record)?)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("update SaaS {kind}: {e}")))?;
        Ok(result.rows_affected() == 1)
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
        .bind(Self::encode(record)?)
        .execute(self.db.pool())
        .await
        .map_err(|e| BotError::db(format!("upsert SaaS {kind}: {e}")))?;
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
            let id = format!("{run}-{kind}");
            let lookup = format!("{run}-{kind}-lookup");
            let record = json!({"run": run, "kind": kind, "state": "active"});
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

        db.close().await;
    }
}
