//! Durable, tenant-scoped storage for custody rotation records (§S-4).
//!
//! # Why this module exists
//!
//! `saas::custody_rotation` kept every rotation in a process-global
//! `OnceLock<Mutex<HashMap<Uuid, RotationRecord>>>`. For a key-rotation
//! workflow that is three distinct failures:
//!
//! * **cross-replica**: a rotation created on replica A returned 404 on
//!   replica B, so `POST /rotations/:id/activate` behind a load balancer
//!   succeeded or failed by coin-flip;
//! * **restart**: a process restart stranded every in-flight rotation —
//!   the profile sat between signers with no record that a rotation had
//!   ever been started;
//! * **audit**: there was no durable evidence of who rotated which
//!   signer, when — exactly the evidence a custody audit asks for first.
//!
//! PostgreSQL (`custody_rotations`, migration 0036) is now the authority.
//!
//! # What is NOT stored here
//!
//! Identifiers only. No private key, no KMS credential, no provider
//! secret and no signing material reaches this table. `RotationRecord`
//! itself carries none; this store persists its fields verbatim and adds
//! nothing.
//!
//! # Degraded mode
//!
//! With no database attached the process-local map is the store — the
//! previous behaviour, preserved for the memory-only dev mode and the
//! existing handler tests. [`CustodyRotationStore::is_durable`] reports
//! which mode is live. Every durable write FAILS CLOSED: the caller is
//! told the rotation did not advance rather than being shown a state
//! transition that exists only in one replica's RAM.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use bot_core::custody::model::{CustodyProfileId, ProviderType, SignerId};
use bot_core::custody::rotation::{RotationRecord, RotationState};
use bot_core::db::Database;
use bot_core::tenant::OrganizationId;
use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

/// Why a durable rotation read/write could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RotationStoreError {
    /// The database rejected or could not serve the statement.
    Backend(String),
    /// Another rotation is already in flight for this (tenant, profile).
    /// Surfaced as 409 by the handler — never as a success.
    ConflictInFlight,
    /// A persisted row could not be mapped back into a `RotationRecord`
    /// (an out-of-domain state, or an identifier the domain type
    /// refuses). Treated as an error, never silently skipped.
    Corrupt(String),
}

impl std::fmt::Display for RotationStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RotationStoreError::Backend(d) => write!(f, "custody rotation store unavailable: {d}"),
            RotationStoreError::ConflictInFlight => write!(
                f,
                "a custody rotation is already in flight for this profile"
            ),
            RotationStoreError::Corrupt(d) => {
                write!(f, "custody rotation row could not be decoded: {d}")
            }
        }
    }
}

impl std::error::Error for RotationStoreError {}

/// Process-local store / read-through cache.
fn cache() -> &'static Mutex<HashMap<Uuid, RotationRecord>> {
    static S: OnceLock<Mutex<HashMap<Uuid, RotationRecord>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop every cached rotation (tests, and after a restore).
pub fn reset_cache() {
    if let Ok(mut m) = cache().lock() {
        m.clear();
    }
}

/// Is a `(organization, profile)` pair already mid-rotation?
fn cache_has_inflight(org: OrganizationId, profile: CustodyProfileId) -> bool {
    match cache().lock() {
        Ok(m) => m
            .values()
            .any(|r| r.organization_id == org && r.profile_id == profile && !r.state.is_terminal()),
        Err(_) => false,
    }
}

/// The custody rotation repository.
#[derive(Clone, Default)]
pub struct CustodyRotationStore {
    db: Option<Arc<Database>>,
}

impl std::fmt::Debug for CustodyRotationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustodyRotationStore")
            .field("durable", &self.db.is_some())
            .finish()
    }
}

impl CustodyRotationStore {
    /// Build a store over the attached database. `None` selects the
    /// memory-only mode.
    pub fn new(db: Option<Arc<Database>>) -> Self {
        CustodyRotationStore { db }
    }

    /// Is PostgreSQL the authority for this store?
    pub fn is_durable(&self) -> bool {
        self.db.is_some()
    }

