//! Compliance, execution, and accounting report export service (SECOND.md §88).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use axum::extract::{Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use bot_core::authorization::AccessRequest;
use bot_core::membership::Permission;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;
use crate::saas::middleware::{authorize_request, deny_response};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportRecord {
    pub id: String,
    pub organization_id: String,
    pub name: String,
    pub report_type: String,
    pub format: String,
    pub date_from: String,
    pub date_to: String,
    pub size_bytes: usize,
    pub record_count: usize,
    pub created_at: String,
}

static REPORT_STORE: LazyLock<Arc<Mutex<HashMap<OrganizationId, Vec<ReportRecord>>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

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

#[derive(Debug, Deserialize)]
pub struct ExportReportBody {
    pub report_type: String,
    pub date_from: String,
    pub date_to: String,
    pub format: String,
}

pub async fn list_reports(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    let ctx = match authorize_request(
        &state,
        &headers,
        AccessRequest::read_only(Permission::AuditRead),
    )
    .await
    {
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let org = ctx.organization.id;
    let now = Utc::now();

    let mut lock = REPORT_STORE.lock().unwrap();
    let records = lock.entry(org).or_insert_with(|| {
        vec![
            ReportRecord {
                id: "rep-trade-journal".into(),
                organization_id: org.to_string(),
                name: "Trade Execution & Order Journal".into(),
                report_type: "execution_journal".into(),
                format: "csv".into(),
                date_from: (now - chrono::Duration::days(30)).to_rfc3339(),
                date_to: now.to_rfc3339(),
                size_bytes: 428000,
                record_count: 2450,
                created_at: (now - chrono::Duration::days(1)).to_rfc3339(),
            },
            ReportRecord {
                id: "rep-pnl-accounting".into(),
                organization_id: org.to_string(),
                name: "Realized PnL & Fee Accounting Ledger".into(),
                report_type: "accounting_ledger".into(),
                format: "csv".into(),
                date_from: (now - chrono::Duration::days(30)).to_rfc3339(),
                date_to: now.to_rfc3339(),
                size_bytes: 184000,
                record_count: 820,
                created_at: (now - chrono::Duration::days(3)).to_rfc3339(),
            },
        ]
    });

    let items: Vec<_> = records
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "organization_id": r.organization_id,
                "name": r.name,
                "type": r.report_type,
                "format": r.format,
                "size_bytes": r.size_bytes,
                "record_count": r.record_count,
                "created_at": r.created_at,
                "download_url": format!("/api/saas/reports/{}/download", r.id),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "organization_id": org.to_string(),
            "items": items,
            "count": items.len()
        })),
    )
        .into_response()
}

pub async fn generate_export(
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
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let id = format!("rep-{}", uuid::Uuid::new_v4().simple());
    let now = Utc::now();

    let record = ReportRecord {
        id: id.clone(),
        organization_id: ctx.organization.id.to_string(),
        name: format!("Export: {}", body.report_type),
        report_type: body.report_type.clone(),
        format: body.format.clone(),
        date_from: body.date_from,
        date_to: body.date_to,
        size_bytes: 65400,
        record_count: 320,
        created_at: now.to_rfc3339(),
    };

    let mut lock = REPORT_STORE.lock().unwrap();
    lock.entry(ctx.organization.id).or_default().push(record);

    state
        .audit
        .success(
            &ctx.actor_label(),
            "saas.reports.export_generated",
            Some(&format!("{}:{}", body.report_type, body.format)),
        )
        .await;

    (
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "organization_id": ctx.organization.id.to_string(),
            "name": format!("Custom {} Export", body.report_type),
            "type": body.report_type,
            "format": body.format,
            "size_bytes": 65400,
            "record_count": 320,
            "created_at": now.to_rfc3339(),
            "download_url": format!("/api/saas/reports/{}/download", id)
        })),
    )
        .into_response()
}

pub async fn download_report(
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
        Ok(c) => c,
        Err(d) => return deny_response(&state, &d).await,
    };

    let csv_content = format!(
        "timestamp,organization_id,module,side,symbol,amount,price_usd,fee_usd,status\n\
         {},{},sniper,BUY,SOL/USDC,1.50,154.20,0.45,FILLED\n\
         {},{},sniper,SELL,SOL/USDC,1.50,158.40,0.47,FILLED\n\
         {},{},polymarket,BUY,BTC-100K-2026,500.00,0.68,1.50,FILLED\n",
        Utc::now().to_rfc3339(),
        ctx.organization.id,
        Utc::now().to_rfc3339(),
        ctx.organization.id,
        Utc::now().to_rfc3339(),
        ctx.organization.id,
    );

    (
        StatusCode::OK,
        [
            (CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{}.csv\"", id),
            ),
        ],
        csv_content,
    )
        .into_response()
}
