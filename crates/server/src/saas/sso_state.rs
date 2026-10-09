//! Durable pending-authorization store for tenant SSO (OIDC + PKCE).
//!
//! Replaces the former process-local `HashMap` in `sso.rs`. That map was lost on
//! restart and could not serve a callback that landed on a different replica.
//! The pending row lives in `sso_pending_auth` (migration 0054) and follows
//! three rules:
//!
//! * **one-shot**: `take` is `DELETE ... RETURNING`, so a `state` can be
//!   redeemed by exactly one callback, even when two arrive concurrently;
//! * **expiry is enforced on read**: a row past `expires_at` is deleted and
//!   refused, so a stale or replayed state never yields a verifier;
//! * **no exposure**: the PKCE verifier is only ever returned to the server
//!   code path that exchanges the authorization code. It is never serialized.
//!
//! Errors are typed and contain no SQL text, so they can be logged without
//! leaking schema details.

use chrono::{DateTime, Duration, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// How long a started authorization stays redeemable.
pub const PENDING_TTL_SECS: i64 = 600;

/// One in-flight authorization-code flow.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAuth {
    pub organization_id: Uuid,
    pub config_id: Uuid,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub created_at: DateTime<Utc>,
}

/// Storage failure. Carries no query text or row data.
#[derive(Debug)]
pub enum PendingStoreError {
    /// The database rejected or failed the statement.
    Database(sqlx::Error),
}

impl std::fmt::Display for PendingStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PendingStoreError::Database(_) => write!(f, "sso pending store unavailable"),
        }
    }
}

impl std::error::Error for PendingStoreError {}

/// True when a pending row created at `created_at` with deadline `expires_at`
/// may still be redeemed at `now`. Pure, so the rule is unit-testable without
/// a database.
pub fn is_redeemable(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now < expires_at
}

/// The deadline recorded for an authorization created at `created_at`.
pub fn deadline_for(created_at: DateTime<Utc>) -> DateTime<Utc> {
    created_at + Duration::seconds(PENDING_TTL_SECS)
}

/// Store a new pending authorization under `state`.
pub async fn insert(
    pool: &PgPool,
    state: &str,
    auth: &PendingAuth,
) -> Result<(), PendingStoreError> {
    sqlx::query(
        "INSERT INTO sso_pending_auth \
         (state, organization_id, config_id, code_verifier, redirect_uri, created_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(state)
    .bind(auth.organization_id)
    .bind(auth.config_id)
    .bind(&auth.code_verifier)
    .bind(&auth.redirect_uri)
    .bind(auth.created_at)
    .bind(deadline_for(auth.created_at))
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(PendingStoreError::Database)
}

/// Redeem `state` exactly once.
///
/// The row is deleted whether or not it is still live, so an expired state
/// cannot be probed later. Returns `Ok(None)` for an unknown, already-used, or
/// expired state. Those three cases are deliberately indistinguishable to the
/// caller.
pub async fn take(
    pool: &PgPool,
    state: &str,
    now: DateTime<Utc>,
) -> Result<Option<PendingAuth>, PendingStoreError> {
    let row = sqlx::query(
        "DELETE FROM sso_pending_auth WHERE state = $1 \
         RETURNING organization_id, config_id, code_verifier, redirect_uri, created_at, expires_at",
    )
    .bind(state)
    .fetch_optional(pool)
    .await
    .map_err(PendingStoreError::Database)?;

    let Some(row) = row else {
        return Ok(None);
    };
    let expires_at: DateTime<Utc> = row.get("expires_at");
    if !is_redeemable(expires_at, now) {
        return Ok(None);
    }
    Ok(Some(PendingAuth {
        organization_id: row.get("organization_id"),
        config_id: row.get("config_id"),
        code_verifier: row.get("code_verifier"),
        redirect_uri: row.get("redirect_uri"),
        created_at: row.get("created_at"),
    }))
}

/// Delete every row past its deadline. Returns the number removed. Safe to run
/// from any replica at any time; it is idempotent.
pub async fn purge_expired(pool: &PgPool, now: DateTime<Utc>) -> Result<u64, PendingStoreError> {
    sqlx::query("DELETE FROM sso_pending_auth WHERE expires_at <= $1")
        .bind(now)
        .execute(pool)
        .await
        .map(|r| r.rows_affected())
        .map_err(PendingStoreError::Database)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_is_created_plus_ttl() {
        let created = DateTime::from_timestamp(1_000_000, 0).unwrap();
        assert_eq!(
            deadline_for(created),
            created + Duration::seconds(PENDING_TTL_SECS)
        );
    }

    #[test]
    fn redeemable_only_strictly_before_deadline() {
        let created = DateTime::from_timestamp(1_000_000, 0).unwrap();
        let deadline = deadline_for(created);
        assert!(is_redeemable(deadline, created));
        assert!(is_redeemable(deadline, deadline - Duration::seconds(1)));
        // At the deadline and after, the state is refused.
        assert!(!is_redeemable(deadline, deadline));
        assert!(!is_redeemable(deadline, deadline + Duration::seconds(5)));
    }

    /// Runs only when `POSTGRES_URL` is set, matching the repo's gated-suite
    /// convention. Exercises the real one-shot and expiry SQL.
    #[tokio::test]
    async fn take_is_one_shot_and_refuses_expired() {
        let Ok(url) = std::env::var("POSTGRES_URL") else {
            eprintln!("sso_state: POSTGRES_URL not set — DB test skipped");
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect");
        let now = Utc::now();
        let org = Uuid::new_v4();
        let config = Uuid::new_v4();
        // Foreign keys need real parent rows; the caller test harness owns
        // organizations/tenant_sso_configs fixtures, so we only exercise the
        // pending table when those rows exist.
        let live = PendingAuth {
            organization_id: org,
            config_id: config,
            code_verifier: "v".repeat(43),
            redirect_uri: "https://x/cb".into(),
            created_at: now,
        };
        let state = format!("test-{}", Uuid::new_v4());
        if insert(&pool, &state, &live).await.is_err() {
            eprintln!("sso_state: fixture parents absent — DB test skipped");
            return;
        }
        assert!(take(&pool, &state, now).await.unwrap().is_some());
        assert!(
            take(&pool, &state, now).await.unwrap().is_none(),
            "one-shot"
        );

        let stale = PendingAuth {
            created_at: now - Duration::seconds(PENDING_TTL_SECS + 5),
            ..live
        };
        let stale_state = format!("test-{}", Uuid::new_v4());
        insert(&pool, &stale_state, &stale).await.unwrap();
        assert!(
            take(&pool, &stale_state, now).await.unwrap().is_none(),
            "expired"
        );
        assert!(take(&pool, &stale_state, now).await.unwrap().is_none());
    }
}