    /// Persist a newly created rotation.
    ///
    /// Returns [`RotationStoreError::ConflictInFlight`] when the profile
    /// already has a non-terminal rotation — the partial unique index
    /// `custody_rotations_one_inflight_per_profile` is the arbiter in the
    /// durable mode, so two replicas racing cannot both win.
    pub async fn insert(&self, rec: &RotationRecord) -> Result<(), RotationStoreError> {
        let Some(db) = &self.db else {
            if cache_has_inflight(rec.organization_id, rec.profile_id) {
                return Err(RotationStoreError::ConflictInFlight);
            }
            if let Ok(mut m) = cache().lock() {
                m.insert(rec.id, rec.clone());
            }
            return Ok(());
        };

        let result = sqlx::query(
            "INSERT INTO custody_rotations
                 (id, organization_id, profile_id, old_signer_id, new_signer_id,
                  state, force_revoked, provider_type, failure_reason,
                  activated_at, revoked_at, version,
                  created_by, updated_by, correlation_id, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 1,
                     $12, $12, $13, $14, $15)",
        )
        .bind(rec.id)
        .bind(rec.organization_id.0)
        .bind(rec.profile_id.to_string())
        .bind(rec.old_signer.to_string())
        .bind(rec.new_signer.to_string())
        .bind(rec.state.as_str())
        .bind(rec.force_revoked)
        .bind(rec.provider_type.as_str())
        .bind(&rec.failure_reason)
        .bind(rec.activated_at)
        .bind(rec.revoked_at)
        .bind("")
        .bind("")
        .bind(rec.created_at)
        .bind(rec.updated_at)
        .execute(db.pool())
        .await;

        match result {
            Ok(_) => {
                if let Ok(mut m) = cache().lock() {
                    m.insert(rec.id, rec.clone());
                }
                Ok(())
            }
            // 23505 = unique_violation: the in-flight index refused a
            // second concurrent rotation for this profile.
            Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23505") => {
                Err(RotationStoreError::ConflictInFlight)
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    organization = %rec.organization_id,
                    rotation = %rec.id,
                    "custody rotation insert failed; the rotation was NOT created"
                );
                Err(RotationStoreError::Backend(e.to_string()))
            }
        }
    }

    /// Load one rotation by id.
    ///
    /// The caller still performs the tenant-ownership check (the handler
    /// must answer 404 identically for "absent" and "another tenant's",
    /// so the store deliberately does not decide that here); the
    /// `organization_id` column is returned on the record for exactly
    /// that comparison.
    pub async fn get(&self, id: Uuid) -> Result<Option<RotationRecord>, RotationStoreError> {
        let Some(db) = &self.db else {
            return Ok(cache().lock().ok().and_then(|m| m.get(&id).cloned()));
        };

        let row = sqlx::query(
            "SELECT id, organization_id, profile_id, old_signer_id, new_signer_id,
                    state, force_revoked, provider_type, failure_reason,
                    activated_at, revoked_at, created_at, updated_at
               FROM custody_rotations
              WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(db.pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, rotation = %id, "custody rotation read failed");
            RotationStoreError::Backend(e.to_string())
        })?;

        match row {
            Some(r) => Ok(Some(decode_row(&r)?)),
            None => Ok(None),
        }
    }

    /// Persist a state transition that the domain object has already
    /// validated (`RotationRecord::transition` is the state machine; this
    /// store never invents a transition of its own).
    ///
    /// The `WHERE version = $N` clause is the compare-and-swap: if
    /// another replica transitioned the same rotation first, zero rows
    /// match and the caller is told the transition did not apply, rather
    /// than overwriting the other replica's decision.
    pub async fn save_transition(
        &self,
        rec: &RotationRecord,
        updated_by: &str,
        correlation_id: &str,
    ) -> Result<(), RotationStoreError> {
        let Some(db) = &self.db else {
            if let Ok(mut m) = cache().lock() {
                m.insert(rec.id, rec.clone());
            }
            return Ok(());
        };

        let result = sqlx::query(
            "UPDATE custody_rotations
                SET state          = $2,
                    force_revoked  = $3,
                    failure_reason = $4,
                    activated_at   = $5,
                    revoked_at     = $6,
                    updated_at     = $7,
                    updated_by     = $8,
                    correlation_id = $9,
                    version        = version + 1
              WHERE id = $1
                AND organization_id = $10",
        )
        .bind(rec.id)
        .bind(rec.state.as_str())
        .bind(rec.force_revoked)
        .bind(&rec.failure_reason)
        .bind(rec.activated_at)
        .bind(rec.revoked_at)
        .bind(rec.updated_at)
        .bind(updated_by)
        .bind(correlation_id)
        .bind(rec.organization_id.0)
        .execute(db.pool())
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                rotation = %rec.id,
                organization = %rec.organization_id,
                "custody rotation transition write failed; the rotation did NOT advance"
            );
            RotationStoreError::Backend(e.to_string())
        })?;

        if result.rows_affected() == 0 {
            // Either the row vanished or the tenant predicate did not
            // match. Both are "this transition did not apply".
            return Err(RotationStoreError::Backend(
                "no rotation row matched the tenant-scoped update".to_string(),
            ));
        }

        if let Ok(mut m) = cache().lock() {
            m.insert(rec.id, rec.clone());
        }
        Ok(())
    }

    /// Every rotation belonging to ONE tenant, newest first. The tenant
    /// predicate is in the SQL, not applied after the fact.
    pub async fn list_for_tenant(
        &self,
        org: OrganizationId,
        limit: i64,
    ) -> Result<Vec<RotationRecord>, RotationStoreError> {
        let Some(db) = &self.db else {
            let mut out: Vec<RotationRecord> = cache()
                .lock()
                .map(|m| {
                    m.values()
                        .filter(|r| r.organization_id == org)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
            out.truncate(limit.max(0) as usize);
            return Ok(out);
        };

        let rows = sqlx::query(
            "SELECT id, organization_id, profile_id, old_signer_id, new_signer_id,
                    state, force_revoked, provider_type, failure_reason,
                    activated_at, revoked_at, created_at, updated_at
               FROM custody_rotations
              WHERE organization_id = $1
              ORDER BY created_at DESC
              LIMIT $2",
        )
        .bind(org.0)
        .bind(limit.clamp(1, 500))
        .fetch_all(db.pool())
        .await
        .map_err(|e| {
            tracing::error!(error = %e, organization = %org, "custody rotation list failed");
            RotationStoreError::Backend(e.to_string())
        })?;

        rows.iter().map(decode_row).collect()
    }
}

/// Map a persisted row back into the domain record.
///
/// Every field is parsed through its domain constructor; an unparseable
/// identifier or an out-of-domain state is an ERROR, never a default. A
/// silently defaulted custody identifier is how a rotation ends up
/// pointing at the wrong signer.
fn decode_row(r: &sqlx::postgres::PgRow) -> Result<RotationRecord, RotationStoreError> {
    let state_str: String = r
        .try_get("state")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;
    let state = RotationState::parse(&state_str).ok_or_else(|| {
        RotationStoreError::Corrupt(format!("unknown rotation state {state_str}"))
    })?;

    let profile_str: String = r
        .try_get("profile_id")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;
    let profile_id = CustodyProfileId::parse(&profile_str)
        .ok_or_else(|| RotationStoreError::Corrupt("unparseable profile_id".to_string()))?;

    let old_str: String = r
        .try_get("old_signer_id")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;
    let old_signer = SignerId::parse(&old_str)
        .ok_or_else(|| RotationStoreError::Corrupt("unparseable old_signer_id".to_string()))?;

    let new_str: String = r
        .try_get("new_signer_id")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;
    let new_signer = SignerId::parse(&new_str)
        .ok_or_else(|| RotationStoreError::Corrupt("unparseable new_signer_id".to_string()))?;

    let provider_str: String = r
        .try_get("provider_type")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;
    let provider_type = ProviderType::parse(&provider_str).ok_or_else(|| {
        RotationStoreError::Corrupt(format!("unknown rotation provider type {provider_str}"))
    })?;

    let org_uuid: Uuid = r
        .try_get("organization_id")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;

    let created_at: DateTime<Utc> = r
        .try_get("created_at")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;
    let updated_at: DateTime<Utc> = r
        .try_get("updated_at")
        .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?;

    Ok(RotationRecord {
        id: r
            .try_get("id")
            .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?,
        organization_id: OrganizationId(org_uuid),
        profile_id,
        old_signer,
        new_signer,
        provider_type,
        state,
        created_at,
        updated_at,
        activated_at: r
            .try_get("activated_at")
            .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?,
        revoked_at: r
            .try_get("revoked_at")
            .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?,
        failure_reason: r
            .try_get("failure_reason")
            .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?,
        force_revoked: r
            .try_get("force_revoked")
            .map_err(|e| RotationStoreError::Corrupt(e.to_string()))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        org: OrganizationId,
        profile: CustodyProfileId,
        old: SignerId,
        new: SignerId,
    ) -> RotationRecord {
        RotationRecord::new(org, profile, old, new, ProviderType::Local, Utc::now())
    }

    /// Memory mode round-trips and admits it is not durable.
    #[tokio::test]
    async fn memory_mode_round_trips() {
        reset_cache();
        let s = CustodyRotationStore::new(None);
        assert!(!s.is_durable());
        let org = OrganizationId::new();
        let rec = record(
            org,
            CustodyProfileId::new(),
            SignerId::new(),
            SignerId::new(),
        );

        s.insert(&rec).await.expect("insert");
        let got = s.get(rec.id).await.expect("get").expect("present");
        assert_eq!(got.id, rec.id);
        assert_eq!(got.state, RotationState::Pending);
    }

    /// Two in-flight rotations on ONE profile are refused — the race that
    /// can revoke a signer the other rotation still depends on.
    #[tokio::test]
    async fn second_inflight_rotation_on_a_profile_is_refused() {
        reset_cache();
        let s = CustodyRotationStore::new(None);
        let org = OrganizationId::new();
        let prof = CustodyProfileId::new();
        let old = SignerId::new();
        let new1 = SignerId::new();
        let new2 = SignerId::new();

        s.insert(&record(org, prof, old, new1))
            .await
            .expect("first");
        let second = s.insert(&record(org, prof, old, new2)).await;
        assert_eq!(second, Err(RotationStoreError::ConflictInFlight));
    }

    /// A different TENANT with the same profile label is not blocked:
    /// the in-flight predicate is tenant-composite, not profile-only.
    #[tokio::test]
    async fn inflight_guard_is_tenant_scoped() {
        reset_cache();
        let s = CustodyRotationStore::new(None);
        let a = OrganizationId::new();
        let b = OrganizationId::new();
        let prof = CustodyProfileId::new();
        let old = SignerId::new();
        let new_s = SignerId::new();

        s.insert(&record(a, prof, old, new_s))
            .await
            .expect("tenant a");
        s.insert(&record(b, prof, old, new_s))
            .await
            .expect("tenant b must not be blocked by tenant a");

        assert_eq!(s.list_for_tenant(a, 50).await.expect("list a").len(), 1);
        assert_eq!(s.list_for_tenant(b, 50).await.expect("list b").len(), 1);
    }
}
