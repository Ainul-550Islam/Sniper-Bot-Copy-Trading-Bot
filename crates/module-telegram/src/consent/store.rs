//! Persistence seam for consents.
//!
//! [`ConsentStore`] is implemented twice: by [`InMemoryConsentStore`] here
//! (used by the unit and flow tests, and by any deployment that has not wired a
//! database) and by the Postgres store the server crate owns
//! (`sniper_suite::telegram_trade_store_pg::PgConsentStore`, tables from
//! migration 0066). Both must satisfy the same invariants:
//!
//! * at most **one** non-revoked consent per `(organization, chat)`;
//! * `insert` revokes a lapsed predecessor instead of silently stacking rows;
//! * `insert` refuses while a live consent exists (the operator must revoke);
//! * `revoke` is idempotent and immediate.
//!
//! [`StoreError`] is the single error type shared by every store in this crate
//! (consents, commands, audit) so callers handle "the store is down" exactly
//! once — always by denying.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::consent::model::Consent;

/// Any store failure. Never carries secrets: a driver message is truncated and
/// scrubbed before it reaches an audit row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The backing store is unreachable or refused the operation.
    Unavailable(String),
    /// A uniqueness/consistency rule was violated (e.g. a live consent
    /// already exists for this chat).
    Conflict(String),
}

impl StoreError {
    /// Stable machine code for audit rows.
    pub fn code(&self) -> &'static str {
        match self {
            StoreError::Unavailable(_) => "store_unavailable",
            StoreError::Conflict(_) => "store_conflict",
        }
    }

    /// The (already scrubbed) human detail.
    pub fn detail(&self) -> &str {
        match self {
            StoreError::Unavailable(d) | StoreError::Conflict(d) => d,
        }
    }
}

/// Durable consents.
#[async_trait]
pub trait ConsentStore: Send + Sync {
    /// The newest **non-revoked** consent for this chat, expired or not.
    ///
    /// Returning an expired row (instead of `None`) is deliberate: it lets
    /// [`crate::consent::policy::require_active`] explain "expired" rather
    /// than "never granted".
    async fn active(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError>;

    /// Persist a new consent. Fails with [`StoreError::Conflict`] when a live
    /// (unexpired, unrevoked) consent already exists for the chat; revokes a
    /// lapsed predecessor first so the "one active row" invariant holds.
    async fn insert(&self, consent: &Consent) -> Result<(), StoreError>;

    /// Revoke whatever consent is live for this chat, returning it.
    ///
    /// Idempotent: revoking twice returns `Ok(None)` the second time. The
    /// caller is responsible for also cancelling the chat's pending commands.
    async fn revoke(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError>;
}

/// A process-local [`ConsentStore`], authoritative for tests and for
/// deployments without a database.
#[derive(Debug, Default)]
pub struct InMemoryConsentStore {
    rows: Mutex<Vec<Consent>>,
}

impl InMemoryConsentStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every row, newest first (diagnostics and tests).
    pub fn all(&self) -> Vec<Consent> {
        let mut rows = self.rows.lock().expect("consent store lock").clone();
        rows.sort_by_key(|c| std::cmp::Reverse(c.granted_at));
        rows
    }

    fn next_revoked(rows: &[Consent], organization_id: &str, chat_id: i64) -> Option<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, c)| {
                c.organization_id == organization_id
                    && c.chat_id == chat_id
                    && c.revoked_at.is_none()
            })
            .max_by_key(|(_, c)| c.granted_at)
            .map(|(i, _)| i)
    }
}

