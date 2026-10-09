//! Durable tenant report generation and download service.
//!
//! Reports are artifacts built from tenant-scoped database rows. An empty
//! source range produces an empty report, not synthetic record counts or
//! sample trades.

use std::collections::BTreeSet;

use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::postgres::PgRow;
use sqlx::Row;
use uuid::Uuid;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

const REPORT_TYPES: &[&str] = &[
    "accounting_ledger",
    "execution_journal",
    "security_audit",
    "risk_events",
    "custody_rotation",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportReportBody {
    pub report_type: String,
    pub date_from: String,
    pub date_to: String,
    pub format: String,
}

pub fn routes() -> Router<ApiState> {
    Router::new()
        .route("/api/saas/reports", axum::routing::get(list_reports))
        .route(
            "/api/saas/reports/export",
            axum::routing::post(generate_export),
        )
        .route(
            "/api/saas/reports/:id/download",
            axum::routing::get(download_report),
        )
}

fn error_response(status: StatusCode, error: &'static str, detail: impl Into<String>) -> Response {
    (
        status,
        Json(json!({ "error": error, "detail": detail.into() })),
    )
        .into_response()
}

fn database(state: &ApiState) -> Result<&bot_core::db::Database, Response> {
    state.db.as_deref().ok_or_else(|| {
        error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "report_storage_unavailable",
            "report generation requires an attached PostgreSQL database",
        )
    })
}

fn parse_window(body: &ExportReportBody) -> Result<(DateTime<Utc>, DateTime<Utc>), Response> {
    let from = DateTime::parse_from_rfc3339(&body.date_from)
        .map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_report_window",
                "date_from must be RFC3339",
            )
        })?
        .with_timezone(&Utc);
    let to = DateTime::parse_from_rfc3339(&body.date_to)
        .map_err(|_| {
            error_response(
                StatusCode::BAD_REQUEST,
                "invalid_report_window",
                "date_to must be RFC3339",
            )
        })?
        .with_timezone(&Utc);
    if to < from {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_report_window",
            "date_to must not precede date_from",
        ));
    }
    if to - from > chrono::Duration::days(366) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "report_window_too_large",
            "report windows may not exceed 366 days",
        ));
    }
    Ok((from, to))
}

fn optional_text(row: &PgRow, column: &str) -> Result<Option<String>, Response> {
    row.try_get(column).map_err(|error| {
        tracing::error!(error = %error, column, "report optional text column could not be decoded");
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "report_decode_error",
            "report source row could not be decoded",
        )
    })
}

fn validate_request(body: &ExportReportBody) -> Result<(DateTime<Utc>, DateTime<Utc>), Response> {
    if !REPORT_TYPES.contains(&body.report_type.as_str()) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "invalid_report_type",
            "report_type is not supported",
        ));
    }
    if body.format != "csv" && body.format != "json" {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            "unsupported_report_format",
            "only csv and json reports are supported",
        ));
    }
    parse_window(body)
}

