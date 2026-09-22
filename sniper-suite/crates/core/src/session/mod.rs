//! Durable user sessions (TASK 7A file 08).
//!
//! Authentication before TASK 7A was deployment-oriented: a static API key
//! in a header, checked against a registry built from configuration. A SaaS
//! control plane also needs *human* sessions — created by a login, bound to
//! a tenant, expiring, revocable, and surviving a restart so customers are
//! not logged out whenever the process recycles.
//!
//! | file | concern |
//! |---|---|
//! | `model.rs` | [`SessionRecord`], [`SessionId`], [`SessionState`] — no secret material |
//! | `token.rs` | opaque token generation, SHA-256 lookup hashing, PBKDF2 passwords, constant-time comparison |
//!
//! The rule the whole module exists to enforce: **only hashes are stored**.
//! The plaintext token is produced once, returned to the client once, and
//! never written to the database, a log line or an audit record.

pub mod model;
pub mod token;

pub use model::{SessionId, SessionRecord, SessionState};
pub use token::{
    generate_token, hash_password, hash_token, verify_password, verify_token, GeneratedToken,
};

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::BotResult;
use crate::tenant::{OrganizationId, UserId};

/// Default session lifetime when the caller does not choose one.
pub const DEFAULT_SESSION_TTL_HOURS: i64 = 12;

/// Durable session storage.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Persist a new session.
    async fn create_session(&self, session: &SessionRecord) -> BotResult<()>;

    /// Look a session up by the SHA-256 of the presented token. The
    /// plaintext never reaches the store.
    async fn session_by_hash(&self, token_hash: &str) -> BotResult<Option<SessionRecord>>;

    /// One session by id.
    async fn session(&self, id: SessionId) -> BotResult<Option<SessionRecord>>;

    /// Persist changes (touch, tenant scoping, revocation).
    async fn update_session(&self, session: &SessionRecord) -> BotResult<()>;

    /// Every session of one user (the "active devices" list).
    async fn sessions_of_user(&self, user_id: UserId) -> BotResult<Vec<SessionRecord>>;

    /// Revoke every session of one user (password change, admin action).
    /// Returns how many were revoked.
    async fn revoke_user_sessions(
        &self,
        user_id: UserId,
        reason: &str,
        now: DateTime<Utc>,
    ) -> BotResult<usize>;
}

/// Why a presented session token was refused. Closed vocabulary so the
/// authorization layer can map it to a stable decision reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionRejection {
    /// No token presented.
    Missing,
    /// The hash matched nothing.
    Unknown,
    /// Past its expiry.
    Expired,
    /// Explicitly revoked.
    Revoked,
    /// The session exists but is not bound to the requested tenant.
    WrongTenant,
}

impl SessionRejection {
    /// Every rejection, stable order.
    pub const ALL: [SessionRejection; 5] = [
        SessionRejection::Missing,
        SessionRejection::Unknown,
        SessionRejection::Expired,
        SessionRejection::Revoked,
        SessionRejection::WrongTenant,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionRejection::Missing => "session_missing",
            SessionRejection::Unknown => "session_unknown",
            SessionRejection::Expired => "session_expired",
            SessionRejection::Revoked => "session_revoked",
            SessionRejection::WrongTenant => "session_wrong_tenant",
        }
    }

    /// Inverse of [`SessionRejection::as_str`].
    pub fn parse(s: &str) -> Option<SessionRejection> {
        SessionRejection::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }
}

impl std::fmt::Display for SessionRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validate a loaded session for a request, optionally requiring a tenant.
///
/// Pure: the caller does the I/O, this decides. Returning the record on
/// success keeps the "check then use" pattern from drifting apart.
pub fn validate<'a>(
    session: Option<&'a SessionRecord>,
    required_tenant: Option<OrganizationId>,
    now: DateTime<Utc>,
) -> Result<&'a SessionRecord, SessionRejection> {
    let Some(s) = session else {
        return Err(SessionRejection::Unknown);
    };
    match s.state(now) {
        SessionState::Revoked => return Err(SessionRejection::Revoked),
        SessionState::Expired => return Err(SessionRejection::Expired),
        SessionState::Active => {}
    }
    if let Some(required) = required_tenant {
        if s.organization_id != Some(required) {
            return Err(SessionRejection::WrongTenant);
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn session(now: DateTime<Utc>, org: Option<OrganizationId>) -> SessionRecord {
        let t = generate_token("ses");
        SessionRecord::new(UserId::new(), org, t.hash, t.prefix, Duration::hours(1), now)
    }

    #[test]
    fn validation_covers_every_rejection() {
        let now = Utc::now();
        let org = OrganizationId::new();
        let other = OrganizationId::new();

        assert_eq!(validate(None, None, now), Err(SessionRejection::Unknown));

        let s = session(now, Some(org));
        assert!(validate(Some(&s), None, now).is_ok());
        assert!(validate(Some(&s), Some(org), now).is_ok());
        assert_eq!(
            validate(Some(&s), Some(other), now),
            Err(SessionRejection::WrongTenant)
        );
        assert_eq!(
            validate(Some(&s), None, now + Duration::hours(2)),
            Err(SessionRejection::Expired)
        );

        let mut revoked = session(now, Some(org));
        revoked.revoke("logout", now);
        assert_eq!(
            validate(Some(&revoked), Some(org), now),
            Err(SessionRejection::Revoked)
        );

        // An unscoped session cannot serve a tenant-scoped request.
        let unscoped = session(now, None);
        assert_eq!(
            validate(Some(&unscoped), Some(org), now),
            Err(SessionRejection::WrongTenant)
        );
        assert!(validate(Some(&unscoped), None, now).is_ok());
    }

    #[test]
    fn rejection_vocabulary_round_trips() {
        for r in SessionRejection::ALL {
            assert_eq!(SessionRejection::parse(r.as_str()), Some(r));
        }
        assert_eq!(SessionRejection::parse("nope"), None);
    }
}
