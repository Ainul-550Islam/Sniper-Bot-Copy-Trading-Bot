//! Tenant-scoped audit export endpoint with secret redaction.

use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};
use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    Json, Router,
};
use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Deserialize)]
pub struct AuditExportQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub format: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
struct AuditExportRecord {
    id: String,
    organization_id: String,
    actor: String,
    action: String,
    outcome: String,
    at: String,
    detail: serde_json::Value,
}

pub fn routes() -> Router<ApiState> {
    Router::new().route("/api/saas/audit/export", axum::routing::get(export))
}

/// Export tenant audit records with bounded ranges and redaction.
pub async fn export(
    State(state): State<ApiState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<AuditExportQuery>,
) -> Response {
    let ctx = match authorize_request(&state, &headers, AccessRequest::read(Permission::AuditRead))
        .await
    {
        Ok(value) => value,
        Err(denial) => return deny_response(&state, &denial).await,
    };
    let org = ctx.organization.id;
    let now = Utc::now();
    let from = match q.from.as_deref() {
        Some(value) => match DateTime::parse_from_rfc3339(value) {
            Ok(parsed) => parsed.with_timezone(&Utc),
            Err(_) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(json!({"error":"invalid_from","reason":"from must be RFC3339"})),
                )
                    .into_response()
            }
        },
        None => now - chrono::Duration::days(30),
    };
    let to = match q.to.as_deref() {
        Some(value) => match DateTime::parse_from_rfc3339(value) {
            Ok(parsed) => parsed.with_timezone(&Utc),
            Err(_) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(json!({"error":"invalid_to","reason":"to must be RFC3339"})),
                )
                    .into_response()
            }
        },
        None => now,
    };
    if from >= to {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_range","reason":"from must be before to"})),
        )
            .into_response();
    }
    if to - from > chrono::Duration::days(366) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error":"range_too_large","reason":"audit export range cannot exceed 366 days"})),
        )
            .into_response();
    }
    if let Some(format) = q.format.as_deref() {
        if format != "json" && format != "csv" {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error":"invalid_format","reason":"format must be json or csv"})),
            )
                .into_response();
        }
    }
    let limit = q.limit.unwrap_or(100).min(1000);
    let offset = q.offset.unwrap_or(0);

    let records = match fetch_audit_records(&state, org, from, to, limit, offset).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let redacted: Vec<AuditExportRecord> = records.into_iter().map(redact_record).collect();

    if q.format.as_deref() == Some("csv") {
        let mut csv = String::from("id,organization_id,actor,action,outcome,at,detail\n");
        for record in &redacted {
            let detail = serde_json::to_string(&record.detail).unwrap_or_else(|_| "{}".to_string());
            let fields = [
                record.id.as_str(),
                record.organization_id.as_str(),
                record.actor.as_str(),
                record.action.as_str(),
                record.outcome.as_str(),
                record.at.as_str(),
                detail.as_str(),
            ];
            csv.push_str(
                &fields
                    .iter()
                    .map(|field| csv_escape(field))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            csv.push('\n');
        }
        let mut response_headers = HeaderMap::new();
        response_headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("text/csv; charset=utf-8"),
        );
        response_headers.insert(
            CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment; filename=\"audit-export.csv\""),
        );
        state
            .audit
            .record(
                "saas",
                "saas.audit.export",
                Some(&org.to_string()),
                bot_core::audit::AuditOutcome::Success,
                json!({
                    "organization": org.to_string(),
                    "from": from.to_rfc3339(),
                    "to": to.to_rfc3339(),
                    "count": redacted.len(),
                    "format": "csv",
                }),
            )
            .await;
        return (axum::http::StatusCode::OK, response_headers, csv).into_response();
    }

    state
        .audit
        .record(
            "saas",
            "saas.audit.export",
            Some(&org.to_string()),
            bot_core::audit::AuditOutcome::Success,
            json!({
                "organization": org.to_string(),
                "from": from.to_rfc3339(),
                "to": to.to_rfc3339(),
                "count": redacted.len(),
            }),
        )
        .await;

    (
        axum::http::StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "from": from.to_rfc3339(),
            "to": to.to_rfc3339(),
            "limit": limit,
            "offset": offset,
            "records": redacted,
            "count": redacted.len(),
        })),
    )
        .into_response()
}

