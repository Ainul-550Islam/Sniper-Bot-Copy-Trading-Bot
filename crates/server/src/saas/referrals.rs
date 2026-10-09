//! Referral codes and attribution (GAP-MAP v2, P2; durable schema in
//! migration 0049).
//!
//! Routes (every one: authorize → org-scope → DB → audit → test):
//! * `POST   /api/saas/referrals/codes` — mint a code for THIS org;
//! * `GET    /api/saas/referrals/codes` — list this org's codes + redemptions;
//! * `POST   /api/saas/referrals/codes/:code/disable` — retire a code;
//! * `GET    /api/saas/referrals/attribution` — who referred this org, and
//!   which orgs this org referred;
//! * `POST   /api/saas/referrals/redeem` — attribute THIS org to a code
//!   (once per org, forever — the DB UNIQUE constraint is the arbiter).
//!
//! Design rules from the schema:
//! * codes are minted SERVER-SIDE from the OS RNG — clients never choose
//!   codes, so a tenant cannot squat a guessable value;
//! * attribution is a durable FACT; payout policy is application logic and
//!   deliberately NOT encoded here (per the migration comments);
//! * re-signup can never re-attribute an org (anti-churn), enforced by the
//!   `referral_attributions_referred_org_unique` constraint.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use ring::rand::SecureRandom as _;
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

/// Conservative code alphabet (no 0/O/1/I confusion) at 10 chars —
/// ~56 bits of entropy, far beyond brute force through a rate-limited API.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 10;
/// Mint retries before giving up (UNIQUE collisions are astronomically
/// unlikely at 56 bits; a persistent failure means something is wrong).
const MINT_ATTEMPTS: usize = 5;

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/referrals/codes", axum::routing::post(mint_code))
        .route("/api/saas/referrals/codes", axum::routing::get(list_codes))
        .route(
            "/api/saas/referrals/codes/:code/disable",
            axum::routing::post(disable_code),
        )
        .route(
            "/api/saas/referrals/attribution",
            axum::routing::get(attribution),
        )
        .route("/api/saas/referrals/redeem", axum::routing::post(redeem))
}

/// Mint one code from the OS RNG (ring's SystemRandom — the same source
/// the signing stack trusts).
fn mint_code_value() -> Result<String, String> {
    let rng = ring::rand::SystemRandom::new();
    let mut bytes = [0u8; CODE_LEN];
    rng.fill(&mut bytes)
        .map_err(|e| format!("referral code generation failed: {e}"))?;
    Ok(bytes
        .iter()
        .map(|b| CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()] as char)
        .collect())
}

/// Storage-unavailable response (shared shape with the other P2 handlers).
fn db_unavailable(what: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "error": "referral_storage_unavailable",
            "reason": format!("{what} requires an attached PostgreSQL database"),
        })),
    )
        .into_response()
}

/// `POST /api/saas/referrals/codes` — mint a code for the caller's org.
pub async fn mint_code(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("minting a referral code");
    };

    for _ in 0..MINT_ATTEMPTS {
        let code = match mint_code_value() {
            Ok(c) => c,
            Err(reason) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "referral_mint_failed", "reason": reason.to_string() }))).into_response()
            }
        };
        let result = sqlx::query(
            "INSERT INTO referral_codes (code, organization_id, created_by, status) VALUES ($1, $2, $3, 'active')",
        )
        .bind(&code)
        .bind(ctx.organization.id.as_uuid())
        .bind(ctx.authorization.user_id.map(|u| u.as_uuid()))
        .execute(db.pool())
        .await;
        match result {
            Ok(_) => {
                state
                    .audit
                    .success(
                        &ctx.actor_label(),
                        "saas.referral.code_minted",
                        Some(&ctx.organization.id.to_string()),
                    )
                    .await;
                return (
                    StatusCode::CREATED,
                    Json(json!({
                        "code": code,
                        "organization_id": ctx.organization.id.to_string(),
                        "status": "active",
                    })),
                )
                    .into_response();
            }
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                continue; // collision — mint another value
            }
            Err(error) => {
                tracing::error!(error = %error, "referral code insert failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "referral_storage_unavailable",
                        "reason": "referral code could not be persisted",
                    })),
                )
                    .into_response();
            }
        }
    }
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "error": "referral_mint_failed",
            "reason": "code space exhausted after retries",
        })),
    )
        .into_response()
}