async fn source_rows(
    db: &bot_core::db::Database,
    organization_id: bot_core::tenant::OrganizationId,
    report_type: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<Value>, Response> {
    let rows = match report_type {
        "accounting_ledger" | "execution_journal" => sqlx::query(
            "SELECT ts, source, venue, mode, side, symbol, amount_in, amount_out, price, fee, slippage_bps, signature
               FROM trades
              WHERE organization_id = $1 AND ts >= $2 AND ts <= $3
              ORDER BY ts ASC, id ASC",
        )
        .bind(organization_id.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_all(db.pool())
        .await,
        "security_audit" => sqlx::query(
            "SELECT id, ts, actor, action, target, outcome, detail
               FROM audit_events
              WHERE organization_id = $1 AND ts >= $2 AND ts <= $3
              ORDER BY ts ASC, id ASC",
        )
        .bind(organization_id.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_all(db.pool())
        .await,
        "risk_events" => sqlx::query(
            "SELECT id, ts, module, kind, symbol, reason, snapshot
               FROM risk_events
              WHERE organization_id = $1 AND ts >= $2 AND ts <= $3
              ORDER BY ts ASC, id ASC",
        )
        .bind(organization_id.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_all(db.pool())
        .await,
        "custody_rotation" => sqlx::query(
            "SELECT id, created_at, action, from_status, to_status, from_provider, to_provider, reason, actor
               FROM custody_audit
              WHERE organization_id = $1 AND created_at >= $2 AND created_at <= $3
              ORDER BY created_at ASC, id ASC",
        )
        .bind(organization_id.as_uuid())
        .bind(from)
        .bind(to)
        .fetch_all(db.pool())
        .await,
        _ => unreachable!("validated report type"),
    }
    .map_err(|error| {
        tracing::error!(error = %error, report_type, "failed to load report source rows");
        error_response(StatusCode::SERVICE_UNAVAILABLE, "report_source_error", "report source rows could not be loaded")
    })?;

    rows.into_iter()
        .map(|row| -> Result<Value, Response> {
            Ok(match report_type {
                "accounting_ledger" | "execution_journal" => json!({
                    "timestamp": row.get::<DateTime<Utc>, _>("ts"),
                    "source": row.get::<String, _>("source"),
                    "venue": row.get::<String, _>("venue"),
                    "mode": row.get::<String, _>("mode"),
                    "side": row.get::<String, _>("side"),
                    "symbol": row.get::<String, _>("symbol"),
                    "amount_in": row.get::<f64, _>("amount_in"),
                    "amount_out": row.get::<f64, _>("amount_out"),
                    "price": row.get::<f64, _>("price"),
                    "fee": row.get::<f64, _>("fee"),
                    "slippage_bps": row.get::<i64, _>("slippage_bps"),
                    "signature": optional_text(&row, "signature")?,
                }),
                "security_audit" => json!({
                    "id": row.get::<i64, _>("id"),
                    "timestamp": row.get::<DateTime<Utc>, _>("ts"),
                    "actor": row.get::<String, _>("actor"),
                    "action": row.get::<String, _>("action"),
                    "target": optional_text(&row, "target")?,
                    "outcome": row.get::<String, _>("outcome"),
                    "detail": row.get::<Value, _>("detail"),
                }),
                "risk_events" => json!({
                    "id": row.get::<i64, _>("id"),
                    "timestamp": row.get::<DateTime<Utc>, _>("ts"),
                    "module": row.get::<String, _>("module"),
                    "kind": row.get::<String, _>("kind"),
                    "symbol": optional_text(&row, "symbol")?,
                    "reason": row.get::<String, _>("reason"),
                    "snapshot": row.get::<Value, _>("snapshot"),
                }),
                "custody_rotation" => json!({
                    "id": row.get::<Uuid, _>("id"),
                    "timestamp": row.get::<DateTime<Utc>, _>("created_at"),
                    "action": row.get::<String, _>("action"),
                    "from_status": optional_text(&row, "from_status")?,
                    "to_status": optional_text(&row, "to_status")?,
                    "from_provider": optional_text(&row, "from_provider")?,
                    "to_provider": optional_text(&row, "to_provider")?,
                    "reason": row.get::<String, _>("reason"),
                    "actor": row.get::<String, _>("actor"),
                }),
                _ => Value::Object(Default::default()),
            })
        })
        .collect()
}

fn csv_escape(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn encode_rows(rows: &[Value], format: &str) -> Result<(Vec<u8>, &'static str), Response> {
    if format == "json" {
        let bytes = serde_json::to_vec(rows).map_err(|_| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "report_encoding_error",
                "report could not be encoded",
            )
        })?;
        return Ok((bytes, "application/json; charset=utf-8"));
    }
    let mut keys = BTreeSet::new();
    for row in rows {
        if let Some(object) = row.as_object() {
            keys.extend(object.keys().cloned());
        }
    }
    let ordered_keys: Vec<String> = keys.into_iter().collect();
    let mut output = String::new();
    output.push_str(
        &ordered_keys
            .iter()
            .map(|key| csv_escape(key))
            .collect::<Vec<_>>()
            .join(","),
    );
    output.push('\n');
    for row in rows {
        let object = row.as_object();
        let values = ordered_keys
            .iter()
            .map(|key| {
                object
                    .and_then(|value| value.get(key))
                    .map(Value::to_string)
                    .unwrap_or_default()
            })
            .map(|value| csv_escape(&value))
            .collect::<Vec<_>>();
        output.push_str(&values.join(","));
        output.push('\n');
    }
    Ok((output.into_bytes(), "text/csv; charset=utf-8"))
}

async fn list_reports(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::AuditRead),
    )
    .await
    {
        Ok(value) => value,
        Err(denial) => return deny_response(&state, &denial).await,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let rows = match sqlx::query(
        "SELECT id, name, report_type, format, date_from, date_to, octet_length(content)::bigint AS size_bytes, record_count, created_at
           FROM report_exports
          WHERE organization_id = $1
          ORDER BY created_at DESC, id DESC",
    )
    .bind(ctx.organization.id.as_uuid())
    .fetch_all(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to list tenant reports");
            return error_response(StatusCode::SERVICE_UNAVAILABLE, "report_storage_error", "reports could not be loaded");
        }
    };
    let items: Vec<_> = rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.get::<Uuid, _>("id"),
                "organization_id": ctx.organization.id,
                "name": row.get::<String, _>("name"),
                "type": row.get::<String, _>("report_type"),
                "format": row.get::<String, _>("format"),
                "size_bytes": row.get::<i64, _>("size_bytes"),
                "record_count": row.get::<i64, _>("record_count"),
                "created_at": row.get::<DateTime<Utc>, _>("created_at"),
                "download_url": format!("/api/saas/reports/{}/download", row.get::<Uuid, _>("id")),
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(
            json!({ "organization_id": ctx.organization.id, "items": items, "count": items.len() }),
        ),
    )
        .into_response()
}

