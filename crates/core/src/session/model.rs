//! Durable session records (TASK 7A file 09).
//!
//! A session is "this browser/user is logged in, acting for this tenant,
//! until this moment". It is durable so a control-plane restart does not
//! log every customer out, and it stores only the SHA-256 of the token, so
//! a database copy cannot be replayed as a login.
//!
//! The record deliberately has no `token` field at all: there is no way to
//! serialise a session and leak the secret, because the struct never holds
//! it. The plaintext exists once, in
//! [`crate::session::token::GeneratedToken`], at creation time.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tenant::{OrganizationId, UserId};

/// A session identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// A fresh identifier.
    pub fn new() -> Self {
        SessionId(Uuid::new_v4())
    }

    /// The inner UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// Parse the canonical string form.
    pub fn parse(s: &str) -> Option<Self> {
        Uuid::parse_str(s.trim()).ok().map(SessionId)
    }
}

impl Default for SessionId {
    fn default() -> Self {
        SessionId::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The state a session is in at a given instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Usable.
    Active,
    /// Past its expiry.
    Expired,
    /// Explicitly revoked (logout, password change, admin action).
    Revoked,
}

impl SessionState {
    /// Every state, stable order.
    pub const ALL: [SessionState; 3] = [
        SessionState::Active,
        SessionState::Expired,
        SessionState::Revoked,
    ];

    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Active => "active",
            SessionState::Expired => "expired",
            SessionState::Revoked => "revoked",
        }
    }

    /// Inverse of [`SessionState::as_str`].
    pub fn parse(s: &str) -> Option<SessionState> {
        SessionState::ALL
            .iter()
            .copied()
            .find(|x| x.as_str() == s.trim())
    }

    /// Only an active session authenticates.
    pub fn is_usable(&self) -> bool {
        matches!(self, SessionState::Active)
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One durable session. Contains no secret material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Row identity.
    pub id: SessionId,
    /// The authenticated human.
    pub user_id: UserId,
    /// The tenant this session is acting for. `None` right after login,
    /// before a tenant is selected; a tenant-scoped request with `None`
    /// here is denied by the authorization layer, never defaulted.
    pub organization_id: Option<OrganizationId>,
    /// SHA-256 of the session token. NEVER the token.
    pub token_hash: String,
    /// Public, non-secret prefix for log correlation.
    pub token_prefix: String,
    /// Client user agent (diagnostics).
    pub user_agent: String,
    /// Client address (diagnostics).
    pub ip: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last time the session was used.
    pub last_seen_at: DateTime<Utc>,
    /// Absolute expiry.
    pub expires_at: DateTime<Utc>,
    /// When it was revoked, if it was.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Why it was revoked.
    pub revoke_reason: String,
}

impl SessionRecord {
    /// Build a session for an already-generated token. The caller keeps the
    /// plaintext and hands it to the client exactly once.
    pub fn new(
        user_id: UserId,
        organization_id: Option<OrganizationId>,
        token_hash: impl Into<String>,
        token_prefix: impl Into<String>,
        ttl: Duration,
        now: DateTime<Utc>,
    ) -> Self {
        SessionRecord {
            id: SessionId::new(),
            user_id,
            organization_id,
            token_hash: token_hash.into(),
            token_prefix: token_prefix.into(),
            user_agent: String::new(),
            ip: String::new(),
            created_at: now,
            last_seen_at: now,
            expires_at: now + ttl,
            revoked_at: None,
            revoke_reason: String::new(),
        }
    }

    /// The session's state at `now`. Revocation wins over expiry so an
    /// audit line says why it stopped working.
    pub fn state(&self, now: DateTime<Utc>) -> SessionState {
        if self.revoked_at.is_some() {
            SessionState::Revoked
        } else if self.expires_at <= now {
            SessionState::Expired
        } else {
            SessionState::Active
        }
    }

    /// May this session authenticate a request at `now`?
    pub fn is_usable(&self, now: DateTime<Utc>) -> bool {
        self.state(now).is_usable()
    }

    /// Seconds until expiry (negative once expired).
    pub fn ttl_secs(&self, now: DateTime<Utc>) -> i64 {
        self.expires_at.signed_duration_since(now).num_seconds()
    }