/// `GET /api/saas/referrals/codes` — this org's codes with redemption counts.
pub async fn list_codes(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("listing referral codes");
    };

    let rows = sqlx::query(
        "SELECT c.code, c.status, c.created_at, COUNT(a.id) AS redemptions \
         FROM referral_codes c \
         LEFT JOIN referral_attributions a ON a.referral_code_id = c.id \
         WHERE c.organization_id = $1 \
         GROUP BY c.code, c.status, c.created_at \
         ORDER BY c.created_at DESC",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(error) => {
            tracing::error!(error = %error, "referral code list failed");
            return db_unavailable("listing referral codes");
        }
    };
    let codes: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            json!({
                "code": row.get::<String, _>("code"),
                "status": row.get::<String, _>("status"),
                "created_at": row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
                "redemptions": row.get::<i64, _>("redemptions"),
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "codes": codes,
        })),
    )
        .into_response()
}

/// `POST /api/saas/referrals/codes/:code/disable` — retire a code. Existing
/// attributions are untouched (they are facts, not permissions).
pub async fn disable_code(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("disabling a referral code");
    };
    let code = code.trim().to_ascii_uppercase();

    let result = sqlx::query(
        "UPDATE referral_codes SET status = 'disabled', disabled_at = now() \
         WHERE code = $1 AND organization_id = $2 AND status = 'active'",
    )
    .bind(&code)
    .bind(ctx.organization.id.as_uuid())
    .execute(db.pool())
    .await;
    match result {
        Ok(out) if out.rows_affected() == 1 => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.referral.code_disabled",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (StatusCode::OK, Json(json!({ "code": code, "status": "disabled" }))).into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "referral_code_not_found",
                "reason": "no active code with that value belongs to this organization",
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "referral code disable failed");
            db_unavailable("disabling a referral code")
        }
    }
}