async fn generate_export(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<ExportReportBody>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::AuditRead),
    )
    .await
    {
        Ok(value) => value,
        Err(denial) => return deny_response(&state, &denial).await,
    };
    let (from, to) = match validate_request(&body) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let rows = match source_rows(db, ctx.organization.id, &body.report_type, from, to).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let (content, content_type) = match encode_rows(&rows, &body.format) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = Uuid::new_v4();
    let name = format!("{} export", body.report_type.replace('_', " "));
    if let Err(error) = sqlx::query(
        "INSERT INTO report_exports
             (id, organization_id, name, report_type, format, date_from, date_to, content_type, content, record_count)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(ctx.organization.id.as_uuid())
    .bind(&name)
    .bind(&body.report_type)
    .bind(&body.format)
    .bind(from)
    .bind(to)
    .bind(content_type)
    .bind(&content)
    .bind(rows.len() as i64)
    .execute(db.pool())
    .await
    {
        tracing::error!(error = %error, "failed to persist generated report");
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "report_storage_error", "report could not be saved");
    }
    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.reports.export_generated",
            Some(&id.to_string()),
        )
        .await;
    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "organization_id": ctx.organization.id,
            "name": name,
            "type": body.report_type,
            "format": body.format,
            "size_bytes": content.len(),
            "record_count": rows.len(),
            "created_at": Utc::now(),
            "download_url": format!("/api/saas/reports/{id}/download"),
        })),
    )
        .into_response()
}

async fn download_report(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::AuditRead),
    )
    .await
    {
        Ok(value) => value,
        Err(denial) => return deny_response(&state, &denial).await,
    };
    let db = match database(&state) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let report_id = match Uuid::parse_str(&id) {
        Ok(value) => value,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "report_not_found",
                "report was not found",
            )
        }
    };
    let row = match sqlx::query(
        "SELECT format, content_type, content
           FROM report_exports
          WHERE id = $1 AND organization_id = $2",
    )
    .bind(report_id)
    .bind(ctx.organization.id.as_uuid())
    .fetch_optional(db.pool())
    .await
    {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(error = %error, "failed to load tenant report");
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "report_storage_error",
                "report could not be loaded",
            );
        }
    };
    let Some(row) = row else {
        return error_response(
            StatusCode::NOT_FOUND,
            "report_not_found",
            "report was not found",
        );
    };
    let format: String = row.get("format");
    let content_type: String = row.get("content_type");
    let content: Vec<u8> = row.get("content");
    let mut response_headers = HeaderMap::new();
    response_headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    let filename = format!("report-{id}.{format}");
    response_headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    (StatusCode::OK, response_headers, content).into_response()
}