async fn fetch_audit_records(
    state: &ApiState,
    org: OrganizationId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    limit: usize,
    offset: usize,
) -> Result<Vec<AuditExportRecord>, Response> {
    let Some(db) = &state.db else {
        return Err((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "audit_storage_unavailable",
                "reason": "audit export requires an attached PostgreSQL database"
            })),
        )
            .into_response());
    };
    use sqlx::Row;
    let query = r#"
        SELECT id::text,
               organization_id::text AS organization_id,
               actor,
               action,
               outcome,
               ts,
               detail
        FROM audit_events
        WHERE organization_id = $1
          AND ts >= $2
          AND ts <= $3
        ORDER BY ts ASC, id ASC
        LIMIT $4 OFFSET $5
    "#;
    let rows = sqlx::query(query)
        .bind(org.as_uuid())
        .bind(from)
        .bind(to)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(db.pool())
        .await
        .map_err(|error| {
            tracing::error!(error = %error, organization_id = %org, "failed to query audit export records");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit records could not be loaded"})),
            )
                .into_response()
        })?;

    let mut list = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id").map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit record schema is invalid"})),
            )
                .into_response()
        })?;
        let org_id: String = row.try_get("organization_id").map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit record schema is invalid"})),
            )
                .into_response()
        })?;
        let actor: String = row.try_get("actor").map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit record schema is invalid"})),
            )
                .into_response()
        })?;
        let action: String = row.try_get("action").map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit record schema is invalid"})),
            )
                .into_response()
        })?;
        let outcome: String = row.try_get("outcome").map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit record schema is invalid"})),
            )
                .into_response()
        })?;
        let ts: DateTime<Utc> = row.try_get("ts").map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit record schema is invalid"})),
            )
                .into_response()
        })?;
        let detail: serde_json::Value = row.try_get("detail").map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"audit_storage_error","reason":"audit record schema is invalid"})),
            )
                .into_response()
        })?;
        list.push(AuditExportRecord {
            id,
            organization_id: org_id,
            actor,
            action,
            outcome,
            at: ts.to_rfc3339(),
            detail,
        });
    }
    Ok(list)
}

fn csv_escape(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn redact_value(value: &mut serde_json::Value) {
    const BANNED: &[&str] = &["secret", "token", "password", "api_key", "private_key"];
    match value {
        serde_json::Value::Object(map) => {
            map.retain(|key, _| {
                let lower = key.to_ascii_lowercase();
                !BANNED.iter().any(|banned| lower.contains(banned))
            });
            for child in map.values_mut() {
                redact_value(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                redact_value(child);
            }
        }
        serde_json::Value::String(text) => {
            if text.contains("sk-") || text.contains("Bearer") {
                *value = serde_json::Value::String("**redacted**".into());
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn redact_record(mut r: AuditExportRecord) -> AuditExportRecord {
    redact_value(&mut r.detail);
    if r.actor.to_ascii_lowercase().contains("secret") {
        r.actor = "**redacted**".into();
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_range_validation() {
        let now = Utc::now();
        assert!(now - chrono::Duration::days(367) < now);
    }

    #[test]
    fn redaction_removes_secret_keys() {
        let r = AuditExportRecord {
            id: "1".into(),
            organization_id: "org".into(),
            actor: "user".into(),
            action: "test".into(),
            outcome: "success".into(),
            at: Utc::now().to_rfc3339(),
            detail: json!({"token":"x","safe":"y"}),
        };
        let redacted = redact_record(r);
        assert_eq!(redacted.detail.get("token"), None);
        assert_eq!(
            redacted.detail.get("safe").and_then(|v| v.as_str()),
            Some("y")
        );
    }
}
