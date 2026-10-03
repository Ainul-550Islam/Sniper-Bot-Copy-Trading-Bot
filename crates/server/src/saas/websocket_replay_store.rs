//! Shared, cross-replica WebSocket ticket replay protection.
//!
//! # Why this module exists
//!
//! `saas::websocket_auth::is_replay` is backed by a process-local
//! `HashMap<token_hash, Instant>`. Its own doc comment already said the
//! quiet part out loud: *"in a multi-replica deployment a replay simply
//! goes to another replica"*. A captured WebSocket ticket replays
//! successfully by reaching any replica that has not seen it — which,
//! behind a round-robin load balancer with N replicas, is almost always.
//!
//! This store makes the seen-set shared: PostgreSQL (`ws_replay_tokens`,
//! migration 0036) holds the hashes, so the FIRST replica to claim a
//! ticket wins and every other replica refuses it.
//!
//! # The claim is atomic
//!
//! "Has this been seen?" followed by "mark it seen" is a TOCTOU race —
//! two replicas can both read "unseen" and both admit the socket. There
//! is therefore no read-then-write path here: [`WebsocketReplayStore::claim`]
//! is a single `INSERT … ON CONFLICT DO UPDATE … RETURNING` statement.
//! The row is returned only when this caller actually took the claim, so
//! exactly one caller can ever be told "fresh".
//!
//! # Expiry is a predicate, never a sweep dependency
//!
//! A row past `expires_at` is garbage, not an "already seen" answer: the
//! conflict branch re-claims it. That means an un-swept table can never
//! start refusing legitimate fresh tickets, and
//! [`WebsocketReplayStore::sweep_expired`] is a space optimisation rather
//! than a correctness requirement.
//!
//! # Fail-closed
//!
//! A database error is treated as REPLAY (refuse the socket), matching
//! the poisoned-mutex policy the in-memory path already uses. Replay
//! protection that switches itself off when its backing store hiccups is
//! not replay protection.
//!
//! # No plaintext
//!
//! Only the SHA-256 hex digest of the ticket is stored. The plaintext
//! token never reaches this table, its indexes, or the write-ahead log.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use bot_core::db::Database;
use bot_core::tenant::OrganizationId;
use chrono::Utc;

/// The outcome of claiming a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// This caller took the claim: the ticket had not been presented
    /// inside its replay window. Admit the socket.
    Fresh,
    /// The ticket was already claimed (by this replica or another one).
    /// Refuse the socket.
    Replay,
    /// The authoritative store could not be consulted. Refuse the socket
    /// — see the fail-closed note on this module.
    Unavailable,
}

impl ClaimOutcome {
    /// Should the caller refuse the connection?
    pub fn is_refusal(self) -> bool {
        !matches!(self, ClaimOutcome::Fresh)
    }
}

/// Process-local fallback set (memory-only deployments and tests).
fn local_seen() -> &'static Mutex<HashMap<String, Instant>> {
    static S: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Drop every locally remembered ticket.
pub fn reset_local() {
    if let Ok(mut m) = local_seen().lock() {
        m.clear();
    }
}

/// SHA-256 hex of the ticket. Never store or log the plaintext.
pub fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// The shared replay-protection repository.
#[derive(Clone, Default)]
pub struct WebsocketReplayStore {
    db: Option<Arc<Database>>,
}

impl std::fmt::Debug for WebsocketReplayStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebsocketReplayStore")
            .field("shared", &self.db.is_some())
            .finish()
    }
}

impl WebsocketReplayStore {
    /// Build a store over the attached database. `None` selects the
    /// process-local mode, which is correct ONLY for a single-replica or
    /// development deployment.
    pub fn new(db: Option<Arc<Database>>) -> Self {
        WebsocketReplayStore { db }
    }

    /// Is the seen-set shared across replicas?
    ///
    /// `false` means replay protection is per-process and a multi-replica
    /// deployment is NOT protected. The readiness surface reports this.
    pub fn is_shared(&self) -> bool {
        self.db.is_some()
    }

