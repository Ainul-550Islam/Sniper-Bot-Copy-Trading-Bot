//! Tenant execution / transaction handlers (PROMPT 3/10 #62).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use bot_core::membership::Permission;
use chrono::Utc;

use super::orders::plane_error;
use crate::api::ApiState;

/// `GET /api/tenant/executions?limit=&cursor=&order_id=` — the
/// caller's execution attempts.
pub async fn list(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // §H: the full customer-API authorization chain — authenticate →
    // organization (credential-side only) → plane → lifecycle →
    // entitlement → module family. Denials carry typed codes.
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let org = auth.organization_id();
    let plane = auth.plane.clone();
    let scope = plane.read_scope(org);
    if let Some(order_id) = params.get("order_id") {
        return match plane
            .executions_read()
            .list_for_order(&scope, order_id, 500)
            .await
        {
            Ok(rows) => (
                StatusCode::OK,
                Json(json!({ "organization_id": org.to_string(), "items": rows })),
            )
                .into_response(),
            Err(e) => plane_error(e),
        };
    }
    let (since, until) = match window_or_response(&params) {
        Ok(w) => w,
        Err(response) => return response,
    };
    match plane
        .executions_read()
        .list_between(&scope, since, until, 1000)
        .await
    {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "organization_id": org.to_string(), "items": rows })),
        )
            .into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/executions/:id` — one of the caller's executions.
pub async fn get_one(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let scope = plane.read_scope(auth.organization_id());
    let id = match id.parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "invalid_execution_id" })),
            )
                .into_response()
        }
    };
    match plane.executions_read().get(&scope, id).await {
        Ok(execution) => (StatusCode::OK, Json(json!({ "execution": execution }))).into_response(),
        Err(e) => plane_error(e),
    }
}

/// `GET /api/tenant/transactions/:signature` — the caller's on-chain
/// transaction record (the signature is a global identity; the
/// repository still answers only for the acting tenant's row).
pub async fn transaction(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Path(signature): Path<String>,
) -> Response {
    // §H: full customer-API authorization chain (authenticate → org
    // from credential → plane → lifecycle → entitlement → module family).
    let auth = match super::authorization_chain::guard(
        &state,
        &headers,
        Permission::OrderRead,
        super::authorization_chain::TradingModuleFamily::CoreTrading,
    )
    .await
    {
        Ok(a) => a,
        Err(response) => return response,
    };
    let plane = auth.plane.clone();
    let scope = plane.read_scope(auth.organization_id());
    match plane
        .executions_read()
        .transaction(&scope, &signature)
        .await
    {
        Ok(tx) => match tx {
            Some(tx) => (StatusCode::OK, Json(json!({ "transaction": tx }))).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "not_found", "kind": "transaction" })),
            )
                .into_response(),
        },
        Err(e) => plane_error(e),
    }
}

/// A resolved `(since, until)` reporting window.
pub(super) type Window = (chrono::DateTime<Utc>, chrono::DateTime<Utc>);

/// Why a requested reporting window was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowError {
    /// A present `since`/`until` is not an RFC 3339 timestamp.
    Malformed,
    /// `since >= until`, or a window longer than [`MAX_WINDOW_DAYS`].
    Empty,
}

impl WindowError {
    /// Stable machine-readable reason for the JSON body and the logs.
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            WindowError::Malformed => "window_timestamp_is_not_rfc3339",
            WindowError::Empty => "window_must_be_a_positive_range_of_at_most_365_days",
        }
    }
}

/// The widest window a reporting query may ask for. Every read is capped, but a
/// request for "the last ten years" still makes the database touch the whole
/// journal before the cap applies — so the bound is enforced up front and
/// documented instead of surfacing as an arbitrary truncation.
pub(super) const MAX_WINDOW_DAYS: i64 = 365;

/// `?since=<rfc3339>&until=<rfc3339>` (defaults: the 24h ending now).
///
/// **A supplied timestamp that cannot be parsed is REFUSED, never silently
/// replaced.** The old helper fell back to "now" per parameter, so
/// `?since=garbage` quietly became the default 24-hour window and
/// `?until=garbage` became `now` — the caller got a plausible-looking page of
/// data for a period it never asked for, with no signal that its request was
/// ignored.
pub(super) fn window(params: &HashMap<String, String>) -> Result<Window, WindowError> {
    let parse = |key: &str| -> Result<Option<chrono::DateTime<Utc>>, WindowError> {
        match params.get(key) {
            None => Ok(None),
            Some(raw) => chrono::DateTime::parse_from_rfc3339(raw.trim())
                .map(|d| Some(d.with_timezone(&Utc)))
                .map_err(|_| WindowError::Malformed),
        }
    };
    let until = parse("until")?.unwrap_or_else(Utc::now);
    let since = match parse("since")? {
        Some(since) => since,
        None => until - chrono::Duration::hours(24),
    };
    if since >= until {
        return Err(WindowError::Empty);
    }
    if until - since > chrono::Duration::days(MAX_WINDOW_DAYS) {
        return Err(WindowError::Empty);
    }
    Ok((since, until))
}

/// Convenience for handlers that want the caller's window or the refusal
/// response, in one step.
pub(super) fn window_or_response(params: &HashMap<String, String>) -> Result<Window, Response> {
    window(params).map_err(bad_window)
}

pub(super) fn bad_window(reason: WindowError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "invalid_window", "reason": reason.as_str() })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{window, WindowError, MAX_WINDOW_DAYS};
    use std::collections::HashMap;

    fn params(kv: &[(&str, &str)]) -> HashMap<String, String> {
        kv.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn a_malformed_timestamp_is_refused_and_never_replaced_by_a_default() {
        // The old helper parsed each parameter with `.ok()` and fell back to
        // "now"/"24h ago", so `?since=yesterday` returned a REAL page of the
        // WRONG period with a 200. This must be a refusal.
        assert_eq!(
            window(&params(&[("since", "yesterday")])).unwrap_err(),
            WindowError::Malformed
        );
        assert_eq!(
            window(&params(&[("until", "2026-13-45T99:99:99Z")])).unwrap_err(),
            WindowError::Malformed
        );
        assert_eq!(
            window(&params(&[
                ("since", "2026-11-01T00:00:00Z"),
                ("until", "not-a-time")
            ]))
            .unwrap_err(),
            WindowError::Malformed
        );
        // An empty string is malformed too (it is not "absent").
        assert_eq!(
            window(&params(&[("until", "  ")])).unwrap_err(),
            WindowError::Malformed
        );
        // Non-ASCII junk must not panic either.
        assert_eq!(
            window(&params(&[("since", "недавно")])).unwrap_err(),
            WindowError::Malformed
        );
    }

    #[test]
    fn a_valid_window_is_returned_exactly_and_defaults_only_when_absent() {
        let (since, until) = window(&params(&[
            ("since", "2026-11-01T00:00:00Z"),
            ("until", "2026-11-02T00:00:00Z"),
        ]))
        .unwrap();
        assert_eq!(
            until.to_rfc3339(),
            "2026-11-02T00:00:00+00:00",
            "the caller's instant is honoured exactly"
        );
        assert_eq!(since.to_rfc3339(), "2026-11-01T00:00:00+00:00");

        // No parameters at all ⇒ the documented 24h default.
        let (since, until) = window(&params(&[])).unwrap();
        assert_eq!(until - since, chrono::Duration::hours(24));

        // Only `until` ⇒ 24h before it (not 24h before "now").
        let (since, until) = window(&params(&[("until", "2026-11-02T00:00:00Z")])).unwrap();
        assert_eq!(until.to_rfc3339(), "2026-11-02T00:00:00+00:00");
        assert_eq!(since.to_rfc3339(), "2026-11-01T00:00:00+00:00");
    }

    #[test]
    fn a_reversed_impossible_or_oversized_window_is_refused() {
        // Reversed (the old code silently served "today").
        assert_eq!(
            window(&params(&[
                ("since", "2026-11-02T00:00:00Z"),
                ("until", "2026-11-01T00:00:00Z")
            ]))
            .unwrap_err(),
            WindowError::Empty
        );
        // Empty (since == until).
        assert_eq!(
            window(&params(&[
                ("since", "2026-11-01T00:00:00Z"),
                ("until", "2026-11-01T00:00:00Z")
            ]))
            .unwrap_err(),
            WindowError::Empty
        );
        // Wider than the documented cap.
        let too_far =
            (chrono::Utc::now() + chrono::Duration::days(MAX_WINDOW_DAYS + 1)).to_rfc3339();
        assert_eq!(
            window(&params(&[
                ("since", "2020-01-01T00:00:00Z"),
                ("until", &too_far)
            ]))
            .unwrap_err(),
            WindowError::Empty
        );
        // Exactly at the cap is allowed.
        let (since, until) = window(&params(&[
            ("since", "2026-01-01T00:00:00Z"),
            ("until", "2026-06-01T00:00:00Z"),
        ]))
        .unwrap();
        assert_eq!(until - since, chrono::Duration::days(151));
    }

    #[test]
    fn the_refusal_reason_is_machine_readable() {
        assert_eq!(
            WindowError::Malformed.as_str(),
            "window_timestamp_is_not_rfc3339"
        );
        assert!(WindowError::Empty.as_str().contains("365"));
    }
}