/// `GET /api/saas/referrals/attribution` — inbound attribution (who referred
/// this org) and outbound list (orgs this org referred). Never leaks other
/// orgs' codes or identities beyond the fact of attribution.
pub async fn attribution(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::BotRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("reading referral attribution");
    };
    let org = ctx.organization.id.as_uuid();

    // Inbound: was THIS org referred, and by which code owner?
    let inbound = sqlx::query(
        "SELECT o.name AS referrer_name, o.slug AS referrer_slug, a.attributed_at \
         FROM referral_attributions a \
         JOIN referral_codes c ON c.id = a.referral_code_id \
         JOIN organizations o ON o.id = c.organization_id \
         WHERE a.referred_organization_id = $1",
    )
    .bind(org)
    .fetch_optional(db.pool())
    .await;
    let inbound = match inbound {
        Ok(row) => row.map(|row| {
            json!({
                "referred_by": row.get::<String, _>("referrer_name"),
                "referrer_slug": row.get::<String, _>("referrer_slug"),
                "attributed_at": row.get::<chrono::DateTime<chrono::Utc>, _>("attributed_at"),
            })
        }),
        Err(error) => {
            tracing::error!(error = %error, "referral inbound query failed");
            return db_unavailable("reading referral attribution");
        }
    };

    // Outbound: orgs that redeemed OUR codes.
    let outbound_rows = sqlx::query(
        "SELECT o.name AS referred_name, o.slug AS referred_slug, a.attributed_at, c.code \
         FROM referral_attributions a \
         JOIN referral_codes c ON c.id = a.referral_code_id \
         JOIN organizations o ON o.id = a.referred_organization_id \
         WHERE c.organization_id = $1 \
         ORDER BY a.attributed_at DESC",
    )
    .bind(org)
    .fetch_all(db.pool())
    .await;
    let outbound_rows = match outbound_rows {
        Ok(r) => r,
        Err(error) => {
            tracing::error!(error = %error, "referral outbound query failed");
            return db_unavailable("reading referral attribution");
        }
    };
    let referred: Vec<serde_json::Value> = outbound_rows
        .iter()
        .map(|row| {
            json!({
                "organization": row.get::<String, _>("referred_name"),
                "slug": row.get::<String, _>("referred_slug"),
                "code": row.get::<String, _>("code"),
                "attributed_at": row.get::<chrono::DateTime<chrono::Utc>, _>("attributed_at"),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": ctx.organization.id.to_string(),
            "referred_by": inbound,
            "referred_organizations": referred,
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct RedeemBody {
    /// The code to attribute this org to (case-insensitive).
    pub code: String,
}

/// `POST /api/saas/referrals/redeem` — attribute the caller's CURRENT org
/// to a code. Once per org forever (DB UNIQUE); a code can never refer its
/// own owner.
pub async fn redeem(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<RedeemBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::manage(Permission::TenantUpdate),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };
    let Some(db) = state.db.as_deref() else {
        return db_unavailable("redeeming a referral code");
    };
    let code = body.code.trim().to_ascii_uppercase();

    // Already attributed? Surface the fact instead of an opaque conflict.
    let existing = sqlx::query(
        "SELECT 1 FROM referral_attributions WHERE referred_organization_id = $1",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await;
    match existing {
        Ok(Some(_)) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "already_attributed",
                    "reason": "this organization was already referred; attribution is permanent",
                })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::error!(error = %error, "referral attribution check failed");
            return db_unavailable("redeeming a referral code");
        }
        Ok(None) => {}
    }

    // Resolve the code; it must exist, be active, and belong to ANOTHER org.
    let code_row = sqlx::query(
        "SELECT id, organization_id FROM referral_codes WHERE code = $1 AND status = 'active'",
    )
    .bind(&code)
    .fetch_optional(db.pool())
    .await;
    let code_row = match code_row {
        Ok(r) => r,
        Err(error) => {
            tracing::error!(error = %error, "referral code lookup failed");
            return db_unavailable("redeeming a referral code");
        }
    };
    let Some(code_row) = code_row else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "referral_code_invalid",
                "reason": "no active referral code with that value",
            })),
        )
            .into_response();
    };
    let code_id: uuid::Uuid = code_row.get("id");
    let owner_id: uuid::Uuid = code_row.get("organization_id");
    if owner_id == ctx.organization.id.as_uuid() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "self_referral",
                "reason": "an organization cannot redeem its own code",
            })),
        )
            .into_response();
    }

    let insert = sqlx::query(
        "INSERT INTO referral_attributions (referral_code_id, referred_organization_id, referred_user_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(code_id)
    .bind(ctx.organization.id.as_uuid())
    .bind(ctx.authorization.user_id.map(|u| u.as_uuid()))
    .execute(db.pool())
    .await;
    match insert {
        Ok(_) => {
            state
                .audit
                .success(
                    &ctx.actor_label(),
                    "saas.referral.attributed",
                    Some(&ctx.organization.id.to_string()),
                )
                .await;
            (
                StatusCode::CREATED,
                Json(json!({
                    "organization_id": ctx.organization.id.to_string(),
                    "attributed": true,
                })),
            )
                .into_response()
        }
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "already_attributed",
                "reason": "this organization was already referred; attribution is permanent",
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!(error = %error, "referral attribution insert failed");
            db_unavailable("redeeming a referral code")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_codes_use_the_safe_alphabet_and_length() {
        for _ in 0..50 {
            let code = mint_code_value().unwrap();
            assert_eq!(code.len(), CODE_LEN);
            assert!(code
                .chars()
                .all(|c| CODE_ALPHABET.contains(&(c as u8))));
        }
    }

    #[test]
    fn minted_codes_are_not_constant() {
        let a = mint_code_value().unwrap();
        let b = mint_code_value().unwrap();
        assert_ne!(a, b, "two OS-RNG codes must differ");
    }
}