    /// Atomically claim a ticket for the duration of `window`.
    ///
    /// `org` is recorded for audit only; it is deliberately NOT part of
    /// the key, because a ticket that is replayable once per tenant is
    /// still replayable.
    pub async fn claim(
        &self,
        token: &str,
        window: Duration,
        org: Option<OrganizationId>,
        correlation_id: &str,
    ) -> ClaimOutcome {
        let hash = token_hash(token);

        let Some(db) = &self.db else {
            return claim_local(&hash, window);
        };

        let expires_at = Utc::now()
            + chrono::Duration::from_std(window).unwrap_or_else(|_| chrono::Duration::seconds(300));

        // ONE statement, no read-then-write. The conflict branch re-claims
        // the row only when the previous claim has expired; otherwise the
        // WHERE fails, no row is returned, and this caller is a replay.
        let claimed = sqlx::query_scalar::<_, bool>(
            "INSERT INTO ws_replay_tokens
                 (token_hash, organization_id, first_seen_at, expires_at, correlation_id)
             VALUES ($1, $2, now(), $3, $4)
             ON CONFLICT (token_hash) DO UPDATE
                SET first_seen_at   = now(),
                    expires_at      = EXCLUDED.expires_at,
                    organization_id = EXCLUDED.organization_id,
                    correlation_id  = EXCLUDED.correlation_id
              WHERE ws_replay_tokens.expires_at <= now()
             RETURNING true",
        )
        .bind(&hash)
        .bind(org.map(|o| o.0))
        .bind(expires_at)
        .bind(correlation_id)
        .fetch_optional(db.pool())
        .await;

        match claimed {
            Ok(Some(_)) => ClaimOutcome::Fresh,
            Ok(None) => ClaimOutcome::Replay,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "websocket replay store unavailable; refusing the socket (fail-closed)"
                );
                ClaimOutcome::Unavailable
            }
        }
    }

    /// Delete expired claims. Correctness does not depend on this running
    /// (see the module docs); it bounds the table's size.
    ///
    /// Returns the number of rows removed.
    pub async fn sweep_expired(&self) -> u64 {
        let Some(db) = &self.db else {
            if let Ok(mut m) = local_seen().lock() {
                let now = Instant::now();
                let before = m.len();
                m.retain(|_, seen| now.duration_since(*seen) < Duration::from_secs(300));
                return (before - m.len()) as u64;
            }
            return 0;
        };

        match sqlx::query("DELETE FROM ws_replay_tokens WHERE expires_at <= now()")
            .execute(db.pool())
            .await
        {
            Ok(r) => r.rows_affected(),
            Err(e) => {
                tracing::warn!(error = %e, "websocket replay sweep failed; table will be swept next cycle");
                0
            }
        }
    }
}

/// Process-local claim with the same atomicity property (the mutex is the
/// arbiter) and the same fail-closed policy on a poisoned lock.
fn claim_local(hash: &str, window: Duration) -> ClaimOutcome {
    let now = Instant::now();
    let mut seen = match local_seen().lock() {
        Ok(g) => g,
        // A panic in another thread must not switch replay protection off.
        Err(_) => return ClaimOutcome::Unavailable,
    };
    // Expire by age first: an entry older than the ticket's own validity
    // window can no longer be replayed successfully anyway.
    seen.retain(|_, first_seen| now.duration_since(*first_seen) < window);
    if seen.contains_key(hash) {
        return ClaimOutcome::Replay;
    }
    seen.insert(hash.to_string(), now);
    ClaimOutcome::Fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(300);

    #[tokio::test]
    async fn first_claim_is_fresh_and_the_second_is_a_replay() {
        reset_local();
        let s = WebsocketReplayStore::new(None);
        assert!(!s.is_shared());

        assert_eq!(
            s.claim("ticket-abc", WINDOW, None, "").await,
            ClaimOutcome::Fresh
        );
        assert_eq!(
            s.claim("ticket-abc", WINDOW, None, "").await,
            ClaimOutcome::Replay
        );
    }

    /// The same ticket presented by a DIFFERENT tenant is still a replay.
    /// Tenant-scoping this set would make a captured ticket reusable once
    /// per tenant.
    #[tokio::test]
    async fn replay_detection_is_not_tenant_scoped() {
        reset_local();
        let s = WebsocketReplayStore::new(None);
        let a = OrganizationId::new();
        let b = OrganizationId::new();

        assert_eq!(
            s.claim("shared-ticket", WINDOW, Some(a), "").await,
            ClaimOutcome::Fresh
        );
        assert_eq!(
            s.claim("shared-ticket", WINDOW, Some(b), "").await,
            ClaimOutcome::Replay
        );
    }

    /// Distinct tickets do not collide, and the stored key is a digest
    /// rather than the plaintext.
    #[tokio::test]
    async fn distinct_tickets_are_independent_and_hashed() {
        reset_local();
        let s = WebsocketReplayStore::new(None);

        assert_eq!(s.claim("one", WINDOW, None, "").await, ClaimOutcome::Fresh);
        assert_eq!(s.claim("two", WINDOW, None, "").await, ClaimOutcome::Fresh);

        let stored: Vec<String> = local_seen().lock().expect("lock").keys().cloned().collect();
        assert!(!stored.iter().any(|k| k == "one" || k == "two"));
        assert!(stored.iter().all(|k| k.len() == 64));
        assert!(stored.contains(&token_hash("one")));
    }

    /// Every non-fresh outcome must refuse the socket — including the
    /// "store unavailable" one.
    #[test]
    fn only_fresh_admits_a_socket() {
        assert!(!ClaimOutcome::Fresh.is_refusal());
        assert!(ClaimOutcome::Replay.is_refusal());
        assert!(ClaimOutcome::Unavailable.is_refusal());
    }
}