    /// Mark the session used (sliding `last_seen`, absolute expiry stays).
    pub fn touch(&mut self, now: DateTime<Utc>) {
        self.last_seen_at = now;
    }

    /// Revoke it. Idempotent: the first reason is kept.
    pub fn revoke(&mut self, reason: impl Into<String>, now: DateTime<Utc>) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        self.revoked_at = Some(now);
        self.revoke_reason = reason.into();
        true
    }

    /// Bind the session to a tenant (tenant selection after login).
    pub fn scope_to(&mut self, organization_id: OrganizationId, now: DateTime<Utc>) {
        self.organization_id = Some(organization_id);
        self.last_seen_at = now;
    }

    /// Single-line audit text — prefix only, never the token.
    pub fn summary(&self, now: DateTime<Utc>) -> String {
        format!(
            "session={} user={} organization={} prefix={} state={} ttl_secs={}",
            self.id,
            self.user_id,
            self.organization_id
                .map(|o| o.to_string())
                .unwrap_or_else(|| "-".into()),
            self.token_prefix,
            self.state(now),
            self.ttl_secs(now)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::token::generate_token;

    fn session(ttl_secs: i64, now: DateTime<Utc>) -> SessionRecord {
        let t = generate_token("ses");
        SessionRecord::new(
            UserId::new(),
            None,
            t.hash,
            t.prefix,
            Duration::seconds(ttl_secs),
            now,
        )
    }

    #[test]
    fn state_is_a_pure_function_of_the_clock() {
        let t0 = Utc::now();
        let s = session(3600, t0);
        assert_eq!(s.state(t0), SessionState::Active);
        assert!(s.is_usable(t0 + Duration::seconds(3599)));
        assert_eq!(s.state(t0 + Duration::seconds(3600)), SessionState::Expired);
        assert!(!s.is_usable(t0 + Duration::seconds(3601)));
        assert_eq!(s.ttl_secs(t0 + Duration::seconds(600)), 3000);
    }

    #[test]
    fn revocation_wins_over_expiry_and_is_idempotent() {
        let t0 = Utc::now();
        let mut s = session(3600, t0);
        assert!(s.revoke("logout", t0));
        assert!(!s.revoke("again", t0), "already revoked");
        assert_eq!(s.revoke_reason, "logout");
        assert_eq!(s.state(t0), SessionState::Revoked);
        assert_eq!(
            s.state(t0 + Duration::days(30)),
            SessionState::Revoked,
            "a revoked session never becomes merely expired"
        );
        assert!(!s.is_usable(t0));
    }

    #[test]
    fn the_record_never_carries_the_token() {
        let t = generate_token("ses");
        let s = SessionRecord::new(
            UserId::new(),
            Some(OrganizationId::new()),
            t.hash.clone(),
            t.prefix.clone(),
            Duration::hours(1),
            Utc::now(),
        );
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            !json.contains(&t.plaintext),
            "the plaintext token must never be serialisable"
        );
        assert!(json.contains(&t.hash));
        assert!(!s.summary(Utc::now()).contains(&t.plaintext));
    }

    #[test]
    fn tenant_scoping_is_explicit() {
        let t0 = Utc::now();
        let mut s = session(3600, t0);
        assert!(s.organization_id.is_none(), "not scoped until chosen");
        let org = OrganizationId::new();
        s.scope_to(org, t0);
        assert_eq!(s.organization_id, Some(org));
        s.touch(t0 + Duration::seconds(10));
        assert_eq!(s.last_seen_at, t0 + Duration::seconds(10));
        assert_eq!(
            s.expires_at,
            t0 + Duration::seconds(3600),
            "absolute expiry"
        );
    }

    #[test]
    fn vocabulary_round_trips() {
        for s in SessionState::ALL {
            assert_eq!(SessionState::parse(s.as_str()), Some(s));
        }
        assert_eq!(SessionState::parse("nope"), None);
        let id = SessionId::new();
        assert_eq!(SessionId::parse(&id.to_string()), Some(id));
        assert_eq!(SessionId::parse("x"), None);
    }
}
