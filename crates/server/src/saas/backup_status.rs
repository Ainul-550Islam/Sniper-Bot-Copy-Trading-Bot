//! Customer-facing data-protection status.
//!
//! # What changed, and why it mattered
//!
//! This handler used to return literals: `retention_configured: true`,
//! `last_backup_at: Utc::now()`, `protection: "encrypted_at_rest"` —
//! with no backup machinery anywhere in the deployment. It compiled, it
//! passed its unit test, it answered 200, and it told a paying customer
//! their data was protected when nothing was being backed up at all.
//!
//! It now reports only what the append-only backup ledger proves (see
//! [`crate::ops::backup_ledger`]). A deployment with no ledger answers
//! `not_configured`, which is the true answer.
//!
//! The response never carries a path, a bucket, a hostname or a backup
//! id: the customer-relevant facts are freshness and whether a restore
//! has actually been proven; where the copies live is operator
//! information and a target list.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::ops::backup_ledger;
use crate::saas::middleware::{authorize_request, deny_response};

/// `GET /api/saas/backup/status`.
pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/backup/status", axum::routing::get(handler))
}

async fn handler(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read(Permission::BillingRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let posture = backup_ledger::posture();
    let body = posture.to_json(&ctx.organization.id.to_string());

    // 200 in every case, including `not_configured`: this endpoint
    // reports a posture, it is not a health probe, and a customer
    // polling it must be able to read the state rather than an error
    // page. The honesty lives in the payload — `protected: false` — not
    // in the status code.
    (StatusCode::OK, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    /// The old failure mode, pinned: the body must never claim a backup
    /// exists unless the ledger says so.
    #[test]
    fn an_absent_ledger_never_claims_protection() {
        // No BACKUP_LEDGER_PATH is set in the test environment, and the
        // default path does not exist inside the test runner.
        let posture = backup_ledger::posture_at(
            std::path::Path::new("/nonexistent/backups.jsonl"),
            Utc::now(),
        );
        let body = posture.to_json("org");
        assert_eq!(body["state"], "not_configured");
        assert_eq!(body["protected"], false);
        assert!(body["last_backup_at"].is_null());
        assert!(body["last_verified_restore"].is_null());
    }

    #[test]
    fn the_body_carries_no_storage_location() {
        let posture = backup_ledger::posture_at(
            std::path::Path::new("/nonexistent/backups.jsonl"),
            Utc::now(),
        );
        let body = posture.to_json("org").to_string().to_ascii_lowercase();
        for banned in ["s3://", "gs://", "postgres://", "/var/", "/app/data"] {
            assert!(!body.contains(banned), "leaked {banned}");
        }
    }
}