#[async_trait]
impl ConsentStore for InMemoryConsentStore {
    async fn active(
        &self,
        organization_id: &str,
        chat_id: i64,
        _now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError> {
        let rows = self.rows.lock().expect("consent store lock");
        Ok(
            InMemoryConsentStore::next_revoked(&rows, organization_id, chat_id)
                .and_then(|i| rows.get(i).cloned()),
        )
    }

    async fn insert(&self, consent: &Consent) -> Result<(), StoreError> {
        let mut rows = self.rows.lock().expect("consent store lock");
        if let Some(i) =
            InMemoryConsentStore::next_revoked(&rows, &consent.organization_id, consent.chat_id)
        {
            let existing = &mut rows[i];
            if consent.granted_at < existing.expires_at {
                return Err(StoreError::Conflict(
                    "a live consent already exists for this chat".into(),
                ));
            }
            // Lapsed predecessor: retire it, then store the new grant.
            existing.revoked_at = Some(consent.granted_at);
        }
        rows.push(consent.clone());
        Ok(())
    }

    async fn revoke(
        &self,
        organization_id: &str,
        chat_id: i64,
        now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError> {
        let mut rows = self.rows.lock().expect("consent store lock");
        let Some(i) = InMemoryConsentStore::next_revoked(&rows, organization_id, chat_id) else {
            return Ok(None);
        };
        rows[i].revoked_at = Some(now);
        Ok(Some(rows[i].clone()))
    }
}

/// A store that always fails — the fail-closed double used to prove that a
/// database outage denies trading instead of allowing it.
#[derive(Debug, Default)]
pub struct BrokenConsentStore {
    /// The detail surfaced on every call.
    pub detail: String,
}

impl BrokenConsentStore {
    /// Build one with a fixed detail string.
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    fn err(&self) -> StoreError {
        StoreError::Unavailable(if self.detail.is_empty() {
            "consent store unavailable".to_string()
        } else {
            self.detail.clone()
        })
    }
}

#[async_trait]
impl ConsentStore for BrokenConsentStore {
    async fn active(
        &self,
        _organization_id: &str,
        _chat_id: i64,
        _now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError> {
        Err(self.err())
    }

    async fn insert(&self, _consent: &Consent) -> Result<(), StoreError> {
        Err(self.err())
    }

    async fn revoke(
        &self,
        _organization_id: &str,
        _chat_id: i64,
        _now: DateTime<Utc>,
    ) -> Result<Option<Consent>, StoreError> {
        Err(self.err())
    }
}

/// Count helper kept next to the trait it documents: a store must never let two
/// live rows coexist, and this is how the tests assert it.
pub fn live_consents_per_chat(rows: &[Consent]) -> HashMap<(String, i64), usize> {
    let mut counts: HashMap<(String, i64), usize> = HashMap::new();
    for row in rows.iter().filter(|c| c.revoked_at.is_none()) {
        *counts
            .entry((row.organization_id.clone(), row.chat_id))
            .or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::model::ConsentScope;
    use uuid::Uuid;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(secs, 0).unwrap()
    }

    fn consent(chat: i64, granted: i64, expires: i64) -> Consent {
        Consent {
            consent_id: Uuid::new_v4(),
            organization_id: "org-1".into(),
            user_id: 42,
            chat_id: chat,
            scope: ConsentScope::both(),
            max_order_lamports: 50_000_000,
            daily_cap_lamports: 200_000_000,
            granted_at: at(granted),
            expires_at: at(expires),
            revoked_at: None,
            phrase_sha256: [7u8; 32],
        }
    }

    #[tokio::test]
    async fn insert_then_read_back() {
        let store = InMemoryConsentStore::new();
        assert!(store
            .active("org-1", 100, at(1_000))
            .await
            .unwrap()
            .is_none());
        let c = consent(100, 1_000, 4_600);
        store.insert(&c).await.unwrap();
        assert_eq!(
            store.active("org-1", 100, at(2_000)).await.unwrap(),
            Some(c)
        );
        // Another tenant cannot see it.
        assert!(store
            .active("org-2", 100, at(2_000))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn only_one_live_consent_per_chat() {
        let store = InMemoryConsentStore::new();
        store.insert(&consent(100, 1_000, 4_600)).await.unwrap();
        let err = store
            .insert(&consent(100, 2_000, 5_600))
            .await
            .expect_err("second live grant must be refused");
        assert_eq!(err.code(), "store_conflict");
        assert_eq!(
            live_consents_per_chat(&store.all())[&("org-1".to_string(), 100)],
            1
        );
    }

    #[tokio::test]
    async fn lapsed_consent_is_retired_by_the_next_grant() {
        let store = InMemoryConsentStore::new();
        store.insert(&consent(100, 1_000, 2_000)).await.unwrap();
        // granted_at (3_000) is past the predecessor's expiry (2_000).
        store.insert(&consent(100, 3_000, 9_000)).await.unwrap();
        let rows = store.all();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            live_consents_per_chat(&rows)[&("org-1".to_string(), 100)],
            1
        );
        let active = store
            .active("org-1", 100, at(4_000))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.granted_at, at(3_000));
    }

    #[tokio::test]
    async fn revoke_is_immediate_and_idempotent() {
        let store = InMemoryConsentStore::new();
        store.insert(&consent(100, 1_000, 4_600)).await.unwrap();
        let revoked = store.revoke("org-1", 100, at(2_000)).await.unwrap();
        assert!(revoked.is_some());
        assert_eq!(revoked.unwrap().revoked_at, Some(at(2_000)));
        assert!(store
            .revoke("org-1", 100, at(2_100))
            .await
            .unwrap()
            .is_none());
        // Revoked rows stay readable as revoked, never as active.
        let row = store.active("org-1", 100, at(2_000)).await.unwrap();
        assert!(row.is_none(), "revoked consents are not 'active'");
        assert_eq!(store.all()[0].revoked_at, Some(at(2_000)));
        // A new grant is allowed now.
        store.insert(&consent(100, 3_000, 7_000)).await.unwrap();
    }

    #[tokio::test]
    async fn broken_store_always_errors() {
        let store = BrokenConsentStore::new("postgres down");
        assert_eq!(
            store.active("org-1", 1, at(0)).await.unwrap_err().code(),
            "store_unavailable"
        );
        assert!(store.insert(&consent(1, 0, 10)).await.is_err());
        assert!(store.revoke("org-1", 1, at(0)).await.is_err());
        assert_eq!(StoreError::Unavailable("x".into()).detail(), "x");
        assert_eq!(BrokenConsentStore::default().detail, "");
        assert_eq!(
            BrokenConsentStore::default()
                .active("o", 1, at(0))
                .await
                .unwrap_err()
                .detail(),
            "consent store unavailable"
        );
    }
}
